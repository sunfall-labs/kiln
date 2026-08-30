//! Turning one [`Request`] into one [`Response`] against the query engine.
//!
//! Deliberately free of I/O on the socket and of any knowledge of threads. Query preparation runs
//! while the engine lock is held; owned interpreter/native work can then execute without it.

use kiln_db::QueryEngine;
use kiln_module_graph::ModuleGraph;
use kiln_proto::{
    Diagnostic, FileId, ModuleSummary, NativeBuildSummary, NativeProfile, NativeRunSummary,
    NativeSharedLibrarySummary, PROTOCOL_VERSION, ProjectSummary, Request, Response, RunSummary,
    ScalarValueSummary, Stats,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
        graph: Result<ModuleGraph, kiln_module_graph::ModuleGraphError>,
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

        Request::Sync { files } => (sync(engine, files), Next::Continue),

        Request::SetSource { path, text } => {
            let file = engine.intern(&path);
            engine.set_source(file, text);
            (Response::Ok, Next::Continue)
        }

        Request::Check { path } => (check(engine, path), Next::Continue),
        Request::Run { path, project_root } => (run(engine, path, project_root), Next::Continue),
        Request::NativeBuild {
            path,
            project_root,
            output,
            profile,
        } => (
            match native_graph(engine, &path, &project_root) {
                Ok(Some(graph)) => native_build_graph(graph, path, project_root, output, profile),
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
        } => (
            match native_graph(engine, &path, &project_root) {
                Ok(Some(graph)) if graph.module_count() > 1 => {
                    native_build_shared_graph(graph, path, project_root, output, profile)
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
        } => (
            match native_graph(engine, &path, &project_root) {
                Ok(Some(graph)) => native_run_graph(graph, path, project_root, profile),
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

        Request::Project => {
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

        Request::Stats => (Response::Stats(stats(engine)), Next::Continue),

        Request::ResetStats => {
            engine.reset_stats();
            (Response::Ok, Next::Continue)
        }

        Request::Shutdown => (Response::Ok, Next::StopDaemon),
    }
}

/// Reconcile the daemon's inputs with the client's complete view.
///
/// Sync is intentionally authoritative: an input omitted by the client is no longer part of
/// the project and must be removed before the response is returned. Validate the complete
/// request before mutating the engine so a malformed request cannot leave a half-reconciled
/// project behind.
fn sync<E: QueryEngine>(engine: &mut E, files: Vec<kiln_proto::SourceHash>) -> Response {
    let mut client = BTreeMap::new();
    let mut ordered = Vec::new();
    let mut duplicates = BTreeSet::new();
    for file in files {
        if client.insert(file.path.clone(), file.hash).is_some() {
            duplicates.insert(file.path);
        } else {
            ordered.push((file.path, file.hash));
        }
    }
    if !duplicates.is_empty() {
        let paths = duplicates.into_iter().collect::<Vec<_>>().join(", ");
        return Response::Error {
            message: format!("Sync request contains duplicate paths: {paths}"),
        };
    }

    // Keep both the stable file id and the current hash from one snapshot. In particular, do
    // not call `intern` while validating: a missing path needs no id until its SetSource arrives,
    // and interning before validation would make a rejected request observable.
    let held = engine
        .source_files()
        .into_iter()
        .map(|(file, path)| {
            let hash = engine.source(file).map(kiln_proto::source_digest);
            (path, (file, hash))
        })
        .collect::<BTreeMap<_, _>>();

    // Removal is part of the Sync transaction's authoritative reconciliation. QueryEngine's
    // input mutation invalidates all dependent queries, so this happens before determining what
    // the client still needs to upload.
    for (path, (file, _)) in &held {
        if !client.contains_key(path) {
            engine.remove_source(*file);
        }
    }

    let needed = ordered
        .into_iter()
        .filter_map(|(path, expected)| match held.get(&path) {
            Some((_, Some(actual))) if *actual == expected => None,
            _ => Some(path),
        })
        .collect();
    Response::Needed { paths: needed }
}

/// Run the pipeline for one module.
///
/// Reading from disk only when the file has no source yet is what makes an editor-driven
/// `SetSource` authoritative: once a client has pushed unsaved buffer contents, later `Check`s
/// must see the buffer, not the stale bytes on disk.
fn check<E: QueryEngine>(engine: &mut E, path: String) -> Response {
    let file = match ensure_source(engine, &path) {
        Ok(file) => file,
        Err(message) => return Response::Error { message },
    };

    let parsed = engine.parse(file);
    // The interface, not `parsed.exports`, is the externally visible surface, and it is what a
    // dependent would be recompiled against - so it is what the summary reports.
    let interface = engine.interface(file);

    Response::Checked(ModuleSummary {
        path,
        file: FileId(file.0),
        implementation_hash: parsed.implementation_hash,
        interface_hash: interface.interface_hash,
        exports: interface.exports,
        diagnostics: parsed
            .module
            .diagnostics
            .into_iter()
            .map(|diagnostic| Diagnostic {
                message: diagnostic.message,
                start: diagnostic.span.map(|span| span.start),
                end: diagnostic.span.map(|span| span.end),
            })
            .collect(),
    })
}

/// Prepare one interpreted run from a single engine snapshot.
pub(crate) fn prepare_run<E: QueryEngine>(
    engine: &mut E,
    path: String,
    project_root: String,
) -> Result<PreparedRun, String> {
    let file = ensure_source(engine, &path)?;
    let entry = std::fs::canonicalize(&path).or_else(|_| {
        let candidate = Path::new(&project_root).join(&path);
        std::fs::canonicalize(candidate)
    });
    let prepared = if let Ok(entry) = entry {
        alias_source(engine, file, &entry);
        PreparedRun::Graph {
            graph: build_graph(engine, &entry, &project_root),
            path,
            file,
            revision: engine.revision().0,
        }
    } else {
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
pub(crate) fn execute_prepared_run(prepared: PreparedRun) -> Response {
    #[cfg(test)]
    wait_for_run_gate(prepared.path());

    match prepared {
        PreparedRun::Graph {
            graph,
            path,
            file,
            revision,
        } => execute_graph(graph, path, file, revision),
        PreparedRun::Program {
            program,
            path,
            file,
            revision,
        } => execute_program(program, path, file, revision),
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

fn run<E: QueryEngine>(engine: &mut E, path: String, project_root: String) -> Response {
    match prepare_run(engine, path, project_root) {
        Ok(prepared) => execute_prepared_run(prepared),
        Err(message) => Response::Error { message },
    }
}

fn execute_graph(
    graph: Result<ModuleGraph, kiln_module_graph::ModuleGraphError>,
    path: String,
    file: kiln_db::FileId,
    revision: u64,
) -> Response {
    let mut host = kiln_runtime::CaptureHost::new();
    let result = match graph {
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
    entry: &Path,
    project_root: &str,
) -> Result<ModuleGraph, kiln_module_graph::ModuleGraphError> {
    ModuleGraph::build_with_module_provider(entry, project_root, |module_path| {
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
    })
}

fn module_graph_diagnostics(error: kiln_module_graph::ModuleGraphError) -> Vec<Diagnostic> {
    match error {
        kiln_module_graph::ModuleGraphError::Compile { diagnostics, .. } => diagnostics
            .into_iter()
            .map(|diagnostic| Diagnostic {
                message: diagnostic.message,
                start: diagnostic.span.map(|span| span.start),
                end: diagnostic.span.map(|span| span.end),
            })
            .collect(),
        error => vec![Diagnostic {
            message: error.to_string(),
            start: None,
            end: None,
        }],
    }
}

pub(crate) fn native_graph<E: QueryEngine>(
    engine: &mut E,
    path: &str,
    project_root: &str,
) -> Result<Option<ModuleGraph>, String> {
    let file = ensure_source(engine, path)?;
    let entry = std::fs::canonicalize(path)
        .or_else(|_| std::fs::canonicalize(Path::new(project_root).join(path)));
    let Ok(entry) = entry else {
        return Ok(None);
    };
    alias_source(engine, file, &entry);
    build_graph(engine, &entry, project_root)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn execute_program(
    program: Result<kiln_ir::Program, Vec<kiln_ir::CompileDiagnostic>>,
    path: String,
    file: kiln_db::FileId,
    revision: u64,
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
                        message: diagnostic.message,
                        start: diagnostic.span.map(|span| span.start),
                        end: diagnostic.span.map(|span| span.end),
                    })
                    .collect(),
            });
        }
    };

    let mut host = kiln_runtime::CaptureHost::new();
    match kiln_interpreter::execute(std::rc::Rc::new(program), &mut host, Default::default()) {
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

pub(crate) fn native_build_program(
    program: kiln_ir::Program,
    path: String,
    project_root: String,
    output: Option<String>,
    profile: NativeProfile,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let output = output.map_or_else(
        || default_native_output(&project_root, &path),
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
        Ok(report) => Response::NativeBuilt(native_summary(report)),
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
        || default_native_output(Path::new(project_root), path),
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
        Ok(report) => Response::NativeSharedBuilt(native_shared_summary(report)),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

pub(crate) fn native_build_shared_graph(
    graph: ModuleGraph,
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
    match builder.build_shared_graph(&graph, &options) {
        Ok(report) => Response::NativeSharedBuilt(native_shared_summary(report)),
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
        || default_native_shared_output(&project_root, path),
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
    graph: ModuleGraph,
    path: String,
    project_root: String,
    output: Option<String>,
    profile: NativeProfile,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let output = output.map_or_else(
        || default_native_output(&project_root, &path),
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
        &graph,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
        },
    ) {
        Ok(report) => Response::NativeBuilt(native_summary(report)),
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
    let output = default_native_output(&project_root, &path);
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
        Ok(report) => Response::NativeRan(NativeRunSummary {
            exit_code: report.exit_code,
            stdout: String::from_utf8_lossy(&report.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&report.stderr).into_owned(),
            scalar_value: report.result.map(native_scalar_value_summary),
            build: native_summary(report.build),
        }),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

pub(crate) fn native_run_graph(
    graph: ModuleGraph,
    path: String,
    project_root: String,
    profile: NativeProfile,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let output = default_native_output(&project_root, &path);
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
        &graph,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
        },
    ) {
        Ok(report) => Response::NativeRan(NativeRunSummary {
            exit_code: report.exit_code,
            stdout: String::from_utf8_lossy(&report.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&report.stderr).into_owned(),
            scalar_value: report.result.map(native_scalar_value_summary),
            build: native_summary(report.build),
        }),
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

fn default_native_output(project_root: &Path, source: &str) -> PathBuf {
    let stem = Path::new(source)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module");
    let mut output = project_root.join(".kiln/bin").join(stem);
    if cfg!(windows) {
        output.set_extension("exe");
    }
    output
}

fn default_native_shared_output(project_root: &Path, source: &str) -> PathBuf {
    let stem = Path::new(source)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module");
    let extension = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(windows) {
        "dll"
    } else {
        "so"
    };
    project_root
        .join(".kiln/lib")
        .join(stem)
        .with_extension(extension)
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
        output_path: report.output.to_string_lossy().into_owned(),
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

/// Keep a canonical graph key synchronized with a legacy relative-path overlay. Editors may
/// resend the relative spelling after every edit, so only filling a missing canonical slot would
/// leave the graph executing stale bytecode.
fn alias_source<E: QueryEngine>(engine: &mut E, file: kiln_db::FileId, entry: &Path) {
    let Some(entry_string) = entry.to_str() else {
        return;
    };
    let canonical_file = engine.intern(entry_string);
    let Some(source) = engine.source(file).map(str::to_owned) else {
        return;
    };
    if engine.source(canonical_file) != Some(source.as_str()) {
        engine.set_source(canonical_file, source);
    }
}

fn stats<E: QueryEngine>(engine: &E) -> Stats {
    let stats = engine.stats();
    Stats {
        revision: stats.revision,
        parse_computed: stats.parse_computed,
        ir_computed: stats.ir_computed,
        interface_computed: stats.interface_computed,
        project_computed: stats.project_computed,
        validated: stats.validated,
        early_cutoff: stats.early_cutoff,
    }
}
