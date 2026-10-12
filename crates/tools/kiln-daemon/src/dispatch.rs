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
    NativeSharedLibrarySummary, PROTOCOL_VERSION, ProjectSummary, RecordedWorkers, Request,
    ResolutionMap, Response, RunSummary, ScalarValueSummary, SourceView, Stats, StepBudget,
};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

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
        exec_argv: Vec<String>,
        disallow_string_code_generation: bool,
        has_preloads: bool,
        file: kiln_db::FileId,
        revision: u64,
        overlay: std::collections::HashMap<std::path::PathBuf, Vec<u8>>,
    },
    Program {
        program: Result<kiln_ir::Program, Vec<kiln_ir::CompileDiagnostic>>,
        path: String,
        args: Vec<String>,
        exec_argv: Vec<String>,
        disallow_string_code_generation: bool,
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
    /// Parsed TS modules and their resolved contracts are immutable for this captured source
    /// set. Graph construction asks for one payload at a time, so memoize the project check here
    /// instead of checking once per module.
    source_types: Arc<OnceLock<SnapshotSourceTypes>>,
}

#[derive(Debug)]
pub(crate) struct SnapshotSourceTypes {
    modules: BTreeMap<PathBuf, Arc<kiln_hir::Module>>,
    checked: kiln_hir::source_types::CheckResult,
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
            | Request::NodeRun { .. }
            | Request::NativeBuild { .. }
            | Request::NativeBuildAtRevision { .. }
            | Request::NativeSharedBuild { .. }
            | Request::ExplainParallel { .. }
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
            wall_clock,
            steps,
            source_view: _,
            stream_output: _,
        } => (
            run(
                engine,
                resolutions,
                path,
                project_root,
                args,
                node,
                wall_clock,
                steps,
            ),
            Next::Continue,
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
            source_view: _,
            stream_output: _,
        } => (
            node_run(
                engine,
                resolutions,
                path,
                project_root,
                args,
                preloads,
                exec_argv,
                disallow_string_code_generation,
                wall_clock,
                steps,
            ),
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
            no_kiln_extensions,
            workers,
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
                        no_kiln_extensions,
                        workers,
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
                            no_kiln_extensions,
                            workers,
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
        Request::ExplainParallel {
            path,
            project_root,
            node,
            profile,
            source_view: _,
        } => (
            match native_graph(engine, resolutions, &path, &project_root, node) {
                Ok(Some(graph)) => {
                    native_explain_parallel_graph(&graph, &path, &project_root, profile, node)
                }
                Ok(None) => match native_program(engine, &path) {
                    Ok(program) => {
                        native_explain_parallel_program(&program, &path, &project_root, profile)
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
            wall_clock,
            source_view: _,
            static_link,
            pgo_generate,
            pgo_use,
            stream_output: _,
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
                        wall_clock,
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
                            wall_clock,
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
        Request::TestInventory { root } => (
            match crate::testing::ConformanceCoordinator::default().inventory(&root) {
                Ok(tests) => Response::TestInventory(tests),
                Err(message) => Response::Error { message },
            },
            Next::Continue,
        ),
        Request::TestBatch { root, mode, ids } => (
            match crate::testing::ConformanceCoordinator::default().batch(&root, mode, ids) {
                Ok(summary) => Response::Tested(summary),
                Err(message) => Response::Error { message },
            },
            Next::Continue,
        ),
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
    // A TypeScript entry's check runs on its own thread while the JavaScript it imports, which
    // needs none of its facts, compiles.
    if matches!(
        parsed.module.source_kind,
        kiln_hir::SourceKind::TypeScript | kiln_hir::SourceKind::Tsx
    ) && parsed.module.diagnostics.is_empty()
    {
        engine.begin_source_check(file);
        prepare_javascript_irs(engine, &project_root, resolutions);
    }
    // The normalized interface is the per-file declared surface and incremental cutoff. Graph
    // resolution below remains separate because stars acquire names only in an entry/root context.
    let interface = engine.interface(file);

    let mut diagnostics = parsed
        .module
        .diagnostics
        .clone()
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

    if diagnostics.is_empty()
        && matches!(
            parsed.module.source_kind,
            kiln_hir::SourceKind::TypeScript | kiln_hir::SourceKind::Tsx
        )
    {
        let checked = engine.source_check(file);
        diagnostics.extend(checked.diagnostics.into_iter().map(|d| Diagnostic {
            path: d.path,
            message: match d.code {
                Some(code) => format!("TS{code}: {}", d.message),
                None => d.message,
            },
            start: d.span.map(|s| s.start),
            end: d.span.map(|s| s.end),
        }));
    }
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
    prepare_run_with_launch_options(
        engine,
        cache,
        path,
        args,
        project_root,
        resolutions,
        node,
        Vec::new(),
        Vec::new(),
        false,
    )
}

pub(crate) fn prepare_run_with_launch_options<E: QueryEngine>(
    engine: &mut E,
    cache: Option<&mut GraphCache>,
    path: String,
    args: Vec<String>,
    project_root: String,
    resolutions: Option<&kiln_proto::ResolutionMap>,
    node: bool,
    preloads: Vec<String>,
    exec_argv: Vec<String>,
    disallow_string_code_generation: bool,
) -> Result<PreparedRun, String> {
    let overlay = build_vfs_overlay(engine);
    let entry = resolve_source_path(&path, &project_root);
    let prepared = if let Some(entry) = entry {
        let file = ensure_canonical_source(engine, &path, &entry, &project_root)?;
        let preload_paths = preloads
            .iter()
            .map(|preload| {
                let resolved = resolve_source_path(preload, &project_root).ok_or_else(|| {
                    format!("cannot resolve source preload `{preload}` from `{project_root}`")
                })?;
                ensure_canonical_source(engine, preload, &resolved, &project_root)?;
                Ok::<_, String>(resolved)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_preloads = !preload_paths.is_empty();
        PreparedRun::Graph {
            graph: graph_snapshot_with_preloads(
                engine,
                cache,
                &entry,
                &project_root,
                resolutions,
                node,
                &preload_paths,
            ),
            path,
            args,
            exec_argv,
            disallow_string_code_generation,
            has_preloads,
            file,
            revision: engine.revision().0,
            overlay,
        }
    } else {
        if !preloads.is_empty() {
            return Err("source preloads require a module-graph entry".to_owned());
        }
        let file = ensure_source(engine, &path)?;
        PreparedRun::Program {
            program: engine.ir(file),
            path,
            args,
            exec_argv,
            disallow_string_code_generation,
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
    let active_sources = engine
        .source_files()
        .into_iter()
        .map(|(source_file, source_path)| {
            let revision = engine.source_revision(source_file).ok_or_else(|| {
                format!("query engine listed {source_path} without a source revision")
            })?;
            Ok((source_file, source_path, revision))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let source_identity_inputs: Vec<_> = active_sources
        .iter()
        .filter(|(_, path, _)| is_source_check_input_path(Path::new(path)))
        .map(|(_, path, revision)| (path.as_str(), *revision))
        .collect();
    let mut key = GraphCacheKey::new(
        canonical_root.clone(),
        canonical_entry.clone(),
        resolutions,
        node,
    );
    key.source_check_identity = source_check_identity_for_sources(&source_identity_inputs);
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
    let source_types = key.source_check_identity.clone().map(|identity| {
        cache.source_check_slot(key.project_root.clone(), key.entry.clone(), identity)
    });

    let mut sources = BTreeMap::new();
    let mut revisions = Vec::new();
    for (source_file, source_path, revision) in active_sources {
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
            source_types: source_types.unwrap_or_else(|| Arc::new(OnceLock::new())),
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

fn is_typescript_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("ts" | "tsx" | "mts" | "cts")
    )
}

fn is_source_code_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts")
    )
}

fn is_checker_auxiliary_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("json" | "jsonc")
    )
}

fn is_source_check_input_path(path: &Path) -> bool {
    is_source_code_path(path) || is_checker_auxiliary_path(path)
}

fn snapshot_source_types(snapshot: &OwnedGraphSnapshot) -> Option<&SnapshotSourceTypes> {
    if !snapshot.sources.keys().any(|path| is_typescript_path(path))
        || snapshot.entry.to_str().is_none()
    {
        return None;
    }
    Some(snapshot.source_types.get_or_init(|| {
        let modules: BTreeMap<_, _> = snapshot
            .sources
            .iter()
            .filter(|(path, _)| is_source_code_path(path))
            .filter_map(|(path, source)| {
                let path_string = path.to_str()?;
                Some((
                    path.clone(),
                    Arc::new(kiln_frontend_oxc::parse(path_string, &source.text)),
                ))
            })
            .collect();
        let check_input: Vec<_> = modules
            .iter()
            .filter_map(|(path, module)| Some((path.to_str()?.to_owned(), Arc::clone(module))))
            .collect();
        let auxiliary: Vec<_> = snapshot
            .sources
            .iter()
            .filter(|(path, _)| is_checker_auxiliary_path(path))
            .filter_map(|(path, source)| {
                Some((
                    path.to_str()?.to_owned(),
                    Arc::<[u8]>::from(source.text.as_bytes()),
                ))
            })
            .collect();
        let checked = crate::server::check_source_types(
            snapshot.entry.to_str().expect("checked above"),
            &check_input,
            &auxiliary,
        );
        SnapshotSourceTypes { modules, checked }
    }))
}

/// Type checking receives the project's TS source set, including ambient declarations which may
/// not be runtime graph edges. Put that set and checker version into an explicit cache-key field
/// so additions, removals, and edits cannot reuse stale source contracts. JS-only projects keep
/// `None` and never pay for checking beyond extension checks.
fn source_check_identity<E: QueryEngine>(engine: &E) -> Option<kiln_digest::Digest> {
    let all_sources = engine.source_files();
    if !all_sources
        .iter()
        .any(|(_, path)| is_typescript_path(Path::new(path)))
    {
        return None;
    }
    let sources: Vec<_> = all_sources
        .iter()
        .filter(|(_, path)| is_source_check_input_path(Path::new(path)))
        .filter_map(|(file, path)| {
            engine
                .source_revision(*file)
                .map(|revision| (path.as_str(), revision))
        })
        .collect();
    source_check_identity_for_sources(&sources)
}

fn source_check_identity_for_sources(sources: &[(&str, Revision)]) -> Option<kiln_digest::Digest> {
    if !sources
        .iter()
        .any(|(path, _)| is_typescript_path(Path::new(path)))
    {
        return None;
    }
    let mut sources = sources.to_vec();
    if sources.is_empty() {
        return None;
    }
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    let mut identity = b"kiln checker source set v1\0".to_vec();
    identity.extend_from_slice(kiln_type_checker::CHECKER_VERSION.as_bytes());
    identity.push(0);
    for (path, revision) in sources {
        identity.extend_from_slice(&(path.len() as u64).to_le_bytes());
        identity.extend_from_slice(path.as_bytes());
        identity.extend_from_slice(&revision.0.to_le_bytes());
    }
    Some(kiln_digest::Digest::hash(&identity))
}

pub(crate) fn build_graph_from_snapshot(
    snapshot: &OwnedGraphSnapshot,
) -> Result<ModuleGraph, kiln_module_graph::ModuleGraphError> {
    // Nothing writes the project while its graph is built.
    let _filesystem = kiln_module_resolver::FilesystemSnapshot::begin();
    let declared_includes = declared_include_sources(snapshot.resolutions.as_ref())?;
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
    let runtime_package_aliases = snapshot
        .resolutions
        .iter()
        .flat_map(|map| &map.runtime_package_aliases)
        .map(|alias| {
            (
                PathBuf::from(&alias.package_root),
                alias.specifier.clone(),
                PathBuf::from(&alias.target),
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
    })?
    .with_runtime_package_aliases(runtime_package_aliases);
    ModuleGraph::build_with_typed_policy_includes(
        &snapshot.entry,
        resolver,
        |path, source_kind| snapshot_module_payload(snapshot, path, source_kind),
        declared_includes,
    )
}

fn declared_include_sources(
    resolutions: Option<&ResolutionMap>,
) -> Result<Vec<(PathBuf, kiln_module_graph::ModuleSource)>, kiln_module_graph::ModuleGraphError> {
    let Some(resolutions) = resolutions else {
        return Ok(Vec::new());
    };
    resolutions
        .declared_includes
        .iter()
        .map(|include| {
            let path = PathBuf::from(&include.target);
            let source = if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(kiln_data_formats::is_data_extension)
            {
                kiln_module_graph::ModuleSource::Json
            } else {
                match resolutions
                    .node_formats
                    .iter()
                    .find(|format| format.path == include.target)
                    .map(|format| format.format)
                {
                    Some(kiln_proto::NodeModuleFormat::CommonJs) => {
                        kiln_module_graph::ModuleSource::CommonJs
                    }
                    Some(kiln_proto::NodeModuleFormat::EsModule) => {
                        kiln_module_graph::ModuleSource::Source
                    }
                    None => {
                        return Err(kiln_module_graph::ModuleGraphError::Source {
                            path,
                            message: format!(
                                "declared include {:?} has no pinned Node source format",
                                include.specifier
                            ),
                        });
                    }
                }
            };
            Ok((path, source))
        })
        .collect()
}

/// Patch one ordinary ESM module in an old immutable graph. Returning `None` asks the caller to
/// fall back to a complete rebuild (CommonJS, JSON, builtins, and topology changes need that).
pub(crate) fn patch_graph_from_snapshot(
    snapshot: &OwnedGraphSnapshot,
    old: &ModuleGraph,
    changed: GraphSourceRevision,
) -> Result<Option<ModuleGraph>, kiln_module_graph::ModuleGraphError> {
    if graph_uses_typescript(old) {
        // A changed TS source can alter contracts attached to other modules, including through
        // type-only or ambient dependencies that have no runtime edge to patch in isolation.
        return Ok(None);
    }
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
    let source_types = if is_typescript_path(path) {
        snapshot_source_types(snapshot)
    } else {
        None
    };
    if source_kind == kiln_module_graph::ModuleSource::CommonJs {
        let (mut hir, static_requires) =
            kiln_frontend_oxc::parse_commonjs_with_requires(path_string, &source.text);
        hir.module_id =
            kiln_module_graph::graph_module_identity(&snapshot.project_root, path).into();
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
        if is_typescript_path(path)
            && let Some(source_types) = source_types
        {
            kiln_ir::apply_source_types(
                path_string,
                &hir,
                &source_types.checked,
                &source_types.checked.checker_identity,
                &mut program.program,
            );
        }
        return Ok(kiln_module_graph::ModulePayload::Program(program));
    }
    let mut hir = source_types
        .and_then(|source_types| source_types.modules.get(path))
        .map(|module| module.as_ref().clone())
        .unwrap_or_else(|| kiln_frontend_oxc::parse(path_string, &source.text));
    hir.module_id = kiln_module_graph::graph_module_identity(&snapshot.project_root, path).into();
    let mut program = kiln_ir::compile_module(&hir).map_err(|diagnostics| {
        kiln_module_graph::ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        }
    })?;
    if is_typescript_path(path)
        && let Some(source_types) = source_types
    {
        kiln_ir::apply_source_types(
            path_string,
            &hir,
            &source_types.checked,
            &source_types.checked.checker_identity,
            &mut program.program,
        );
    }
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
    if graph_uses_typescript(graph) {
        // Checker facts can depend on ambient and declaration files which have no runtime edge.
        // Retain the complete captured source set for TS graphs; JS-only graphs remain narrow.
        return Some(snapshot.revisions.clone());
    }
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

fn graph_uses_typescript(graph: &ModuleGraph) -> bool {
    graph
        .module_order()
        .iter()
        .any(|path| is_typescript_path(path))
}

/// Execute an interpreted run after preparation has released the query-engine mutex.
pub(crate) fn execute_prepared_run(
    prepared: PreparedRun,
    node: bool,
    wall_clock: bool,
    steps: StepBudget,
) -> Response {
    #[cfg(test)]
    wait_for_run_gate(prepared.path());

    let limits = execution_limits(steps);
    // The program is a process of its own for signals and `process.pid`, never this one
    // (realms issue 26; `kiln_runtime::signals`). Its subscriptions end with it.
    let hosted = kiln_runtime::signals::host_program();
    let mut response = match prepared {
        PreparedRun::Graph {
            graph,
            path,
            args,
            exec_argv,
            disallow_string_code_generation,
            has_preloads,
            file,
            revision,
            overlay,
        } => execute_graph(
            graph,
            path,
            args,
            exec_argv,
            disallow_string_code_generation,
            has_preloads,
            file,
            revision,
            node,
            wall_clock,
            limits,
            overlay,
        ),
        PreparedRun::Program {
            program,
            path,
            args,
            exec_argv,
            disallow_string_code_generation,
            file,
            revision,
            overlay,
        } => execute_program(
            program,
            path,
            args,
            exec_argv,
            disallow_string_code_generation,
            file,
            revision,
            node,
            wall_clock,
            limits,
            overlay,
        ),
    };
    if let (Some(signal), Response::Ran(summary)) = (hosted.terminated_by(), &mut response) {
        summary.signal = Some(signal);
        summary.exit_code = Some(128 + signal);
        summary.succeeded = false;
    }
    drop(hosted);
    response
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
    wall_clock: bool,
    steps: StepBudget,
) -> Response {
    match prepare_run(engine, resolutions, path, args, project_root, node) {
        Ok(prepared) => execute_prepared_run(prepared, node, wall_clock, steps),
        Err(message) => Response::Error { message },
    }
}

#[allow(clippy::too_many_arguments)]
fn node_run<E: QueryEngine>(
    engine: &mut E,
    resolutions: Option<&ResolutionMap>,
    path: String,
    project_root: String,
    args: Vec<String>,
    preloads: Vec<String>,
    exec_argv: Vec<String>,
    disallow_string_code_generation: bool,
    wall_clock: bool,
    steps: StepBudget,
) -> Response {
    match prepare_run_with_launch_options(
        engine,
        None,
        path,
        args,
        project_root,
        resolutions,
        true,
        preloads,
        exec_argv,
        disallow_string_code_generation,
    ) {
        Ok(prepared) => execute_prepared_run(prepared, true, wall_clock, steps),
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

/// A capture host that delivers its output as it is written when the request asked for that.
fn streaming_host(host: kiln_runtime::CaptureHost) -> kiln_runtime::CaptureHost {
    match crate::stream::current() {
        Some(sink) => host.with_stream(Box::new(move |is_stderr, text| {
            let stream = if is_stderr {
                kiln_proto::OutputStream::Stderr
            } else {
                kiln_proto::OutputStream::Stdout
            };
            sink.emit(stream, text.to_owned());
        })),
        None => host,
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_graph(
    graph: Result<Arc<ModuleGraph>, kiln_module_graph::ModuleGraphError>,
    path: String,
    args: Vec<String>,
    exec_argv: Vec<String>,
    disallow_string_code_generation: bool,
    has_preloads: bool,
    file: kiln_db::FileId,
    revision: u64,
    node: bool,
    wall_clock: bool,
    limits: kiln_interpreter::ExecutionLimits,
    overlay: std::collections::HashMap<std::path::PathBuf, Vec<u8>>,
) -> Response {
    let capabilities = kiln_conformance::source_compiler_capability()
        .with_argv(interpreted_argv(&path, &args))
        .with_exec_argv(exec_argv)
        .with_string_code_generation(!disallow_string_code_generation);
    let mut host = streaming_host(
        kiln_runtime::CaptureHost::new()
            .with_fs(Box::new(kiln_host_abi::OverlayFileSystem::new(
                kiln_host_abi::OsFileSystem,
                overlay,
            )))
            .with_sync_process(node)
            .with_clock(if wall_clock {
                kiln_runtime::ClockCapability::Host
            } else {
                kiln_runtime::ClockCapability::Virtual
            }),
    );
    // One shaken graph feeds execution: reduce to the closed-world live
    // subset first, so dead modules never evaluate. Development selection
    // keeps the interpreter conservative; a reduction failure runs the full
    // graph rather than refusing a program liveness could not classify.
    let graph = graph.map(|graph| {
        if has_preloads {
            return graph;
        }
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
            graph.execute_node_with_clock(
                &mut host,
                limits,
                capabilities,
                if wall_clock {
                    kiln_runtime::ClockCapability::Host
                } else {
                    kiln_runtime::ClockCapability::Virtual
                },
            )
        }
        Ok(graph) => {
            let capabilities = capabilities.with_module_loader_service(std::sync::Arc::new(
                kiln_module_eval::GraphModuleLoader::new(std::sync::Arc::clone(&graph)),
            ));
            graph.execute_with_clock(
                &mut host,
                limits,
                capabilities,
                if wall_clock {
                    kiln_runtime::ClockCapability::Host
                } else {
                    kiln_runtime::ClockCapability::Virtual
                },
            )
        }
        Err(error) => Err(error.into()),
    };
    match result {
        Ok(execution) => {
            let scalar_value = scalar_value_summary(&execution.value);
            let exit_code = execution.process_exit_code.unwrap_or(0);
            Response::Ran(RunSummary {
                revision,
                path,
                file: FileId(file.0),
                succeeded: exit_code == 0,
                exit_code: Some(exit_code),
                signal: None,
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
            signal: None,
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
            signal: None,
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
    preloads: &[PathBuf],
) -> Result<ModuleGraph, kiln_module_graph::ModuleGraphError> {
    // Nothing writes the project while its graph is built.
    let _filesystem = kiln_module_resolver::FilesystemSnapshot::begin();
    let declared_includes = declared_include_sources(resolutions)?;
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
    let formats = node_formats(resolutions);
    let runtime_package_aliases = resolutions
        .into_iter()
        .flat_map(|map| &map.runtime_package_aliases)
        .map(|alias| {
            (
                PathBuf::from(&alias.package_root),
                alias.specifier.clone(),
                PathBuf::from(&alias.target),
            )
        })
        .collect::<Vec<_>>();
    let mut prepared = prepare_payloads(engine, project_root, &formats);
    let provider = |module_path: &Path, source_kind: kiln_module_graph::ModuleSource| {
        if source_kind == kiln_module_graph::ModuleSource::CommonJs
            && let Some(payload) = prepared.remove(module_path)
        {
            return payload;
        }
        module_payload(engine, module_path, project_root, source_kind)
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
    })?
    .with_runtime_package_aliases(runtime_package_aliases);
    if !preloads.is_empty() {
        ModuleGraph::build_with_typed_policy_preloads_and_includes(
            entry,
            resolver,
            provider,
            preloads.to_vec(),
            declared_includes,
        )
    } else {
        ModuleGraph::build_with_typed_policy_includes(entry, resolver, provider, declared_includes)
    }
}

/// A CommonJS module's payload: a function of its path, text and graph identity alone, so it is
/// kept by path and identity beside the text it was compiled from and reused while that text is
/// unchanged. A package's CommonJS does not change between checks, and recompiling all of it made
/// every graph rebuild pay for the whole dependency tree again.
fn commonjs_payload(
    module_path: &Path,
    module_path_string: &str,
    source: &str,
    identity: &str,
) -> Result<kiln_module_graph::ModulePayload, kiln_module_graph::ModuleGraphError> {
    type Compiled = Arc<(String, kiln_module_graph::ModulePayload)>;
    static COMPILED: OnceLock<Mutex<HashMap<(String, String), Compiled>>> = OnceLock::new();
    if !crate::server::serving() {
        return compile_commonjs_payload(module_path, module_path_string, source, identity);
    }
    let compiled = COMPILED.get_or_init(Default::default);
    let key = (module_path_string.to_owned(), identity.to_owned());
    let held = compiled
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned();
    if let Some(held) = held
        && held.0 == source
    {
        return Ok(held.1.clone());
    }
    let payload = compile_commonjs_payload(module_path, module_path_string, source, identity)?;
    compiled
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, Arc::new((source.to_owned(), payload.clone())));
    Ok(payload)
}

fn compile_commonjs_payload(
    module_path: &Path,
    module_path_string: &str,
    source: &str,
    identity: &str,
) -> Result<kiln_module_graph::ModulePayload, kiln_module_graph::ModuleGraphError> {
    let (mut hir, static_requires) =
        kiln_frontend_oxc::parse_commonjs_with_requires(module_path_string, source);
    hir.module_id = identity.into();
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
    Ok(kiln_module_graph::ModulePayload::Program(program))
}

type PreparedPayload =
    Result<kiln_module_graph::ModulePayload, kiln_module_graph::ModuleGraphError>;

/// Compute ahead, on several threads, the payloads a graph walk will ask for one module at a
/// time: the IR of each synchronized source module, through the engine, and the program of each
/// CommonJS one, returned by path. Each module is classified as the pinned resolver classifies it
/// -- by its extension, then by its pinned Node format. A module the walk asks for as something
/// else, or does not reach, is computed for nothing; one it reaches that this missed is computed
/// when asked, as before.
/// The Node module formats the client pinned.
fn node_formats(
    resolutions: Option<&ResolutionMap>,
) -> Vec<(PathBuf, kiln_module_resolver::NodeModuleFormat)> {
    resolutions
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
        .collect()
}

/// The graph modules a walk would compile as ES modules and those it would load as CommonJS, as
/// [`prepare_payloads`] classifies them.
type PayloadModules = (
    Vec<(kiln_db::FileId, String)>,
    Vec<(PathBuf, String, String, String)>,
);

fn payload_modules<E: QueryEngine>(
    engine: &mut E,
    project_root: &str,
    formats: &[(PathBuf, kiln_module_resolver::NodeModuleFormat)],
) -> PayloadModules {
    let formats: HashMap<&Path, kiln_module_resolver::NodeModuleFormat> = formats
        .iter()
        .map(|(path, format)| (path.as_path(), *format))
        .collect();
    let (mut modules, mut commonjs) = (Vec::new(), Vec::new());
    for (file, path) in engine.source_files() {
        let module_path = Path::new(&path);
        let is_commonjs = match module_path.extension().and_then(|e| e.to_str()) {
            Some("mjs" | "mts") => false,
            Some("cjs" | "cts") => true,
            Some("js" | "jsx" | "ts" | "tsx") => {
                formats.get(module_path) == Some(&kiln_module_resolver::NodeModuleFormat::CommonJs)
            }
            _ => continue,
        };
        let identity =
            kiln_module_graph::graph_module_identity(Path::new(project_root), module_path);
        if is_commonjs {
            if let Some(source) = engine.source(file) {
                commonjs.push((
                    PathBuf::from(&path),
                    path.clone(),
                    source.to_owned(),
                    identity,
                ));
            }
        } else {
            modules.push((file, identity));
        }
    }
    (modules, commonjs)
}

/// Compile, on several threads, the ES modules among the project's JavaScript, whose programs
/// do not wait for type facts: meant to run while a source check does.
fn prepare_javascript_irs<E: QueryEngine>(
    engine: &mut E,
    project_root: &str,
    resolutions: Option<&ResolutionMap>,
) {
    let formats = node_formats(resolutions);
    let paths: HashMap<kiln_db::FileId, String> = engine.source_files().into_iter().collect();
    let (modules, _) = payload_modules(engine, project_root, &formats);
    let javascript: Vec<(kiln_db::FileId, String)> = modules
        .into_iter()
        .filter(|(file, _)| {
            paths.get(file).is_some_and(|path| {
                [".js", ".mjs", ".jsx"]
                    .iter()
                    .any(|extension| path.ends_with(extension))
            })
        })
        .collect();
    engine.prepare_module_irs(&javascript);
}

fn prepare_payloads<E: QueryEngine>(
    engine: &mut E,
    project_root: &str,
    formats: &[(PathBuf, kiln_module_resolver::NodeModuleFormat)],
) -> HashMap<PathBuf, PreparedPayload> {
    let (modules, commonjs) = payload_modules(engine, project_root, formats);
    engine.prepare_module_irs(&modules);
    let prepare = |(module_path, path, source, identity): &(PathBuf, String, String, String)| {
        // A panic is left for the walk's own request to report.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            commonjs_payload(module_path, path, source, identity)
        }))
        .ok()
    };
    let prepared: Vec<Option<PreparedPayload>> = match commonjs_pool() {
        Some(pool) if commonjs.len() > 1 => {
            pool.install(|| commonjs.par_iter().map(prepare).collect())
        }
        _ => commonjs.iter().map(prepare).collect(),
    };
    commonjs
        .into_iter()
        .zip(prepared)
        .filter_map(|((module_path, ..), payload)| Some((module_path, payload?)))
        .collect()
}

/// The pool that compiles CommonJS payloads ahead of a graph walk: the available CPUs, at most
/// eight, bounded by any positive `KILN_JOBS` or `RAYON_NUM_THREADS`, with the stack of a request
/// thread, where these payloads are otherwise compiled.
fn commonjs_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let stack = Some(crate::SERVE_STACK_BYTES);
        kiln_jobs::pool("kiln-commonjs", kiln_jobs::budget(8), stack)
    })
    .as_ref()
}

fn module_payload<E: QueryEngine>(
    engine: &mut E,
    module_path: &Path,
    project_root: &str,
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
    let identity = kiln_module_graph::graph_module_identity(Path::new(project_root), module_path);
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
        return commonjs_payload(module_path, module_path_string, source, &identity);
    }
    engine
        .module_ir_with_identity(module_file, &identity)
        .map(kiln_module_graph::ModulePayload::Program)
        .map_err(|diagnostics| kiln_module_graph::ModuleGraphError::Compile {
            path: module_path.to_path_buf(),
            diagnostics,
        })
}

#[cfg(test)]
mod module_identity_tests {
    use super::*;
    use kiln_db::{MemoEngine, QueryEngine};
    use kiln_module_graph::{ModulePayload, ModuleSource};

    fn private_constants(payload: ModulePayload) -> Vec<String> {
        let ModulePayload::Program(program) = payload else {
            panic!("source module compiles to a program");
        };
        program
            .program
            .constants
            .into_iter()
            .filter_map(|constant| match constant {
                kiln_ir::Constant::String(value) => value
                    .to_scalar_string()
                    .ok()
                    .filter(|value| value.starts_with("@@private:")),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn memoized_module_ir_tracks_graph_identity_changes_for_same_file() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kiln-private-module-identity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let project = dir.join("project");
        let source_dir = project.join("src");
        std::fs::create_dir_all(&source_dir).expect("create project source directory");
        // Canonical, like every path the resolver hands the daemon: `graph_module_identity`
        // strips the *canonical* project root, and on macOS `temp_dir()` is `/var/folders/...`
        // while its canonical form is `/private/var/...`, so an uncanonicalized fixture path
        // never has the root as a prefix and both identities collapse to the absolute path.
        let dir = std::fs::canonicalize(&dir).expect("canonicalize temporary project");
        let project = dir.join("project");
        let source_dir = project.join("src");
        let path = source_dir.join("private.ts");
        let path_string = path.to_str().expect("UTF-8 test path");
        let source = "class Cache { #value = 1; read() { return this.#value; } }";
        let mut engine = MemoEngine::new(kiln_frontend_oxc::parse);
        let file = engine.intern(path_string);
        engine.set_source(file, source.to_owned());

        let project_payload = module_payload(
            &mut engine,
            &path,
            project.to_str().expect("UTF-8 project path"),
            ModuleSource::Source,
        )
        .expect("compile under project identity");
        let project_keys = private_constants(project_payload);
        assert!(!project_keys.is_empty(), "fixture contains a private name");

        let cached_payload = module_payload(
            &mut engine,
            &path,
            project.to_str().expect("UTF-8 project path"),
            ModuleSource::Source,
        )
        .expect("reuse cached graph identity");
        assert_eq!(project_keys, private_constants(cached_payload));

        let parent = dir.to_str().expect("UTF-8 parent path");
        let parent_payload = module_payload(&mut engine, &path, parent, ModuleSource::Source)
            .expect("compile under changed graph identity");
        let parent_keys = private_constants(parent_payload);
        assert_ne!(project_keys, parent_keys);
        assert_eq!(engine.stats().ir_computed, 2);

        std::fs::remove_dir_all(dir).expect("remove temporary project");
    }
}

fn graph_sources<E: QueryEngine>(
    engine: &mut E,
    graph: &ModuleGraph,
) -> Option<Vec<GraphSourceRevision>> {
    if graph_uses_typescript(graph) {
        return engine
            .source_files()
            .into_iter()
            .map(|(file, _)| {
                Some(GraphSourceRevision {
                    file,
                    revision: engine.source_revision(file)?,
                })
            })
            .collect();
    }
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
    graph_snapshot_with_preloads(engine, cache, entry, project_root, resolutions, node, &[])
}

fn graph_snapshot_with_preloads<E: QueryEngine>(
    engine: &mut E,
    cache: Option<&mut GraphCache>,
    entry: &Path,
    project_root: &str,
    resolutions: Option<&kiln_proto::ResolutionMap>,
    node: bool,
    preloads: &[PathBuf],
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
    let canonical_preloads = preloads
        .iter()
        .map(|path| {
            std::fs::canonicalize(path).map_err(|error| {
                kiln_module_graph::ModuleGraphError::Entry {
                    path: path.clone(),
                    kind: error.kind(),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut key = GraphCacheKey::new_with_startup_roots(
        canonical_root,
        canonical_entry.clone(),
        resolutions,
        node,
        canonical_preloads,
    );
    key.source_check_identity = source_check_identity(engine);
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
                if !graph_uses_typescript(&graph)
                    && let Some(path) = path
                    && graph.module(&path).is_some_and(|module| {
                        module.source == kiln_module_graph::ModuleSource::Source
                    })
                {
                    let payload = match module_payload(
                        engine,
                        &path,
                        project_root,
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
        let graph = build_graph(
            engine,
            resolutions,
            &canonical_entry,
            project_root,
            node,
            &key.startup_roots,
        )?;
        return Ok(match graph_sources(engine, &graph) {
            Some(sources) => cache.insert(key, sources, graph),
            // Graph construction rejects non-UTF-8 and unsynchronized modules, so this is a
            // defensive fallback for an external QueryEngine that violates that contract.
            None => Arc::new(graph),
        });
    }
    build_graph(
        engine,
        resolutions,
        &canonical_entry,
        project_root,
        node,
        &key.startup_roots,
    )
    .map(Arc::new)
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
    exec_argv: Vec<String>,
    disallow_string_code_generation: bool,
    file: kiln_db::FileId,
    revision: u64,
    node: bool,
    wall_clock: bool,
    limits: kiln_interpreter::ExecutionLimits,
    overlay: std::collections::HashMap<std::path::PathBuf, Vec<u8>>,
) -> Response {
    let capabilities = kiln_conformance::source_compiler_capability()
        .with_argv(interpreted_argv(&path, &args))
        .with_exec_argv(exec_argv)
        .with_string_code_generation(!disallow_string_code_generation)
        .with_realm_bootstrap(kiln_module_eval::realm_bootstrap(node));
    let program = match program {
        Ok(program) => program,
        Err(diagnostics) => {
            return Response::Ran(RunSummary {
                revision,
                path,
                file: FileId(file.0),
                succeeded: false,
                exit_code: Some(1),
                signal: None,
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

    let mut host = streaming_host(
        kiln_runtime::CaptureHost::new()
            .with_fs(Box::new(kiln_host_abi::OverlayFileSystem::new(
                kiln_host_abi::OsFileSystem,
                overlay,
            )))
            .with_sync_process(node)
            .with_clock(if wall_clock {
                kiln_runtime::ClockCapability::Host
            } else {
                kiln_runtime::ClockCapability::Virtual
            }),
    );
    let result = if node {
        if wall_clock {
            let strictness = std::rc::Rc::new(kiln_ir::Strictness::sloppy(&program));
            kiln_interpreter::execute_node_with_clock_and_strictness(
                std::rc::Rc::new(program),
                strictness,
                &mut host,
                limits,
                capabilities,
                kiln_runtime::ClockCapability::Host,
            )
        } else {
            kiln_interpreter::execute_node(
                std::rc::Rc::new(program),
                &mut host,
                limits,
                capabilities,
            )
        }
    } else if wall_clock {
        let strictness = std::rc::Rc::new(kiln_ir::Strictness::sloppy(&program));
        kiln_interpreter::execute_with_clock_and_strictness(
            std::rc::Rc::new(program),
            strictness,
            &mut host,
            limits,
            capabilities,
            kiln_runtime::ClockCapability::Host,
        )
    } else {
        kiln_interpreter::execute(std::rc::Rc::new(program), &mut host, limits, capabilities)
    };
    match result {
        Ok(execution) => {
            let scalar_value = scalar_value_summary(&execution.value);
            let exit_code = execution.process_exit_code.unwrap_or(0);
            Response::Ran(RunSummary {
                revision,
                path,
                file: FileId(file.0),
                succeeded: exit_code == 0,
                exit_code: Some(exit_code),
                signal: None,
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
            signal: None,
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
            signal: None,
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
    no_kiln_extensions: bool,
    workers: Option<RecordedWorkers>,
    static_link: bool,
    pgo_generate: Option<PathBuf>,
    pgo_use: Option<PathBuf>,
) -> Response {
    // The worker supervisor lives in the graph launcher; the single-program launcher runs one
    // realm and has nowhere to record a count, so the flag is refused rather than dropped.
    if workers.is_some() {
        return Response::Error {
            message: format!(
                "--workers needs a module-graph build, and {path} did not resolve to a project source file"
            ),
        };
    }
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
            realm: kiln_native::NativeRealmPolicy::host_clock()
                .without_kiln_extensions(no_kiln_extensions),
            march_native: false,
            static_link,
            pgo_generate,
            pgo_use,
            node: false,
        },
    ) {
        Ok(report) => {
            let explanation = if explain {
                // The parallel section first: `--explain-parallel` keeps it and drops the rest.
                let mut text =
                    kiln_backend_c::parallel::format_explain_section(&report.parallel_sites, &path);
                text.push_str(
                    &kiln_backend_c::facts::analyze_program(&program, Some(&path)).format_explain(),
                );
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

/// `kiln check --explain-parallel` for a module graph: the same selection, reduction and
/// emission `native_build_graph` runs for this profile, stopped before the C compiler, and the
/// same `parallel:` section `--explain` renders from it.
pub(crate) fn native_explain_parallel_graph(
    graph: &ModuleGraph,
    path: &str,
    project_root: &str,
    profile: NativeProfile,
    node: bool,
) -> Response {
    let options = kiln_native::NativeBuildOptions {
        profile: native_profile(profile),
        realm: kiln_native::NativeRealmPolicy::host_clock(),
        node,
        ..kiln_native::NativeBuildOptions::default()
    };
    match discover_native_builder(Path::new(project_root)).and_then(|builder| {
        builder
            .parallel_sites_graph(graph, &options)
            .map_err(|e| e.to_string())
    }) {
        Ok(sites) => Response::ParallelExplained {
            text: kiln_backend_c::parallel::format_explain_section(&sites, path),
        },
        Err(message) => Response::Error { message },
    }
}

/// [`native_explain_parallel_graph`] for an entry the daemon builds as a single program.
pub(crate) fn native_explain_parallel_program(
    program: &kiln_ir::Program,
    path: &str,
    project_root: &str,
    profile: NativeProfile,
) -> Response {
    let options = kiln_native::NativeBuildOptions {
        profile: native_profile(profile),
        realm: kiln_native::NativeRealmPolicy::host_clock(),
        ..kiln_native::NativeBuildOptions::default()
    };
    match discover_native_builder(Path::new(project_root)).and_then(|builder| {
        builder
            .parallel_sites(program, &options)
            .map_err(|e| e.to_string())
    }) {
        Ok(sites) => Response::ParallelExplained {
            text: kiln_backend_c::parallel::format_explain_section(&sites, path),
        },
        Err(message) => Response::Error { message },
    }
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
    no_kiln_extensions: bool,
    workers: Option<RecordedWorkers>,
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
            realm: kiln_native::NativeRealmPolicy::host_clock()
                .without_kiln_extensions(no_kiln_extensions)
                .with_workers(workers.map(native_workers)),
            march_native: false,
            static_link,
            pgo_generate,
            pgo_use,
            node,
        },
    ) {
        Ok(report) => {
            let explanation = if explain {
                // The parallel section first: `--explain-parallel` keeps it and drops the rest.
                let mut text =
                    kiln_backend_c::parallel::format_explain_section(&report.parallel_sites, &path);
                text.push_str(&kiln_backend_c::facts::analyze_graph(graph).format_explain());
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

/// Run a native build-and-launch, passing it a sink for the child's output when the request
/// asked to stream, and flushing any incomplete UTF-8 tail once the child is gone.
fn run_streaming_native(
    run: impl FnOnce(
        Option<&mut dyn FnMut(kiln_native::ChildStream, &[u8])>,
        bool,
    ) -> Result<kiln_native::NativeRunReport, kiln_native::NativeError>,
) -> Result<kiln_native::NativeRunReport, kiln_native::NativeError> {
    let inherit = crate::stream::inherit_native_output();
    if inherit {
        return run(None, true);
    }
    let Some(sink) = crate::stream::current() else {
        return run(None, false);
    };
    let mut chunker = crate::stream::Utf8Chunker::default();
    let wire = |stream: kiln_native::ChildStream| match stream {
        kiln_native::ChildStream::Stdout => kiln_proto::OutputStream::Stdout,
        kiln_native::ChildStream::Stderr => kiln_proto::OutputStream::Stderr,
    };
    let outcome = run(
        Some(&mut |stream, bytes| {
            let stream = wire(stream);
            sink.emit(stream, chunker.push(stream, bytes));
        }),
        false,
    );
    for (stream, text) in chunker.finish() {
        sink.emit(stream, text);
    }
    outcome
}

pub(crate) fn native_run_program(
    program: kiln_ir::Program,
    path: String,
    project_root: String,
    profile: NativeProfile,
    wall_clock: bool,
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
    let realm = if wall_clock {
        kiln_native::NativeRealmPolicy::host_clock()
    } else {
        kiln_native::NativeRealmPolicy::default()
    };
    let result = run_streaming_native(|sink, inherit| {
        if inherit {
            builder.run_inheriting_output(
                &program,
                &kiln_native::NativeBuildOptions {
                    output,
                    profile: native_profile(profile),
                    realm: realm.clone(),
                    march_native: false,
                    static_link,
                    pgo_generate,
                    pgo_use,
                    node: false,
                },
            )
        } else {
            builder.run_streaming(
                &program,
                &kiln_native::NativeBuildOptions {
                    output,
                    profile: native_profile(profile),
                    realm,
                    march_native: false,
                    static_link,
                    pgo_generate,
                    pgo_use,
                    node: false,
                },
                sink,
            )
        }
    });
    match result {
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
    wall_clock: bool,
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
    let result = run_streaming_native(|sink, inherit| {
        let options = kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
            // A directly-run native Node program has the same host-backed clock policy as a
            // node-enabled launcher built for later execution. Keep plain native scripts on
            // the deterministic clock.
            realm: if wall_clock {
                kiln_native::NativeRealmPolicy::host_clock()
            } else {
                kiln_native::NativeRealmPolicy::default()
            },
            march_native: false,
            static_link,
            pgo_generate,
            pgo_use,
            node,
        };
        if inherit {
            builder.run_graph_inheriting_output(graph, &options)
        } else {
            builder.run_graph_streaming(graph, &options, sink)
        }
    });
    match result {
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

fn native_workers(workers: RecordedWorkers) -> kiln_native::NativeWorkers {
    match workers {
        RecordedWorkers::Auto => kiln_native::NativeWorkers::Auto,
        RecordedWorkers::Count(count) => kiln_native::NativeWorkers::Count(count),
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

    // Canonicalizing is a `readlink` per path component, and every active source was examined on
    // every request. A path that names an existing file other than the entry's cannot canonicalize
    // to the entry, so one `stat` rules out nearly all of them first; only a path that does not
    // exist, or that is the entry's file, takes the full resolution.
    #[cfg(unix)]
    let entry_identity = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(entry)
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino()))
    };
    let mut aliases = Vec::new();
    for (file, path) in engine.source_files() {
        #[cfg(unix)]
        if let Some(entry_identity) = entry_identity {
            use std::os::unix::fs::MetadataExt;
            let candidate = Path::new(project_root).join(&path);
            if let Ok(metadata) = std::fs::metadata(&candidate)
                && (metadata.dev(), metadata.ino()) != entry_identity
            {
                continue;
            }
        }
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

#[cfg(test)]
mod source_check_cache_tests {
    use super::*;
    use kiln_db::MemoEngine;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn engine_with_sources(sources: &[(&str, &str)]) -> MemoEngine {
        let mut engine = MemoEngine::new(kiln_frontend_oxc::parse);
        for (path, text) in sources {
            let file = engine.intern(path);
            engine.set_source(file, (*text).to_owned());
        }
        engine
    }

    fn key_for(engine: &MemoEngine, root: &Path, entry: &Path) -> GraphCacheKey {
        let mut key = GraphCacheKey::new(root.to_path_buf(), entry.to_path_buf(), None, false);
        key.source_check_identity = source_check_identity(engine);
        key
    }

    fn revisions(engine: &MemoEngine) -> Vec<GraphSourceRevision> {
        engine
            .source_files()
            .into_iter()
            .map(|(file, _)| GraphSourceRevision {
                file,
                revision: engine
                    .source_revision(file)
                    .expect("active source revision"),
            })
            .collect()
    }

    fn graph_fixture(file_name: &str, source: &str) -> (PathBuf, PathBuf, ModuleGraph) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kiln-source-check-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create graph fixture");
        let entry = root.join(file_name);
        std::fs::write(&entry, source).expect("write graph entry");
        let root = std::fs::canonicalize(root).expect("canonical graph root");
        let entry = std::fs::canonicalize(entry).expect("canonical graph entry");
        let graph = ModuleGraph::build(
            &entry,
            &root,
            |path: &Path| std::fs::read_to_string(path),
            kiln_frontend_oxc::parse,
        )
        .expect("build one-module graph");
        (root, entry, graph)
    }

    fn source_snapshot(
        root: &Path,
        entry: &Path,
        sources: &[(&str, &str)],
        source_types: Arc<OnceLock<SnapshotSourceTypes>>,
    ) -> OwnedGraphSnapshot {
        OwnedGraphSnapshot {
            source_view: SourceView(0),
            project_root: root.to_path_buf(),
            entry: entry.to_path_buf(),
            resolutions: None,
            node: false,
            sources: sources
                .iter()
                .map(|(path, text)| {
                    (
                        PathBuf::from(path),
                        OwnedGraphSource {
                            file: DbFileId(0),
                            text: (*text).to_owned(),
                            revision: Revision(0),
                        },
                    )
                })
                .collect(),
            revisions: Vec::new(),
            source_types,
        }
    }

    #[test]
    fn type_source_identity_tracks_alias_edits_and_source_membership() {
        let mut engine = engine_with_sources(&[
            ("/project/main.ts", "export const value: Alias = 1;"),
            ("/project/ambient.d.ts", "type Alias = number;"),
        ]);
        let root = PathBuf::from("/project");
        let entry = PathBuf::from("/project/main.ts");
        let unchanged = key_for(&engine, &root, &entry);
        assert!(unchanged.source_check_identity.is_some());
        assert_eq!(key_for(&engine, &root, &entry), unchanged);
        let (_, _, graph) = graph_fixture("entry.ts", "export const value: number = 1;\n");
        let mut cache = GraphCache::default();
        cache.insert(unchanged.clone(), revisions(&engine), graph.clone());
        assert!(matches!(
            cache.lookup(&key_for(&engine, &root, &entry), |file| engine
                .source_revision(file)),
            GraphCacheLookup::Hit(_)
        ));

        let alias = engine.intern("/project/ambient.d.ts");
        engine.set_source(alias, "type Alias = string;".to_owned());
        let edited_alias = key_for(&engine, &root, &entry);
        assert_ne!(
            edited_alias, unchanged,
            "type-only edits invalidate TS graphs"
        );
        assert!(matches!(
            cache.lookup(&edited_alias, |file| engine.source_revision(file)),
            GraphCacheLookup::Miss
        ));

        let added = engine.intern("/project/added.d.ts");
        engine.set_source(added, "declare const extra: boolean;".to_owned());
        let with_added_source = key_for(&engine, &root, &entry);
        assert_ne!(
            with_added_source, edited_alias,
            "new ambient files change the source set"
        );
        assert!(matches!(
            cache.lookup(&with_added_source, |file| engine.source_revision(file)),
            GraphCacheLookup::Miss
        ));
        cache.insert(with_added_source, revisions(&engine), graph);

        engine.remove_source(added);
        let after_removal = key_for(&engine, &root, &entry);
        assert_eq!(after_removal, edited_alias);
        assert!(matches!(
            cache.lookup(&after_removal, |file| engine.source_revision(file)),
            GraphCacheLookup::Miss
        ));
    }

    #[test]
    fn unchanged_typescript_snapshot_reuses_warm_graph_and_js_unrelated_edits_do_too() {
        let (root, entry, graph) = graph_fixture("entry.ts", "export const value: number = 1;\n");
        let engine = engine_with_sources(&[
            ("/project/main.ts", "export const value: number = 1;"),
            ("/project/types.d.ts", "type Alias = number;"),
        ]);
        let key = key_for(&engine, &root, &entry);
        let mut cache = GraphCache::default();
        cache.insert(key.clone(), revisions(&engine), graph.clone());
        assert!(matches!(
            cache.lookup(&key_for(&engine, &root, &entry), |file| engine
                .source_revision(file)),
            GraphCacheLookup::Hit(_)
        ));

        let (js_root, js_entry, js_graph) = graph_fixture("entry.js", "export const value = 1;\n");
        let mut js_engine = engine_with_sources(&[
            ("/project/entry.js", "export const value = 1;"),
            ("/project/unrelated.js", "export const spare = 0;"),
        ]);
        let js_key = key_for(&js_engine, &js_root, &js_entry);
        assert!(js_key.source_check_identity.is_none());
        let entry_file = js_engine.intern("/project/entry.js");
        let js_graph_sources = vec![GraphSourceRevision {
            file: entry_file,
            revision: js_engine
                .source_revision(entry_file)
                .expect("entry revision"),
        }];
        cache.insert(js_key.clone(), js_graph_sources, js_graph);
        let unrelated = js_engine.intern("/project/unrelated.js");
        js_engine.set_source(unrelated, "export const spare = 1;".to_owned());
        assert!(matches!(
            cache.lookup(&key_for(&js_engine, &js_root, &js_entry), |file| js_engine
                .source_revision(file)),
            GraphCacheLookup::Hit(_)
        ));
    }

    #[test]
    fn source_check_slots_share_by_identity_and_isolate_entries_and_edits() {
        let root = PathBuf::from("/project");
        let entry = PathBuf::from("/project/main.ts");
        let other_entry = PathBuf::from("/project/other.ts");
        let mut engine = engine_with_sources(&[
            (
                "/project/main.ts",
                "import type { Amount } from './types'; const amount: Amount = 'wrong';",
            ),
            ("/project/other.ts", "const label: string = 'ok';"),
            ("/project/types.ts", "export type Amount = number;"),
        ]);
        let original_identity = source_check_identity(&engine).expect("TS source identity");
        let mut cache = GraphCache::default();
        let original_slot = cache.source_check_slot(root.clone(), entry.clone(), original_identity);
        let original_snapshot = source_snapshot(
            &root,
            &entry,
            &[
                (
                    "/project/main.ts",
                    "import type { Amount } from './types'; const amount: Amount = 'wrong';",
                ),
                ("/project/other.ts", "const label: string = 'ok';"),
                ("/project/types.ts", "export type Amount = number;"),
            ],
            Arc::clone(&original_slot),
        );
        let before = snapshot_source_types(&original_snapshot)
            .expect("TypeScript snapshot is checked")
            .checked
            .clone();
        assert!(before.complete);

        let same_entry_slot =
            cache.source_check_slot(root.clone(), entry.clone(), original_identity);
        assert!(Arc::ptr_eq(&original_slot, &same_entry_slot));
        let same_snapshot = source_snapshot(
            &root,
            &entry,
            &[
                (
                    "/project/main.ts",
                    "import type { Amount } from './types'; const amount: Amount = 'wrong';",
                ),
                ("/project/other.ts", "const label: string = 'ok';"),
                ("/project/types.ts", "export type Amount = number;"),
            ],
            Arc::clone(&same_entry_slot),
        );
        assert!(std::ptr::eq(
            original_slot
                .get()
                .expect("first snapshot initialized slot"),
            snapshot_source_types(&same_snapshot).expect("shared slot is initialized")
        ));
        let other_entry_slot =
            cache.source_check_slot(root.clone(), other_entry.clone(), original_identity);
        assert!(!Arc::ptr_eq(&original_slot, &other_entry_slot));
        let other_snapshot = source_snapshot(
            &root,
            &other_entry,
            &[
                (
                    "/project/main.ts",
                    "import type { Amount } from './types'; const amount: Amount = 'wrong';",
                ),
                ("/project/other.ts", "const label: string = 'ok';"),
                ("/project/types.ts", "export type Amount = number;"),
            ],
            Arc::clone(&other_entry_slot),
        );
        let other = snapshot_source_types(&other_snapshot)
            .expect("second TypeScript entry is checked")
            .checked
            .clone();
        assert!(other.complete);
        assert!(
            !other.diagnostics.iter().any(|d| d.code == Some(2322)),
            "the other entry must not inherit main.ts diagnostics"
        );

        let declarations = engine.intern("/project/types.ts");
        engine.set_source(
            declarations,
            "export type Amount = string; const valid: Amount = 'ok';".to_owned(),
        );
        let edited_identity = source_check_identity(&engine).expect("edited TS identity");
        assert_ne!(edited_identity, original_identity);
        let edited_slot = cache.source_check_slot(root.clone(), entry.clone(), edited_identity);
        assert!(!Arc::ptr_eq(&original_slot, &edited_slot));
        let edited_snapshot = source_snapshot(
            &root,
            &entry,
            &[
                (
                    "/project/main.ts",
                    "import type { Amount } from './types'; const amount: Amount = 'wrong';",
                ),
                ("/project/other.ts", "const label: string = 'ok';"),
                (
                    "/project/types.ts",
                    "export type Amount = string; const valid: Amount = 'ok';",
                ),
            ],
            Arc::clone(&edited_slot),
        );
        let after = snapshot_source_types(&edited_snapshot)
            .expect("edited TypeScript snapshot is checked")
            .checked
            .clone();
        assert!(after.complete);
        assert_ne!(
            before, after,
            "the alias edit must change checked contracts"
        );
        assert!(before.diagnostics.iter().any(|d| d.code == Some(2322)));
        assert!(!after.diagnostics.iter().any(|d| d.code == Some(2322)));
        assert_eq!(
            snapshot_source_types(&original_snapshot)
                .expect("old snapshot remains available")
                .checked,
            before,
            "editing sources must not mutate an in-flight snapshot result"
        );

        // A third identity evicts the oldest cache entry, but snapshots already holding its Arc
        // keep their immutable result. Explicit graph invalidation then prevents lookup reuse.
        let extra_slot = cache.source_check_slot(
            root.clone(),
            entry.clone(),
            kiln_digest::Digest::hash(b"third source identity"),
        );
        assert!(!Arc::ptr_eq(&extra_slot, &original_slot));
        assert_eq!(
            snapshot_source_types(&original_snapshot)
                .expect("eviction cannot invalidate an in-flight snapshot")
                .checked,
            before
        );
        cache.invalidate();
        let post_invalidation = cache.source_check_slot(root, entry, original_identity);
        assert!(!Arc::ptr_eq(&post_invalidation, &original_slot));
    }

    #[test]
    fn json_auxiliary_edits_get_a_fresh_source_check_slot() {
        let root = PathBuf::from("/project");
        let entry = PathBuf::from("/project/main.ts");
        let mut engine = engine_with_sources(&[
            ("/project/main.ts", "export const value = 1;"),
            ("/project/data.json", "{\"mode\":\"before\"}"),
        ]);
        let before = source_check_identity(&engine).expect("TS source identity");
        let mut cache = GraphCache::default();
        let before_slot = cache.source_check_slot(root.clone(), entry.clone(), before);

        let json = engine.intern("/project/data.json");
        engine.set_source(json, "{\"mode\":\"after\"}".to_owned());
        let after = source_check_identity(&engine).expect("updated TS source identity");
        assert_ne!(before, after, "JSON is an auxiliary source-check input");
        let after_slot = cache.source_check_slot(root, entry, after);
        assert!(!Arc::ptr_eq(&before_slot, &after_slot));
    }
}
