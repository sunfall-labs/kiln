mod realm_bootstrap;
pub use realm_bootstrap::realm_bootstrap;

use kiln_host_abi::HostOps;
use kiln_interpreter::{
    Cell, CommonJsModuleLoader, CommonJsRequire, ExecutionCapabilities, ExecutionError,
    ExecutionLimits, ExecutionResult, ImportAttributes, ModuleBinding, ModuleExportValue,
    ModuleLoadError, ModuleLoadRequest, ModuleLoaderService, ModulePlan, ModuleRealmSession,
    ModuleSelfExport, ModuleSelfImport, ModuleStatus, PlannedBody, PlannedModule, Property,
    RealmId, SharedModuleMap, Value, execute_module_in_runtime_with_loader,
    execute_node_module_in_session_with_loader, lookup_export, ordinary_object, own_data_property,
    parse_json_module, read_through_cell,
};
use kiln_ir::{BYTECODE_VERSION, Instruction, Program};
use kiln_module_graph::{ModuleGraph, ModuleGraphError, ModuleNode, ModuleSource, ResolvedExport};
use kiln_module_resolver::NodeBuiltin;
use kiln_runtime::{ClockCapability, RealmCapabilities, RealmRuntime};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum ModuleEvaluationError {
    Graph(ModuleGraphError),
    Execution {
        module: PathBuf,
        error: ExecutionError,
    },
    StepOverflow,
}

impl From<ModuleGraphError> for ModuleEvaluationError {
    fn from(error: ModuleGraphError) -> Self {
        Self::Graph(error)
    }
}

impl std::fmt::Display for ModuleEvaluationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Graph(error) => error.fmt(formatter),
            Self::Execution { module, error } => write!(
                formatter,
                "could not execute module {}: {error}",
                module.display()
            ),
            Self::StepOverflow => formatter.write_str("module execution step count overflowed"),
        }
    }
}

impl std::error::Error for ModuleEvaluationError {}

pub trait ModuleGraphExecution {
    fn execute<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
    ) -> Result<ExecutionResult, ModuleEvaluationError>;
    fn execute_with_clock<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        clock: ClockCapability,
    ) -> Result<ExecutionResult, ModuleEvaluationError>;
    fn execute_node<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
    ) -> Result<ExecutionResult, ModuleEvaluationError>;
    fn execute_node_with_clock<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        clock: ClockCapability,
    ) -> Result<ExecutionResult, ModuleEvaluationError>;
    fn import_plan(&self) -> Result<ModulePlan, ModuleEvaluationError>;
    fn import_plan_for(&self, entry: impl AsRef<Path>)
    -> Result<ModulePlan, ModuleEvaluationError>;
    fn import_plan_for_request(
        &self,
        entry: impl AsRef<Path>,
        attributes: Option<&ImportAttributes>,
    ) -> Result<ModulePlan, ModuleEvaluationError>;
}

impl ModuleGraphExecution for ModuleGraph {
    fn execute<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        GraphExecutor(self).execute(host, limits, capabilities)
    }
    fn execute_with_clock<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        clock: ClockCapability,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        GraphExecutor(self).execute_with_clock(host, limits, capabilities, clock)
    }
    fn execute_node<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        GraphExecutor(self).execute_node(host, limits, capabilities)
    }
    fn execute_node_with_clock<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        clock: ClockCapability,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        GraphExecutor(self).execute_node_with_clock(host, limits, capabilities, clock)
    }
    fn import_plan(&self) -> Result<ModulePlan, ModuleEvaluationError> {
        GraphExecutor(self).import_plan()
    }
    fn import_plan_for(
        &self,
        entry: impl AsRef<Path>,
    ) -> Result<ModulePlan, ModuleEvaluationError> {
        GraphExecutor(self).import_plan_for(entry)
    }
    fn import_plan_for_request(
        &self,
        entry: impl AsRef<Path>,
        attributes: Option<&ImportAttributes>,
    ) -> Result<ModulePlan, ModuleEvaluationError> {
        GraphExecutor(self).import_plan_for_request(entry, attributes)
    }
}

struct GraphExecutor<'graph>(&'graph ModuleGraph);

thread_local! {
    /// The value of the last exception a module or the event loop left uncaught on this thread:
    /// what a worker whose realm ended with one reports to its parent's `error` event.
    static LAST_THROWN: std::cell::RefCell<Option<Value>> = const { std::cell::RefCell::new(None) };
}

/// Workers for a graph's program (`node:worker_threads`, realms issue 06): each runs one of the
/// graph's modules as its entry, in a realm of its own on the worker's thread, with the same
/// limits and capabilities as the program that started it.
struct GraphWorkers {
    graph: ModuleGraph,
    limits: ExecutionLimits,
    capabilities: ExecutionCapabilities,
    clock: ClockCapability,
}

impl kiln_interpreter::WorkerHost for GraphWorkers {
    fn has_module(&self, path: &str) -> bool {
        if path == "kiln:actors/worker" {
            let entry = kiln_module_resolver::NodeBuiltin::ActorsWorker
                .virtual_path(self.graph.project_root());
            return self.graph.module(entry).is_some();
        }
        // A worker script the compiler did not see named is loaded from disk like any module
        // (the interpreter answers what node would; a native build refuses it by name).
        self.graph.module(path).is_some() || Path::new(path).is_file()
    }

    fn run(&self, worker: Arc<kiln_runtime::workers::WorkerContext>) -> i32 {
        use kiln_runtime::workers::WorkerEntry;
        let graph = match &worker.spec.entry {
            WorkerEntry::File(path) => self
                .graph
                .with_entry(if path == "kiln:actors/worker" {
                    kiln_module_resolver::NodeBuiltin::ActorsWorker
                        .virtual_path(self.graph.project_root())
                } else {
                    PathBuf::from(path)
                })
                .map(Ok)
                .unwrap_or_else(|| disk_graph(self.graph.project_root(), Path::new(path))),
            WorkerEntry::Eval(source) => eval_graph(source),
        };
        let graph = match graph {
            Ok(graph) => graph,
            Err(message) => {
                worker.report_error(worker_failure(message));
                return 1;
            }
        };
        let mut host = WorkerStdio {
            worker: Arc::clone(&worker),
            inner: kiln_runtime::CaptureHost::new().with_fs(Box::new(kiln_host_abi::OsFileSystem)),
        };
        let capabilities = self
            .capabilities
            .clone()
            .with_argv(worker.spec.argv.clone());
        LAST_THROWN.with(|last| last.borrow_mut().take());
        let outcome =
            graph.execute_node_with_clock(&mut host, self.limits, capabilities, self.clock);
        let thrown = LAST_THROWN.with(|last| last.borrow_mut().take());
        match outcome {
            Ok(result) => result.process_exit_code.unwrap_or(0),
            Err(ModuleEvaluationError::Execution {
                error: ExecutionError::ProcessExited(code),
                ..
            }) => code,
            Err(ModuleEvaluationError::Execution {
                error:
                    ExecutionError::Uncaught {
                        error_constructor,
                        message,
                        ..
                    },
                ..
            }) => {
                let tokens = match thrown {
                    Some(value) => {
                        kiln_interpreter::worker_error_tokens(&value, error_constructor.as_deref())
                    }
                    None => {
                        kiln_interpreter::worker_error_tokens(&Value::String(message.into()), None)
                    }
                };
                worker.report_error(tokens);
                1
            }
            Err(error) => {
                worker.report_error(kiln_interpreter::worker_error_tokens(
                    &Value::String(error.to_string().into()),
                    None,
                ));
                1
            }
        }
    }
}

/// A failure to start a worker's program, as the `Error` its parent's `error` event gets.
fn worker_failure(message: String) -> Vec<kiln_runtime::workers::Token> {
    let mut error = kiln_interpreter::ObjectData::default();
    error.set("message", Value::String(message.into()));
    kiln_interpreter::worker_error_tokens(
        &Value::Object(std::rc::Rc::new(std::cell::RefCell::new(error))),
        Some("Error"),
    )
}

/// The graph a worker made from source text runs (`new Worker(code, { eval: true })`): the
/// text is a module of its own at `[worker eval]` in the working directory, whose requests
/// resolve from there, as node's do. CommonJS, as node runs it, unless it has module syntax.
/// The frontend a graph built while the program runs uses: node's grammar goals, the same
/// lowering every other graph gets.
struct NodeFrontend;

impl kiln_module_graph::Frontend for NodeFrontend {
    fn parse(&mut self, path: &str, source: &str) -> kiln_hir::Module {
        kiln_frontend_oxc::parse_with_goal(path, source, kiln_hir::SourceGoal::Module)
    }

    fn parse_commonjs(&mut self, path: &str, source: &str) -> kiln_module_graph::CommonJsParse {
        let (module, static_requires) =
            kiln_frontend_oxc::parse_commonjs_with_requires(path, source);
        kiln_module_graph::CommonJsParse {
            module,
            static_requires,
        }
    }
}

/// The graph of a worker script the program's graph does not hold, built from disk the way
/// node would load it.
fn disk_graph(project_root: &Path, path: &Path) -> Result<ModuleGraph, String> {
    let entry = std::fs::canonicalize(path)
        .map_err(|_| format!("Cannot find module '{}'", path.display()))?;
    let resolver = kiln_module_resolver::NodeRuntimeResolver::new(project_root)
        .map_err(|error| error.to_string())?;
    ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| std::fs::read_to_string(path),
        NodeFrontend,
    )
    .map_err(|error| error.to_string())
}

fn eval_graph(source: &str) -> Result<ModuleGraph, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let build = |name: &str| {
        let resolver = kiln_module_resolver::NodeRuntimeResolver::new(&cwd).map_err(|error| {
            ModuleGraphError::Source {
                path: cwd.clone(),
                message: error.to_string(),
            }
        })?;
        ModuleGraph::build_node_virtual(
            cwd.join(name),
            source.to_owned(),
            resolver,
            |path: &Path| std::fs::read_to_string(path),
            NodeFrontend,
        )
    };
    match build("[worker eval].cjs") {
        Err(ModuleGraphError::CjsStaticModuleSyntax { .. }) => build("[worker eval].mjs"),
        other => other,
    }
    .map_err(|error| error.to_string())
}

/// A worker's host: its standard streams go to its parent as messages, where node's worker
/// pipes them into the parent's own; everything else is the ordinary capture host's.
struct WorkerStdio {
    worker: Arc<kiln_runtime::workers::WorkerContext>,
    inner: kiln_runtime::CaptureHost,
}

impl HostOps for WorkerStdio {
    fn call(
        &mut self,
        operation: kiln_host_abi::HostOp,
        arguments: &[kiln_host_abi::HostValue],
    ) -> Result<kiln_host_abi::HostValue, kiln_host_abi::HostError> {
        use kiln_host_abi::HostOp;
        match operation {
            HostOp::WriteStdout | HostOp::WriteStderr => {
                let mut line = String::new();
                for (index, argument) in arguments.iter().enumerate() {
                    if index > 0 {
                        line.push(' ');
                    }
                    line.push_str(&argument.to_console_string());
                }
                line.push('\n');
                self.worker
                    .write_stdio(operation == HostOp::WriteStderr, line.as_str().into());
                Ok(kiln_host_abi::HostValue::Undefined)
            }
            _ => self.inner.call(operation, arguments),
        }
    }

    fn fs(&mut self) -> Option<&mut (dyn kiln_host_abi::FileSystemOps + '_)> {
        self.inner.fs()
    }

    fn clone_fs(&self) -> Option<Box<dyn kiln_host_abi::FileSystemOps>> {
        self.inner.clone_fs()
    }

    fn run_process_sync(
        &mut self,
        command: &str,
        args: &[String],
    ) -> Result<kiln_host_abi::SyncProcessOutput, kiln_host_abi::HostError> {
        self.inner.run_process_sync(command, args)
    }
}

impl std::ops::Deref for GraphExecutor<'_> {
    type Target = ModuleGraph;
    fn deref(&self) -> &Self::Target {
        self.0
    }
}

/// A module's key in the realm's module map: its canonical path.
fn module_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A copy of `program` that hands back the values in `slots` instead of `undefined`.
///
/// A module body ends by returning `undefined`, and its bindings die with its frame. The only
/// way an importer running in the same machine can see them is for the body to *return* them, so
/// the trailing `Return` becomes a `Pop` -- it consumes the same one value, so a jump that
/// targeted it still balances -- and the epilogue is appended behind it.
fn export_epilogue(program: &Program, slots: Vec<u32>) -> Program {
    let mut program = program.clone();
    if matches!(program.code.last(), Some(Instruction::Return)) {
        let last = program.code.len() - 1;
        program.code[last] = Instruction::Pop;
    }
    let count = u32::try_from(slots.len()).unwrap_or(u32::MAX);
    for slot in slots {
        program.code.push(Instruction::LoadLocal(slot));
    }
    program.code.push(Instruction::NewArray { count });
    program.code.push(Instruction::Return);
    program
}

/// A realm-scoped dynamic importer backed by one immutable graph snapshot.
///
/// Requests carry their importer, so nested dynamic imports resolve against the module that
/// issued them. The service owns the graph through `Arc`; no thread-local or process-global
/// resolver state is involved.
///
/// Runtime requests can reach past that snapshot in two cases: an actor entry (`spawnActor`,
/// ADR 0072) and an ordinary dynamic import whose target is not part of the original graph.
/// These names are runtime values, so the interpreter resolves them with the same Node resolver
/// and builds an additional graph from disk. Each built graph is retained by this loader, which
/// lets its modules resolve their own imports and keeps every module in the same realm map.
pub struct GraphModuleLoader {
    graph: Arc<ModuleGraph>,
    /// Runtime graphs retain the requested file URL identity as well as their canonical path.
    /// Query and fragment variants are distinct Node module instances and can observe different
    /// source bytes when the file changes between imports.
    runtime_graphs: std::sync::Mutex<HashMap<RuntimeGraphKey, Arc<ModuleGraph>>>,
    /// Successfully loaded URL identities remain resolvable after their source file is removed.
    /// This mirrors Node's module cache: resolving an already loaded URL must not stat the file
    /// again or discard the source graph that was retained for that module instance.
    runtime_url_graphs: std::sync::Mutex<HashMap<String, Arc<ModuleGraph>>>,
    /// A resolved request keeps its graph even if the target is removed or its resolver inputs
    /// later change. The importer is part of the key because the same relative specifier can
    /// name different files from different modules.
    runtime_requests: std::sync::Mutex<HashMap<RuntimeRequestKey, Arc<ModuleGraph>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RuntimeGraphKey {
    entry: PathBuf,
    url_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RuntimeRequestKey {
    importer: String,
    specifier: String,
}

impl GraphModuleLoader {
    pub fn new(graph: Arc<ModuleGraph>) -> Self {
        Self {
            graph,
            runtime_graphs: std::sync::Mutex::new(HashMap::new()),
            runtime_url_graphs: std::sync::Mutex::new(HashMap::new()),
            runtime_requests: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The graph that holds `importer`: the snapshot, or a graph built for a runtime import.
    fn graph_holding(
        &self,
        importer: &Path,
        importer_identity: Option<&str>,
    ) -> Option<Arc<ModuleGraph>> {
        let graphs = self.runtime_graphs.lock().ok()?;
        if let Some(identity) = importer_identity
            && let Some(graph) = graphs.get(&RuntimeGraphKey {
                entry: importer.to_path_buf(),
                url_identity: Some(identity.to_owned()),
            })
        {
            return Some(Arc::clone(graph));
        }
        if self.graph.module(importer).is_some() {
            return Some(Arc::clone(&self.graph));
        }
        if let Some(graph) = graphs.get(&RuntimeGraphKey {
            entry: importer.to_path_buf(),
            url_identity: None,
        }) {
            return Some(Arc::clone(graph));
        }
        graphs
            .values()
            .find(|graph| graph.module(importer).is_some())
            .cloned()
    }

    fn importer_path(importer: Option<&str>, fallback: &Path) -> Option<PathBuf> {
        let Some(importer) = importer else {
            return Some(fallback.to_path_buf());
        };
        if importer.starts_with("file:") {
            return url::Url::parse(importer).ok()?.to_file_path().ok();
        }
        Some(PathBuf::from(importer))
    }

    /// Preserve Node's distinct module records for file URLs with query/fragment components.
    /// The graph is still built from the canonical disk path; only the requested module-map key
    /// carries the serialized URL identity.
    fn file_url_identity(specifier: &str) -> Option<String> {
        let url = url::Url::parse(specifier).ok()?;
        (url.scheme() == "file" && (url.query().is_some() || url.fragment().is_some()))
            .then(|| url.to_string())
    }

    fn cached_url_graph(&self, identity: &str) -> Option<Arc<ModuleGraph>> {
        self.runtime_url_graphs.lock().ok()?.get(identity).cloned()
    }

    fn cached_request_graph(&self, key: &RuntimeRequestKey) -> Option<Arc<ModuleGraph>> {
        self.runtime_requests.lock().ok()?.get(key).cloned()
    }

    fn retain_request_graph(&self, key: RuntimeRequestKey, graph: &Arc<ModuleGraph>) {
        if let Ok(mut graphs) = self.runtime_requests.lock() {
            graphs.entry(key).or_insert_with(|| Arc::clone(graph));
        }
    }

    fn apply_file_url_identity(mut plan: ModulePlan, identity: Option<&str>) -> ModulePlan {
        if let Some(identity) = identity {
            let requested_key = plan.key.clone();
            plan.key = identity.to_owned();
            if let Some(requested) = plan
                .modules
                .iter_mut()
                .find(|module| module.key == requested_key)
            {
                requested.key = identity.to_owned();
            } else {
                debug_assert!(false, "requested module must be present in its import plan");
            }
        }
        plan
    }

    /// The actor module `specifier` names, loaded from disk when the snapshot lacks it.
    ///
    /// Only the actor worker entry asks this, and only for the absolute names `spawnActor`
    /// admits; the referrer for resolution is the named file itself, never the worker entry's
    /// virtual path under `.kiln/builtins`. `None` leaves the request to the snapshot's answer.
    fn load_actor_module(
        &self,
        graph: &ModuleGraph,
        importer: &Path,
        request: &ModuleLoadRequest,
    ) -> Option<Result<ModulePlan, ModuleLoadError>> {
        let node = graph.module(importer)?;
        if node.source != ModuleSource::NodeBuiltin(NodeBuiltin::ActorsWorker) {
            return None;
        }
        let path = if request.specifier.starts_with("file:") {
            url::Url::parse(&request.specifier)
                .ok()
                .and_then(|url| url.to_file_path().ok())?
        } else {
            let path = PathBuf::from(&request.specifier);
            if !path.is_absolute() {
                return None;
            }
            path
        };
        Some(
            self.runtime_graph(graph.project_root(), &path, None)
                .and_then(|built| {
                    built
                        .import_plan_for_request(built.entry(), request.attributes.as_ref())
                        .map_err(|error| ModuleLoadError::Unsupported(error.to_string()))
                }),
        )
    }

    fn runtime_graph(
        &self,
        project_root: &Path,
        path: &Path,
        url_identity: Option<&str>,
    ) -> Result<Arc<ModuleGraph>, ModuleLoadError> {
        let not_found = |message: String| {
            ModuleLoadError::NotFound(format!(
                "could not resolve runtime module `{}`: {message}",
                path.display()
            ))
        };
        // Canonical identity is shared by path and file-URL spellings, and is the identity used
        // by module records. Resolve before consulting the cache for the same reason.
        let entry = std::fs::canonicalize(path).map_err(|error| not_found(error.to_string()))?;
        let key = RuntimeGraphKey {
            entry: entry.clone(),
            url_identity: url_identity.map(str::to_owned),
        };
        if let Some(graph) = self
            .runtime_graphs
            .lock()
            .ok()
            .and_then(|graphs| graphs.get(&key).cloned())
        {
            return Ok(graph);
        }
        let resolver = kiln_module_resolver::NodeRuntimeResolver::new(project_root)
            .map_err(|error| not_found(error.to_string()))?;
        // An absolute ESM name is the file itself, as node's loader treats it: no extension
        // or directory probing.
        let built = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| std::fs::read_to_string(path),
            NodeFrontend,
        )
        .map_err(|error| match &error {
            ModuleGraphError::Resolve { .. }
            | ModuleGraphError::Entry { .. }
            | ModuleGraphError::NonAbsoluteEntry { .. } => not_found(error.to_string()),
            ModuleGraphError::MissingExport { .. }
            | ModuleGraphError::AmbiguousExport { .. }
            | ModuleGraphError::DuplicateExport { .. }
            | ModuleGraphError::UnresolvedImport { .. } => {
                ModuleLoadError::Syntax(error.to_string())
            }
            _ => ModuleLoadError::Unsupported(error.to_string()),
        })?;
        let built = Arc::new(built);
        if let Ok(mut graphs) = self.runtime_graphs.lock() {
            graphs.insert(key, Arc::clone(&built));
        }
        if let Some(identity) = url_identity
            && let Ok(mut graphs) = self.runtime_url_graphs.lock()
        {
            graphs.insert(identity.to_owned(), Arc::clone(&built));
        }
        Ok(built)
    }
}

impl ModuleLoaderService for GraphModuleLoader {
    fn load(&self, request: ModuleLoadRequest) -> Result<ModulePlan, ModuleLoadError> {
        let request_key = RuntimeRequestKey {
            importer: request
                .importer
                .clone()
                .unwrap_or_else(|| self.graph.entry().to_string_lossy().into_owned()),
            specifier: request.specifier.clone(),
        };
        let importer_identity = request
            .importer
            .as_deref()
            .and_then(Self::file_url_identity);
        let importer = Self::importer_path(request.importer.as_deref(), self.graph.entry())
            .ok_or_else(|| {
                ModuleLoadError::NotFound("invalid dynamic import referrer".to_owned())
            })?;
        let file_url_identity = Self::file_url_identity(&request.specifier);
        if let Some(loaded) = self.cached_request_graph(&request_key) {
            let plan = loaded
                .import_plan_for_request(loaded.entry(), request.attributes.as_ref())
                .map_err(|error| ModuleLoadError::Unsupported(error.to_string()))?;
            return Ok(Self::apply_file_url_identity(
                plan,
                file_url_identity.as_deref(),
            ));
        }
        let graph = self
            .graph_holding(&importer, importer_identity.as_deref())
            .ok_or_else(|| {
                ModuleLoadError::NotFound(format!(
                    "importer `{}` is not in the module graph",
                    importer.display()
                ))
            })?;
        let graph = graph.as_ref();
        if let Some(identity) = file_url_identity.as_deref()
            && let Some(loaded) = self.cached_url_graph(identity)
        {
            self.retain_request_graph(request_key, &loaded);
            let plan = loaded
                .import_plan_for_request(loaded.entry(), request.attributes.as_ref())
                .map_err(|error| ModuleLoadError::Unsupported(error.to_string()))?;
            return Ok(Self::apply_file_url_identity(plan, Some(identity)));
        }
        let node = graph
            .module(&importer)
            .expect("graph_holding found the importer");
        if let Some(message) = graph.dynamic_failure(&importer, &request.specifier) {
            return Err(ModuleLoadError::Unsupported(message.to_owned()));
        }
        let dependency = graph
            .dynamic_dependencies(&importer)
            .iter()
            .find(|dependency| dependency.specifier == request.specifier)
            .or_else(|| {
                node.dependencies
                    .iter()
                    .find(|dependency| dependency.specifier == request.specifier)
            });
        // An engine-internal module (`kiln:actors/core`) is addressable only from an engine
        // builtin: a computed specifier cannot slip past the resolver that refuses the literal.
        let is_engine_importer = matches!(node.source, ModuleSource::NodeBuiltin(_));
        if !is_engine_importer
            && let Some(owner) =
                NodeBuiltin::parse(&request.specifier).and_then(NodeBuiltin::internal_owner)
        {
            return Err(ModuleLoadError::Unsupported(format!(
                "`{}` is internal to {owner}",
                request.specifier
            )));
        }
        let target = dependency
            .map(|dependency| dependency.target.clone())
            .or_else(|| graph.declared_target_for_runtime_specifier(&request.specifier))
            .or_else(|| {
                if let Some(builtin) = NodeBuiltin::parse(&request.specifier)
                    .filter(|builtin| !builtin.is_engine_internal() || is_engine_importer)
                {
                    graph.module_map().iter().find_map(|(path, node)| {
                        (node.source == ModuleSource::NodeBuiltin(builtin)).then(|| path.clone())
                    })
                } else {
                    None
                }
            })
            .or_else(|| {
                if request.specifier.starts_with("file:") {
                    return url::Url::parse(&request.specifier)
                        .ok()
                        .and_then(|url| url.to_file_path().ok())
                        .filter(|path| graph.module_map().contains_key(path));
                }
                let direct = PathBuf::from(&request.specifier);
                if graph.module_map().contains_key(&direct) {
                    return Some(direct);
                }
                if let Some(parent) = importer.parent() {
                    let relative = parent.join(&request.specifier);
                    if graph.module_map().contains_key(&relative) {
                        return Some(relative);
                    }
                    for ext in &[".ts", ".js", ".json", "/index.ts", "/index.js"] {
                        let candidate = parent.join(format!("{}{ext}", request.specifier));
                        if graph.module_map().contains_key(&candidate) {
                            return Some(candidate);
                        }
                    }
                }
                None
            });
        let Some(target) = target else {
            if let Some(answer) = self.load_actor_module(graph, &importer, &request) {
                return answer
                    .map(|plan| Self::apply_file_url_identity(plan, file_url_identity.as_deref()));
            }
            // Runtime-created ESM (for example a compiler-generated file) is not represented
            // by an edge in the immutable entry graph. Resolve it from its actual referrer using
            // the same Node resolver and project-root policy as graph construction, then retain
            // the resulting graph for subsequent imports from that module. Never resolve against
            // process cwd: a relative request belongs to the importing module.
            let runtime_specifier = if request.specifier.starts_with("file:") {
                let url = url::Url::parse(&request.specifier).map_err(|error| {
                    ModuleLoadError::NotFound(format!("invalid file URL: {error}"))
                })?;
                // Module records are currently keyed by canonical path. Do not silently merge
                // Node's distinct query/fragment module identities into one record.
                url.to_file_path()
                    .map_err(|()| ModuleLoadError::NotFound("invalid file URL path".into()))?
                    .to_string_lossy()
                    .into_owned()
            } else {
                request.specifier.clone()
            };
            let resolved = kiln_module_resolver::NodeRuntimeResolver::new(graph.project_root())
                .and_then(|resolver| resolver.resolve(&runtime_specifier, &importer))
                .map_err(|error| ModuleLoadError::NotFound(error.to_string()))?;
            if resolved.kind == kiln_module_resolver::ResolutionKind::Builtin {
                return Err(ModuleLoadError::NotFound(format!(
                    "runtime builtin `{}` is not present in the module graph",
                    request.specifier
                )));
            }
            let loaded = self.runtime_graph(
                graph.project_root(),
                &resolved.path,
                file_url_identity.as_deref(),
            )?;
            let plan = loaded
                .import_plan_for_request(loaded.entry(), request.attributes.as_ref())
                .map_err(|error| ModuleLoadError::Unsupported(error.to_string()))?;
            // Retain only the first successful resolution for this request identity, as Node's
            // module cache does. Different query/fragment URLs remain separate keys.
            self.retain_request_graph(request_key, &loaded);
            return Ok(Self::apply_file_url_identity(
                plan,
                file_url_identity.as_deref(),
            ));
        };
        let plan = graph
            .import_plan_for_request(&target, request.attributes.as_ref())
            .map_err(|error| ModuleLoadError::Unsupported(error.to_string()))?;
        Ok(Self::apply_file_url_identity(
            plan,
            file_url_identity.as_deref(),
        ))
    }
}

/// The realm's module map as the graph executor addresses it: by path, in the realm the graph's
/// modules run in. The map itself is the session's, so dynamic `import()` in any of the graph's
/// module bodies reads and writes the same records.
#[derive(Clone)]
struct Records {
    map: SharedModuleMap,
    /// The static graph's epoch does not change when resetModules advances the dynamic-import
    /// epoch. Existing graph bindings and namespace cells remain tied to this generation.
    generation: u64,
}

impl Records {
    const REALM: RealmId = RealmId::PRIMARY;

    fn status(&self, path: &Path) -> Option<ModuleStatus> {
        self.map
            .borrow()
            .status_in_generation(Self::REALM, self.generation, &module_key(path))
    }

    fn is_evaluated(&self, path: &Path) -> bool {
        matches!(self.status(path), Some(ModuleStatus::Evaluated))
    }

    fn is_started(&self, path: &Path) -> bool {
        matches!(
            self.status(path),
            Some(ModuleStatus::Evaluating | ModuleStatus::Evaluated | ModuleStatus::Failed(_))
        )
    }

    fn exports(&self, path: &Path) -> Option<std::rc::Rc<HashMap<String, Value>>> {
        self.map
            .borrow()
            .exports_in_generation(Self::REALM, self.generation, &module_key(path))
    }

    fn begin(&self, path: &Path) {
        self.map
            .borrow_mut()
            .begin_in_generation(Self::REALM, self.generation, &module_key(path));
    }

    fn publish(&self, path: &Path, exports: HashMap<String, Value>) {
        self.map.borrow_mut().publish_in_generation(
            Self::REALM,
            self.generation,
            &module_key(path),
            exports,
        );
    }

    fn commonjs_namespace_ready(&self, path: &Path) -> bool {
        self.map.borrow().commonjs_namespace_ready_in_generation(
            Self::REALM,
            self.generation,
            &module_key(path),
        )
    }

    fn commonjs_namespace_failure(&self, path: &Path) -> Option<Value> {
        self.map.borrow().commonjs_namespace_failure_in_generation(
            Self::REALM,
            self.generation,
            &module_key(path),
        )
    }

    fn fail_commonjs_namespace(&self, path: &Path, value: Value) {
        self.map.borrow_mut().fail_commonjs_namespace_in_generation(
            Self::REALM,
            self.generation,
            &module_key(path),
            value,
        );
    }

    fn publish_commonjs_namespace(&self, path: &Path, exports: HashMap<String, Value>) {
        self.map
            .borrow_mut()
            .publish_commonjs_namespace_in_generation(
                Self::REALM,
                self.generation,
                &module_key(path),
                exports,
            );
    }

    fn pending_cell(&self, path: &Path, name: &str) -> Cell {
        self.map.borrow_mut().pending_cell_in_generation(
            Self::REALM,
            self.generation,
            &module_key(path),
            name,
        )
    }

    fn namespace(&self, path: &Path) -> Value {
        self.map.borrow_mut().namespace_in_generation(
            Self::REALM,
            self.generation,
            &module_key(path),
        )
    }

    /// What an importer of `imported` from the finished module `path` links to: its namespace
    /// object, or the binding (read through its cell when `read_cells`).
    fn linked(&self, path: &Path, imported: &str, read_cells: bool) -> Option<Value> {
        if imported == kiln_ir::NAMESPACE_IMPORT {
            return Some(self.namespace(path));
        }
        let exports = self.exports(path)?;
        let value = lookup_export(&exports, imported)?;
        Some(if read_cells {
            read_through_cell(&value)
        } else {
            value
        })
    }
}

impl GraphExecutor<'_> {
    /// Bind one import a module makes from its own specifier, or `None` if it cannot be bound.
    ///
    /// `None` is not a failure of this function; it is the honest answer for a name whose origin
    /// is a module that has not run. That happens only inside an import cycle, where the value
    /// genuinely does not exist yet, and answering `undefined` there would be a wrong answer
    /// where a refusal is a true one.
    fn self_import(
        &self,
        path: &Path,
        import: &kiln_ir::ImportBinding,
        records: &Records,
    ) -> Option<ModuleSelfImport> {
        if import.imported == kiln_ir::NAMESPACE_IMPORT {
            // Sorted, because that is the order a namespace holds its names in and the object is
            // built once and only ever refreshed in place afterwards.
            let mut names = self.resolved_export_names(path)?;
            names.sort();
            let mut exports = Vec::with_capacity(names.len());
            for name in names {
                let source = self.self_export(path, &name, records)?;
                exports.push((name, source));
            }
            return Some(ModuleSelfImport::Namespace {
                import_slot: import.local_slot,
                exports,
            });
        }
        match self.self_export(path, &import.imported, records)? {
            ModuleSelfExport::Slot(export_slot) => Some(ModuleSelfImport::Binding {
                import_slot: import.local_slot,
                export_slot,
            }),
            // The name is this module's to export but another module's to hold, which is what
            // `export { x } from './other.js'` makes it. That module has finished, so the value
            // is a value like any other and there is nothing to keep live.
            ModuleSelfExport::Settled(_) => None,
        }
    }

    /// The cell a back-edge import binds to, created on first demand in the provider's record.
    ///
    /// The importer is running before the module that owns the binding, so there is no value to
    /// hand over. What can be handed over is the *place* the value will arrive in, which is what
    /// an import slot holds anyway -- the interpreter boxes every import precisely so that an
    /// import is a live view of the exporter's binding rather than a copy of it. The module map
    /// fills the cell when the owning module publishes.
    ///
    /// An ambiguous or absent name has no binding to point at, and is refused by name, because a
    /// cycle that cannot be bound is a smaller thing than a cycle that cannot be built.
    fn cycle_cell(
        &self,
        importer: &Path,
        provider: &Path,
        import: &kiln_ir::ImportBinding,
        records: &Records,
    ) -> Result<Cell, ModuleEvaluationError> {
        let namespace_cell = |module: &Path| {
            if records.is_evaluated(module) {
                std::rc::Rc::new(std::cell::RefCell::new(records.namespace(module)))
            } else {
                records.pending_cell(module, kiln_ir::NAMESPACE_IMPORT)
            }
        };
        if import.imported == kiln_ir::NAMESPACE_IMPORT {
            return Ok(namespace_cell(provider));
        }
        let (origin, local_slot) = match self.resolved_export(provider, &import.imported) {
            Some(ResolvedExport::Local { module, local_slot }) => (module.clone(), *local_slot),
            Some(ResolvedExport::CommonJsNamed { module, .. }) => (module.clone(), 0),
            Some(ResolvedExport::Namespace { module }) => return Ok(namespace_cell(module)),
            Some(ResolvedExport::Ambiguous) => {
                return Err(ModuleGraphError::AmbiguousExport {
                    importer: importer.to_path_buf(),
                    provider: provider.to_path_buf(),
                    name: import.imported.clone(),
                }
                .into());
            }
            None => {
                return Err(ModuleGraphError::MissingExport {
                    importer: importer.to_path_buf(),
                    provider: provider.to_path_buf(),
                    name: import.imported.clone(),
                }
                .into());
            }
        };
        // The resolution names a slot; the finished module will publish names. The name that
        // slot is exported under is what bridges the two, and it is the key the cell waits on.
        let exported = self
            .module(&origin)
            .and_then(|node| {
                node.program
                    .exports
                    .iter()
                    .find(|export| export.local_slot == local_slot)
            })
            .map(|export| export.exported.clone())
            .ok_or_else(|| ModuleGraphError::MissingExport {
                importer: importer.to_path_buf(),
                provider: provider.to_path_buf(),
                name: import.imported.clone(),
            })?;
        Ok(records.pending_cell(&origin, &exported))
    }

    /// Where one name exported by `module` gets its value, from `module`'s own point of view.
    fn self_export(&self, path: &Path, name: &str, records: &Records) -> Option<ModuleSelfExport> {
        match self.resolved_export(path, name)? {
            // The one case that has to stay live: this module's own binding, which the body it
            // is about to run creates.
            ResolvedExport::Local { module, local_slot } if module == path => {
                Some(ModuleSelfExport::Slot(*local_slot))
            }
            ResolvedExport::Local { module, local_slot } => {
                // Another module's binding, reached through a re-export. It has already run, so
                // the value is settled -- but the resolution names a *slot* and the finished
                // module publishes *names*, so the name that slot was exported under is what
                // bridges the two.
                let exported = self
                    .module(module)?
                    .program
                    .exports
                    .iter()
                    .find(|export| export.local_slot == *local_slot)?;
                let value = records.exports(module)?.get(&exported.exported)?.clone();
                Some(ModuleSelfExport::Settled(value))
            }
            ResolvedExport::Namespace { module } => records
                .is_evaluated(module)
                .then(|| ModuleSelfExport::Settled(records.namespace(module))),
            ResolvedExport::CommonJsNamed { .. } | ResolvedExport::Ambiguous => None,
        }
    }

    /// Execute the graph in deterministic module order, loading CommonJS dependencies only when
    /// their `require` call runs, and return the entry's value.
    ///
    /// The returned `steps` is the total across the graph; its `value` is the entry module's
    /// result. Side-effect-only dependencies are present in `order` and are therefore evaluated.
    pub fn execute<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        self.execute_mode(host, limits, capabilities, false)
    }

    /// Turn this graph into the plan one `import()` needs, with the entry as the module asked for.
    ///
    /// Nothing is executed: the machine that is *already running* when `import()` evaluates has
    /// to run these bodies itself, because a module and its importer share one realm -- one
    /// microtask queue, one set of intrinsics, one heap. So the graph's job here is only the part
    /// the machine cannot do: resolve, link, and say in what order.
    ///
    /// Each planned module's program ends by handing back an array of its own exports, which the
    /// plan names; every other link is by module key and export name against the realm's module
    /// map, so a module already evaluated -- by the static graph or by another `import()` -- is
    /// linked to rather than run again.
    pub fn import_plan(&self) -> Result<ModulePlan, ModuleEvaluationError> {
        let static_reachable = self.statically_reachable();
        let ordered_paths: Vec<_> = self
            .module_order()
            .iter()
            .filter(|path| static_reachable.contains(*path))
            .cloned()
            .collect();
        let evaluation_components = self.evaluation_components(&ordered_paths);
        let mut modules = Vec::new();
        for path in &ordered_paths {
            let node = self
                .module(path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
            let (body, bindings, reexports) = match node.source {
                // Evaluated by the CommonJS loader, through the cache `require` uses.
                ModuleSource::CommonJs | ModuleSource::Json => {
                    (PlannedBody::Host, Vec::new(), Vec::new())
                }
                ModuleSource::Asset => {
                    let asset = node
                        .data
                        .as_deref()
                        .ok_or_else(|| ModuleGraphError::Source {
                            path: path.clone(),
                            message: "asset module has no path metadata".to_owned(),
                        })?;
                    (
                        PlannedBody::Exports(vec![(
                            "default".to_owned(),
                            Value::String(asset.to_owned().into()),
                        )]),
                        Vec::new(),
                        Vec::new(),
                    )
                }
                ModuleSource::Source | ModuleSource::NodeBuiltin(_) => {
                    let bindings = self.plan_bindings(path, node)?;
                    // The linker's answer for every visible name: one that resolves to a binding
                    // of this module is handed back by its body, in this order; every other name
                    // (a re-export, a star export, `export * as ns`) links to the record of the
                    // module that holds it. An ambiguous star export has no value, and leaving it
                    // out keeps the namespace honest about what it holds.
                    let mut own: Vec<(String, u32)> = Vec::new();
                    let mut reexports = Vec::new();
                    for (name, resolution) in
                        self.export_resolutions().get(path).into_iter().flatten()
                    {
                        match resolution {
                            ResolvedExport::Local { module, local_slot } if module == path => {
                                own.push((name.clone(), *local_slot));
                            }
                            resolution => {
                                if let Some(binding) = self.plan_resolution(resolution) {
                                    reexports.push((name.clone(), binding));
                                }
                            }
                        }
                    }
                    let program = export_epilogue(
                        &node.program.program,
                        own.iter().map(|(_, slot)| *slot).collect(),
                    );
                    (
                        PlannedBody::Source {
                            program: std::rc::Rc::new(program),
                            exports: own.into_iter().map(|(name, _)| name).collect(),
                            engine_private: matches!(node.source, ModuleSource::NodeBuiltin(_)),
                        },
                        bindings,
                        reexports,
                    )
                }
            };
            modules.push(PlannedModule {
                key: module_key(path),
                body,
                bindings,
                reexports,
                dependencies: node
                    .dependencies
                    .iter()
                    .filter(|dependency| static_reachable.contains(&dependency.target))
                    .map(|dependency| module_key(&dependency.target))
                    .collect(),
                evaluation_component: evaluation_components[path],
            });
        }
        Ok(ModulePlan {
            key: module_key(self.entry()),
            modules,
        })
    }

    /// Assign strongly-connected component identities to the static dependency closure. The
    /// first pass follows dependency edges; the reverse pass groups exactly the modules whose
    /// bodies must start together under the ECMAScript module evaluation algorithm.
    fn evaluation_components(&self, ordered_paths: &[PathBuf]) -> HashMap<PathBuf, u32> {
        let included: HashSet<_> = ordered_paths.iter().cloned().collect();
        let mut visited = HashSet::new();
        let mut finished = Vec::with_capacity(ordered_paths.len());
        for root in ordered_paths {
            if visited.contains(root) {
                continue;
            }
            let mut stack = vec![(root.clone(), false)];
            while let Some((path, expanded)) = stack.pop() {
                if expanded {
                    finished.push(path);
                    continue;
                }
                if !visited.insert(path.clone()) {
                    continue;
                }
                stack.push((path.clone(), true));
                if let Some(node) = self.module(&path) {
                    for dependency in node.dependencies.iter().rev() {
                        if included.contains(&dependency.target)
                            && !visited.contains(&dependency.target)
                        {
                            stack.push((dependency.target.clone(), false));
                        }
                    }
                }
            }
        }

        let mut reverse: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for path in ordered_paths {
            if let Some(node) = self.module(path) {
                for dependency in &node.dependencies {
                    if included.contains(&dependency.target) {
                        reverse
                            .entry(dependency.target.clone())
                            .or_default()
                            .push(path.clone());
                    }
                }
            }
        }

        let mut components = HashMap::with_capacity(ordered_paths.len());
        let mut component = 0_u32;
        for root in finished.into_iter().rev() {
            if components.contains_key(&root) {
                continue;
            }
            let mut stack = vec![root];
            while let Some(path) = stack.pop() {
                if components.contains_key(&path) {
                    continue;
                }
                components.insert(path.clone(), component);
                if let Some(importers) = reverse.get(&path) {
                    stack.extend(importers.iter().cloned());
                }
            }
            component = component.saturating_add(1);
        }
        components
    }

    /// Where each import of the source module at `path` comes from, by module key and name.
    fn plan_bindings(
        &self,
        path: &Path,
        node: &ModuleNode,
    ) -> Result<Vec<(u32, ModuleBinding)>, ModuleEvaluationError> {
        let mut bindings = Vec::with_capacity(node.program.imports.len());
        for import in &node.program.imports {
            let dependency = node
                .dependencies
                .iter()
                .find(|dependency| dependency.specifier == import.source)
                .ok_or_else(|| ModuleGraphError::UnresolvedImport {
                    importer: path.to_path_buf(),
                    source: import.source.clone(),
                })?;
            // The only import a module can have from itself that this linker binds is the
            // placeholder a re-export desugars to, whose name no source can write; the linker
            // resolves the export it stands for straight through to its origin. Any *other*
            // self-import would be read, so it is refused rather than left holding `undefined`.
            let binding = if dependency.target == path {
                if !import.local.starts_with(kiln_ir::REEXPORT_LOCAL_PREFIX) {
                    return Err(ModuleGraphError::Source {
                        path: path.to_path_buf(),
                        message: format!(
                            "module imports `{}` from itself, which this linker cannot bind",
                            import.imported
                        ),
                    }
                    .into());
                }
                Some(ModuleBinding::Ready(Value::Undefined))
            } else if import.imported == kiln_ir::NAMESPACE_IMPORT {
                Some(ModuleBinding::Namespace {
                    module: module_key(&dependency.target),
                })
            } else {
                match self.resolved_export(&dependency.target, &import.imported) {
                    Some(ResolvedExport::Ambiguous) | None => None,
                    Some(resolution) => self.plan_resolution(resolution),
                }
            };
            let binding = binding.ok_or_else(|| ModuleGraphError::MissingExport {
                importer: path.to_path_buf(),
                provider: dependency.target.clone(),
                name: import.imported.clone(),
            })?;
            bindings.push((import.local_slot, binding));
        }
        Ok(bindings)
    }

    /// A resolved export as the module-map binding it names: the origin module and the name
    /// the origin exports it under, or the namespace of a module.
    fn plan_resolution(&self, resolution: &ResolvedExport) -> Option<ModuleBinding> {
        match resolution {
            ResolvedExport::Local { module, local_slot } => {
                let name = if *local_slot == 0
                    && self
                        .module(module)
                        .is_some_and(|node| node.source == ModuleSource::CommonJs)
                {
                    // CommonJS's ESM default is virtual slot zero: the loader publishes
                    // `module.exports` under "default", but there is no HIR export row for
                    // `exported_name` to find. Dynamic import plans still bind by the published
                    // module-map key, so carry the same bridge used by the static graph linker.
                    "default".to_owned()
                } else {
                    self.exported_name(module, *local_slot)?
                };
                Some(ModuleBinding::Export {
                    module: module_key(module),
                    name,
                })
            }
            ResolvedExport::Namespace { module } => Some(ModuleBinding::Namespace {
                module: module_key(module),
            }),
            ResolvedExport::CommonJsNamed { module, property } => Some(ModuleBinding::Export {
                module: module_key(module),
                name: property.clone(),
            }),
            ResolvedExport::Ambiguous => None,
        }
    }

    /// The name `module` publishes the binding in `local_slot` under: the bridge between a
    /// resolution, which names a slot, and a module record, which holds names.
    fn exported_name(&self, module: &Path, local_slot: u32) -> Option<String> {
        self.module(module)?
            .program
            .exports
            .iter()
            .find(|export| {
                export.local_slot == local_slot
                    && !export.exported.starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
            })
            .map(|export| export.exported.clone())
    }

    pub fn import_plan_for(
        &self,
        entry: impl AsRef<Path>,
    ) -> Result<ModulePlan, ModuleEvaluationError> {
        self.0.rooted_at(entry)?.import_plan()
    }

    /// Build a plan for a runtime request after applying its import-attribute policy.
    ///
    /// Keeping validation beside rooted-plan construction prevents a loader from reusing a
    /// source module for a request that requires a file asset. Hosts retain control over how
    /// graph errors are classified at their boundary.
    pub fn import_plan_for_request(
        &self,
        entry: impl AsRef<Path>,
        attributes: Option<&ImportAttributes>,
    ) -> Result<ModulePlan, ModuleEvaluationError> {
        let entry = entry.as_ref();
        let node = self
            .module(entry)
            .ok_or_else(|| ModuleGraphError::InternalMissingModule {
                path: entry.to_path_buf(),
            })?;
        if node.source == ModuleSource::Asset
            && attributes.and_then(|attributes| attributes.type_.as_deref()) != Some("file")
        {
            return Err(ModuleGraphError::Source {
                path: node.path.clone(),
                message: "file asset imports require `with: { type: 'file' }`".to_owned(),
            }
            .into());
        }
        if let Some(ImportAttributes { type_: Some(kind) }) = attributes {
            if kind != "file" {
                return Err(ModuleGraphError::Source {
                    path: node.path.clone(),
                    message: "unsupported import attribute `type`".to_owned(),
                }
                .into());
            }
            if node.source != ModuleSource::Asset {
                return Err(ModuleGraphError::Source {
                    path: node.path.clone(),
                    message: format!("module `{}` is not a file asset", node.path.display()),
                }
                .into());
            }
        }
        self.import_plan_for(entry)
    }

    /// Every name the module at `path` exports, as the binding an importer links to, given the
    /// values its own export slots hold once its body has run.
    fn module_exports(
        &self,
        path: &Path,
        raw: &[ModuleExportValue],
        records: &Records,
    ) -> HashMap<String, Value> {
        // A `export * from` entry stands for the provider's names rather than for one of
        // its own. Local exports are inserted first so that a name this module exports
        // itself wins over the same name arriving through a star, which is what the
        // specification says and what a barrel file that overrides one name relies on.
        let mut exports: HashMap<String, Value> = raw
            .iter()
            .filter(|export| !export.name.starts_with(kiln_ir::STAR_REEXPORT_PREFIX))
            .map(|export| (export.name.clone(), export.value.clone()))
            .collect();
        for star in raw
            .iter()
            .filter(|export| export.name.starts_with(kiln_ir::STAR_REEXPORT_PREFIX))
        {
            let expanded = kiln_interpreter::read_through_cell(&star.value);
            for (name, value) in kiln_interpreter::namespace_entries(&expanded) {
                // `export *` never forwards a default: a module's default is its own.
                if name != "default"
                    && matches!(
                        self.export_resolutions()
                            .get(path)
                            .and_then(|resolutions| resolutions.get(&name)),
                        Some(
                            ResolvedExport::Local { .. }
                                | ResolvedExport::Namespace { .. }
                                | ResolvedExport::CommonJsNamed { .. }
                        )
                    )
                {
                    exports.entry(name).or_insert(value);
                }
            }
        }
        if let Some(resolutions) = self.export_resolutions().get(path) {
            for (name, resolution) in resolutions {
                if name == "default" || exports.contains_key(name) {
                    continue;
                }
                match resolution {
                    ResolvedExport::Local {
                        module: origin,
                        local_slot,
                    } => {
                        if let Some(origin_exports) = records.exports(origin) {
                            let origin_node = self.module(origin);
                            let exported_name = origin_node
                                .and_then(|node| {
                                    node.program.exports.iter().find(|export| {
                                        export.local_slot == *local_slot
                                            && !export
                                                .exported
                                                .starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
                                    })
                                })
                                .map(|export| export.exported.as_str())
                                .unwrap_or(name);
                            if let Some(val) = origin_exports.get(exported_name) {
                                exports.insert(name.clone(), val.clone());
                            }
                        } else {
                            let origin_node = self.module(origin);
                            let origin_exported = origin_node
                                .and_then(|node| {
                                    node.program.exports.iter().find(|export| {
                                        export.local_slot == *local_slot
                                            && !export
                                                .exported
                                                .starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
                                    })
                                })
                                .map(|export| export.exported.clone())
                                .unwrap_or_else(|| name.clone());
                            let cell = records.pending_cell(origin, &origin_exported);
                            exports.insert(name.clone(), Value::Cell(cell));
                        }
                    }
                    ResolvedExport::CommonJsNamed {
                        module: origin,
                        property,
                    } => {
                        if let Some(origin_exports) = records.exports(origin) {
                            if let Some(val) = origin_exports.get(property) {
                                exports.insert(name.clone(), val.clone());
                            }
                        }
                    }
                    // The module's own namespace object, filled in place if it has not finished.
                    ResolvedExport::Namespace { module: origin } => {
                        exports.insert(name.clone(), records.namespace(origin));
                    }
                    ResolvedExport::Ambiguous => {}
                }
            }
        }
        exports
    }

    /// Report a module that already failed -- in a dynamic `import()` that caught the error --
    /// the way its own evaluation reported it: as the same value, uncaught, in this realm.
    ///
    /// The value belongs to the realm and the report is built by a machine, so a machine is what
    /// raises it: a two-instruction body that throws the one value it imports.
    #[allow(clippy::too_many_arguments)]
    fn rethrow<H: HostOps + ?Sized>(
        &self,
        path: &Path,
        error: Value,
        host: &mut H,
        limits: ExecutionLimits,
        runtime: &mut RealmRuntime,
        session: &mut ModuleRealmSession,
        capabilities: &ExecutionCapabilities,
        loader: &mut dyn CommonJsModuleLoader,
    ) -> ModuleEvaluationError {
        let program = Program {
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: Vec::new(),
            code: vec![Instruction::LoadCell(0), Instruction::Throw],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            source: "".into(),
        };
        let module = kiln_ir::ModuleProgram {
            strictness: kiln_ir::Strictness::always(&program),
            program,
            dependencies: Vec::new(),
            imports: vec![kiln_ir::ImportBinding {
                source: String::new(),
                imported: "default".to_owned(),
                local: "error".to_owned(),
                local_slot: 0,
            }],
            exports: Vec::new(),
            link_facts: kiln_ir::ModuleLinkFacts::default(),
        };
        let error = match execute_node_module_in_session_with_loader(
            &module,
            &[error],
            &[],
            host,
            limits,
            runtime,
            session,
            capabilities.clone(),
            false,
            loader,
            true,
            Some(path.to_string_lossy().as_ref()),
        ) {
            Err(error) => error,
            Ok(_) => ExecutionError::Host {
                instruction: 0,
                message: format!("module `{}` failed earlier", path.display()),
            },
        };
        ModuleEvaluationError::Execution {
            module: path.to_path_buf(),
            error,
        }
    }

    /// Execute this graph with the realm-owned Tokio Node host installed.
    ///
    /// The graph and linker are identical to [`Self::execute`]; only the explicit execution
    /// capability differs. This prevents importing a Node-shaped package from silently granting
    /// host I/O to deterministic evaluation.
    pub fn execute_node<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        self.execute_node_with_clock(host, limits, capabilities, ClockCapability::Virtual)
    }

    /// Execute this graph with Node host APIs and an explicit clock policy. Node I/O remains
    /// available under either clock; wall-clock scheduling is opt-in for workloads that need to
    /// observe elapsed real time.
    pub fn execute_node_with_clock<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        clock: ClockCapability,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        self.execute_mode_with_clock(host, limits, capabilities, true, clock)
    }

    /// [`Self::execute`] under an explicit clock policy, still without the Node adapter.
    pub fn execute_with_clock<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        clock: ClockCapability,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        self.execute_mode_with_clock(host, limits, capabilities, false, clock)
    }

    fn execute_mode<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        node: bool,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        self.execute_mode_with_clock(host, limits, capabilities, node, ClockCapability::Virtual)
    }

    fn execute_mode_with_clock<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        node: bool,
        clock: ClockCapability,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        // Every realm this graph runs in -- the one the modules share and any a module makes --
        // runs the shared built-ins first, in the set this mode has (see `realm_bootstrap`).
        let capabilities = capabilities.with_realm_bootstrap(realm_bootstrap(node));
        // Stack frames name modules relative to the project root, as the native backend does.
        let capabilities = capabilities.with_display_root(self.project_root().to_string_lossy());
        // A Node program can start workers, each a realm running another of this graph's
        // modules on a thread of its own (realms issue 06).
        let capabilities = if node {
            let workers = GraphWorkers {
                graph: (*self.0).clone(),
                limits,
                capabilities: capabilities.clone(),
                clock,
            };
            capabilities.with_worker_host(Arc::new(workers))
        } else {
            capabilities
        };
        // A program run on a main thread numbers its workers from 1, as a node process does.
        kiln_runtime::workers::begin_program();
        let outcome = self.execute_mode_in(host, limits, capabilities, node, clock);
        // The realm is over, and so are the workers it started and did not wait for.
        kiln_runtime::workers::finish_children();
        // And so are its signal subscriptions (realms issue 23): a daemon or a test runner goes
        // on to other programs, and a signal must not be taken on behalf of one that has ended.
        kiln_runtime::signals::release_thread();
        outcome
    }

    fn execute_mode_in<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        node: bool,
        clock: ClockCapability,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        // Allocated for every CommonJS node before any body runs. This is the module cache's
        // identity seam: every require edge names the same mutable export cell.
        let commonjs_cache: HashMap<PathBuf, Cell> = self
            .module_map()
            .iter()
            .filter(|(_, node)| node.source == ModuleSource::CommonJs)
            .map(|(path, _)| {
                (
                    path.clone(),
                    std::rc::Rc::new(std::cell::RefCell::new(ordinary_object([]))),
                )
            })
            .collect();
        let mut total_steps = 0_u64;
        let mut entry_result = None;
        let realm_capabilities = RealmCapabilities {
            clock,
            ..RealmCapabilities::default()
        };
        let mut runtime = RealmRuntime::with_capabilities(realm_capabilities).map_err(|error| {
            ModuleEvaluationError::Execution {
                module: self.entry().to_path_buf(),
                error: ExecutionError::Host {
                    instruction: 0,
                    message: error.to_string(),
                },
            }
        })?;
        if let Some(fs) = host.clone_fs() {
            runtime.set_fs(fs);
        }
        // One realm for the whole graph in every mode: a module that writes `globalThis.x`
        // is read by its importer, and a dependency's timers run in the entry's loop rather
        // than before the entry body. Until 2026-09-19 the plain mode gave each module its own
        // global object, which neither node nor the native backend does; the adapter (and the
        // host clock) is the only thing `--node` adds now.
        let mut node_session = Some(if node {
            ModuleRealmSession::default()
        } else {
            ModuleRealmSession::without_adapter()
        });
        if node {
            let module_map = node_session
                .as_ref()
                .expect("Node graph has a realm session")
                .module_map();
            let mut module_map = module_map.borrow_mut();
            for path in self.module_map().keys() {
                module_map.add_runner_candidate(RealmId::PRIMARY, &module_key(path));
            }
        }
        // The realm's module map: one record per module, holding its evaluation state and its
        // exports by name. Dynamic `import()` in any module body reads and writes the same map,
        // so whichever reaches a module first is its only evaluation. A binding a cycle forces
        // somebody to hold before its module has run is a dead-zone cell in that module's
        // record, keyed by the *origin* of the name, because a re-export chain can cross a cycle
        // several times and every crossing has to land on the same cell.
        let records = Records {
            map: node_session
                .as_ref()
                .expect("the graph owns a session")
                .module_map(),
            generation: 0,
        };
        let cache_targets = commonjs_cache
            .iter()
            .map(|(path, cell)| (std::rc::Rc::as_ptr(cell), path.clone()))
            .collect();
        let mut commonjs_loader = CommonJsLoader {
            graph: self,
            commonjs_cache: &commonjs_cache,
            cache_targets,
            module_objects: HashMap::new(),
            records: records.clone(),
            total_steps: &mut total_steps as *mut _,
            entry_result: &mut entry_result as *mut _,
            host: host as *mut _,
            runtime: &mut runtime as *mut _,
            node_session: &mut node_session as *mut _,
            limits,
            capabilities: capabilities.clone(),
            node,
            pending_thrown: None,
            ensuring_targets: HashSet::new(),
        };

        // The shared TypeScript built-ins are not run here: every machine in the session realizes
        // the realm before its first instruction, and the first one to run bootstraps it through
        // the capability installed above.

        let static_reachable = self.statically_reachable();
        for path in self.module_order().iter() {
            if !static_reachable.contains(path) {
                continue;
            }
            let module_node = self
                .module(path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
            // A module something else already reached -- a dynamic `import()` in a module that
            // ran earlier, or a `require` -- has had its one evaluation, and this importer links
            // to its record. One that threw rethrows the same value, as the specification has
            // every later evaluation of it do.
            match records.status(path) {
                Some(ModuleStatus::Evaluated | ModuleStatus::Evaluating) => {
                    if path == self.entry() && entry_result.is_none() {
                        entry_result = Some(ExecutionResult {
                            value: Value::Undefined,
                            steps: 0,
                            process_exit_code: None,
                        });
                    }
                    continue;
                }
                Some(ModuleStatus::Failed(error)) => {
                    return Err(self.rethrow(
                        path,
                        error,
                        host,
                        limits,
                        &mut runtime,
                        node_session.as_mut().expect("the graph owns a session"),
                        &capabilities,
                        &mut commonjs_loader,
                    ));
                }
                Some(ModuleStatus::Linked) | None => {}
            }
            if module_node.source == ModuleSource::Json {
                let source =
                    module_node
                        .data
                        .as_deref()
                        .ok_or_else(|| ModuleGraphError::Source {
                            path: path.clone(),
                            message: "JSON module has no captured source text".to_owned(),
                        })?;
                let value = parse_json_module(source).map_err(|error| {
                    ModuleEvaluationError::Execution {
                        module: path.clone(),
                        error,
                    }
                })?;
                records.publish(path, HashMap::from([("default".to_owned(), value)]));
                continue;
            }
            if module_node.source == ModuleSource::Asset {
                let path = module_node
                    .data
                    .as_deref()
                    .ok_or_else(|| ModuleGraphError::Source {
                        path: path.clone(),
                        message: "asset module has no path metadata".to_owned(),
                    })?;
                records.publish(
                    &module_node.path,
                    HashMap::from([("default".to_owned(), Value::String(path.to_owned().into()))]),
                );
                continue;
            }
            if module_node.source == ModuleSource::CommonJs {
                if path == self.entry() {
                    commonjs_loader
                        .execute_target(path, true)
                        .map_err(|error| ModuleEvaluationError::Execution {
                            module: path.clone(),
                            error,
                        })?;
                }
                continue;
            }
            let mut imports = Vec::with_capacity(module_node.program.imports.len());
            let mut self_imports = Vec::new();
            for import in &module_node.program.imports {
                let dependency = module_node
                    .dependencies
                    .iter()
                    .find(|dependency| dependency.specifier == import.source)
                    .ok_or_else(|| ModuleGraphError::UnresolvedImport {
                        importer: path.clone(),
                        source: import.source.clone(),
                    })?;
                // A module importing from itself has no finished exports to take a value from,
                // and it never will: the binding it names is created by the body that is about
                // to run. So the linker hands over the *place* rather than the value, and the
                // machine ties the two together as the body creates them. A re-export
                // placeholder is the one self-import that is never read -- its slot no source
                // can name, and its export the linker resolves straight through to its origin.
                if dependency.target == *path {
                    if import.local.starts_with(kiln_ir::REEXPORT_LOCAL_PREFIX) {
                        imports.push(Value::Undefined);
                        continue;
                    }
                    let bound = self.self_import(path, import, &records);
                    let bound = bound.ok_or_else(|| ModuleGraphError::Source {
                        path: path.clone(),
                        message: format!(
                            "module imports `{}` from itself, which this linker cannot bind",
                            import.imported
                        ),
                    })?;
                    self_imports.push(bound);
                    imports.push(Value::Undefined);
                    continue;
                }
                // In a cycle a name can travel out through one module and come back as this
                // module's own binding: `import { B } from "./b.js"` where b.js says
                // `export { A as B } from "./a.js"` and a.js is this module. The specifier names
                // b.js, but the *binding* is this module's, and it does not exist yet -- the
                // body about to run creates it. That is precisely the shape the self-import
                // machinery already handles, so it is routed there rather than given a second
                // one: the slot starts in its dead zone and the machine ties the two together
                // as the declaration runs.
                if let Some(ResolvedExport::Local {
                    module: origin,
                    local_slot,
                }) = self.resolved_export(&dependency.target, &import.imported)
                    && origin == path
                {
                    self_imports.push(ModuleSelfImport::Binding {
                        import_slot: import.local_slot,
                        export_slot: *local_slot,
                    });
                    imports.push(Value::Undefined);
                    continue;
                }
                if !records.is_started(&dependency.target)
                    && self
                        .module(&dependency.target)
                        .is_some_and(|node| node.source == ModuleSource::CommonJs)
                {
                    commonjs_loader
                        .execute_target(&dependency.target, true)
                        .map_err(|error| ModuleEvaluationError::Execution {
                            module: dependency.target.clone(),
                            error,
                        })?;
                }
                if self
                    .module(&dependency.target)
                    .is_some_and(|node| node.source == ModuleSource::CommonJs)
                {
                    commonjs_loader
                        .prepare_namespace(&dependency.target)
                        .map_err(|error| ModuleEvaluationError::Execution {
                            module: dependency.target.clone(),
                            error,
                        })?;
                }
                // A dependency with no value table has not run, and in an acyclic graph that
                // cannot happen. So this is a back edge: the importer is an ancestor of the
                // module it is importing from, and the binding it wants will not exist until
                // later in this very loop.
                if !records.is_evaluated(&dependency.target) {
                    let cell = self.cycle_cell(path, &dependency.target, import, &records)?;
                    imports.push(Value::Cell(cell));
                    continue;
                }
                let value = match records.linked(&dependency.target, &import.imported, false) {
                    Some(value) => value,
                    None => match self.resolved_export(&dependency.target, &import.imported) {
                        Some(ResolvedExport::Local {
                            module: origin,
                            local_slot,
                        }) => {
                            if let Some(origin_exports) = records.exports(origin) {
                                let origin_node = self.module(origin);
                                let exported_name = origin_node
                                    .and_then(|node| {
                                        node.program.exports.iter().find(|export| {
                                            export.local_slot == *local_slot
                                                && !export
                                                    .exported
                                                    .starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
                                        })
                                    })
                                    .map(|export| export.exported.as_str())
                                    .unwrap_or(&import.imported);
                                lookup_export(&origin_exports, exported_name).ok_or_else(|| {
                                    ModuleGraphError::MissingExport {
                                        importer: path.clone(),
                                        provider: origin.clone(),
                                        name: import.imported.clone(),
                                    }
                                })?
                            } else {
                                let cell =
                                    self.cycle_cell(path, &dependency.target, import, &records)?;
                                imports.push(Value::Cell(cell));
                                continue;
                            }
                        }
                        Some(ResolvedExport::CommonJsNamed {
                            module: origin,
                            property,
                        }) => {
                            if let Some(origin_exports) = records.exports(origin) {
                                lookup_export(&origin_exports, property).ok_or_else(|| {
                                    ModuleGraphError::MissingExport {
                                        importer: path.clone(),
                                        provider: origin.clone(),
                                        name: import.imported.clone(),
                                    }
                                })?
                            } else {
                                let cell =
                                    self.cycle_cell(path, &dependency.target, import, &records)?;
                                imports.push(Value::Cell(cell));
                                continue;
                            }
                        }
                        Some(ResolvedExport::Namespace { module: origin }) => {
                            if records.is_evaluated(origin) {
                                records.namespace(origin)
                            } else {
                                let cell =
                                    self.cycle_cell(path, &dependency.target, import, &records)?;
                                imports.push(Value::Cell(cell));
                                continue;
                            }
                        }
                        Some(ResolvedExport::Ambiguous) => {
                            return Err(ModuleGraphError::AmbiguousExport {
                                importer: path.clone(),
                                provider: dependency.target.clone(),
                                name: import.imported.clone(),
                            }
                            .into());
                        }
                        None => {
                            return Err(ModuleGraphError::MissingExport {
                                importer: path.clone(),
                                provider: dependency.target.clone(),
                                name: import.imported.clone(),
                            }
                            .into());
                        }
                    },
                };
                imports.push(value);
            }

            if total_steps >= limits.max_steps {
                return Err(ModuleEvaluationError::Execution {
                    module: path.clone(),
                    error: ExecutionError::StepLimitExceeded {
                        steps: total_steps,
                        limit: limits.max_steps,
                    },
                });
            }
            let remaining_limits = ExecutionLimits {
                max_steps: limits.max_steps - total_steps,
                ..limits
            };
            let shared_builtin = matches!(module_node.source, ModuleSource::NodeBuiltin(_));
            records.begin(path);
            if shared_builtin {
                node_session
                    .as_mut()
                    .expect("the graph owns a session")
                    .begin_engine_module();
            }
            let result = execute_node_module_in_session_with_loader(
                &module_node.program,
                &imports,
                &self_imports,
                host,
                remaining_limits,
                &mut runtime,
                node_session.as_mut().expect("the graph owns a session"),
                capabilities.clone(),
                path == self.entry(),
                &mut commonjs_loader,
                true,
                Some(path.to_string_lossy().as_ref()),
            );
            if shared_builtin {
                node_session
                    .as_mut()
                    .expect("the graph owns a session")
                    .end_engine_module();
            }
            let result = result.map_err(|error| {
                let error = match error {
                    ExecutionError::StepLimitExceeded { steps, .. } => {
                        ExecutionError::StepLimitExceeded {
                            steps: total_steps.saturating_add(steps),
                            limit: limits.max_steps,
                        }
                    }
                    error => error,
                };
                ModuleEvaluationError::Execution {
                    module: path.clone(),
                    error,
                }
            })?;
            total_steps = total_steps
                .checked_add(result.execution.steps)
                .ok_or(ModuleEvaluationError::StepOverflow)?;
            // The body publishes as soon as it finishes, before the event loop runs what it
            // queued (see `module_evaluated`); this is the same answer for a body that did not.
            if !records.is_evaluated(path) {
                records.publish(path, self.module_exports(path, &result.exports, &records));
            }
            if path == self.entry() {
                entry_result = Some(result.execution);
            }
        }

        let mut result = entry_result.ok_or_else(|| ModuleGraphError::InternalMissingModule {
            path: self.entry().to_path_buf(),
        })?;
        result.steps = total_steps;
        Ok(result)
    }
}
/// The graph-owned half of lazy CommonJS execution.
//
// The interpreter can call back into this loader while `execute_mode` is still holding the
// graph's host, runtime, session, and result state. These pointers are scoped to that one
// synchronous execution, and the loader never escapes it; reentrant callbacks therefore do not
// outlive the pointed-to values.
struct CommonJsLoader<'graph, H: HostOps + ?Sized> {
    graph: &'graph ModuleGraph,
    commonjs_cache: &'graph HashMap<PathBuf, Cell>,
    cache_targets: HashMap<*const std::cell::RefCell<Value>, PathBuf>,
    /// Current wrapper objects are retained while their CommonJS bodies run. A cyclic require
    /// reads `module.exports` here so a replacement published before the back-edge is visible.
    module_objects: HashMap<PathBuf, Value>,
    /// The realm's module map; a CommonJS module's evaluation state is its record's.
    records: Records,
    total_steps: *mut u64,
    entry_result: *mut Option<ExecutionResult>,
    host: *mut H,
    runtime: *mut RealmRuntime,
    node_session: *mut Option<ModuleRealmSession>,
    limits: ExecutionLimits,
    capabilities: ExecutionCapabilities,
    node: bool,
    pending_thrown: Option<Value>,
    ensuring_targets: HashSet<PathBuf>,
}

impl<H: HostOps + ?Sized> CommonJsLoader<'_, H> {
    fn ensure_target_value(
        &mut self,
        target: &Path,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        self.ensure_target_value_inner(target, instruction)
    }

    fn ensure_target_value_inner(
        &mut self,
        target: &Path,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if let Some(cell) = self.commonjs_cache.get(target) {
            self.execute_target(target, false)?;
            return Ok(cell.borrow().clone());
        }
        if let Some(provider) = self.records.exports(target)
            && let Some(value) = provider.get("default")
        {
            return Ok(read_through_cell(value));
        }
        let Some(module_node) = self.graph.module(target) else {
            return Err(ExecutionError::Host {
                instruction,
                message: format!(
                    "CommonJS runtime target `{}` is not in the graph",
                    target.display()
                ),
            });
        };
        if module_node.source == ModuleSource::Json {
            let source = module_node
                .data
                .as_deref()
                .ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: format!(
                        "JSON module `{}` has no captured source text",
                        target.display()
                    ),
                })?;
            let value = parse_json_module(source)?;
            self.records.publish(
                target,
                HashMap::from([("default".to_owned(), value.clone())]),
            );
            return Ok(value);
        }
        if !matches!(module_node.source, ModuleSource::NodeBuiltin(_)) {
            return Err(ExecutionError::Host {
                instruction,
                message: format!(
                    "CommonJS runtime target `{}` has no value",
                    target.display()
                ),
            });
        }
        if !self.ensuring_targets.insert(target.to_path_buf()) {
            return Err(ExecutionError::Host {
                instruction,
                message: format!("cyclic lazy builtin dependency at `{}`", target.display()),
            });
        }
        let result = self.evaluate_ts_builtin_target(target, instruction, module_node);
        self.ensuring_targets.remove(target);
        return result;
    }

    fn evaluate_ts_builtin_target(
        &mut self,
        target: &Path,
        instruction: usize,
        module_node: &ModuleNode,
    ) -> Result<Value, ExecutionError> {
        let program = module_node.program.clone();
        let mut imports = Vec::with_capacity(program.imports.len());
        for import in &program.imports {
            let dependency = module_node
                .dependencies
                .iter()
                .find(|dependency| dependency.specifier == import.source)
                .ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: format!(
                        "lazy builtin `{}` has unresolved import `{}`",
                        target.display(),
                        import.source
                    ),
                })?;
            if !self.records.is_evaluated(&dependency.target)
                && let Some(node) = self.graph.module(&dependency.target)
            {
                if node.source == ModuleSource::CommonJs {
                    self.execute_target(&dependency.target, false)?;
                } else if matches!(node.source, ModuleSource::NodeBuiltin(_)) {
                    self.ensure_target_value(&dependency.target, instruction)?;
                }
            }
            if !self.records.is_evaluated(&dependency.target) {
                return Err(ExecutionError::Host {
                    instruction,
                    message: format!(
                        "lazy builtin dependency `{}` has not been evaluated",
                        dependency.target.display()
                    ),
                });
            }
            imports.push(
                self.records
                    .linked(&dependency.target, &import.imported, true)
                    .ok_or_else(|| ExecutionError::Host {
                        instruction,
                        message: format!(
                            "lazy builtin dependency `{}` has no export `{}`",
                            dependency.target.display(),
                            import.imported
                        ),
                    })?,
            );
        }
        let shared_builtin = matches!(module_node.source, ModuleSource::NodeBuiltin(_));
        self.records.begin(target);
        if shared_builtin {
            unsafe {
                (&mut *self.node_session)
                    .as_mut()
                    .expect("the graph owns a realm session")
                    .begin_engine_module();
            }
        }
        let result = unsafe {
            execute_node_module_in_session_with_loader(
                &program,
                &imports,
                &[],
                &mut *self.host,
                self.limits,
                &mut *self.runtime,
                (&mut *self.node_session)
                    .as_mut()
                    .expect("the graph owns a realm session"),
                self.capabilities.clone(),
                false,
                self,
                false,
                Some(target.to_string_lossy().as_ref()),
            )
        };
        if shared_builtin {
            unsafe {
                (&mut *self.node_session)
                    .as_mut()
                    .expect("the graph owns a realm session")
                    .end_engine_module();
            }
        }
        let result = result?;
        let exports = result
            .exports
            .into_iter()
            .map(|export| (export.name, export.value))
            .collect::<HashMap<_, _>>();
        let value = exports
            .get("default")
            .cloned()
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: format!("lazy builtin `{}` has no default export", target.display()),
            })?;
        if !self.records.is_evaluated(target) {
            self.records.publish(target, exports);
        }
        Ok(read_through_cell(&value))
    }

    /// ESM linking snapshots CommonJS properties once, while require retains the original
    /// object and does not invoke these getters. Read through ordinary bytecode in this realm
    /// so receiver binding, closures, host calls and thrown values follow the usual VM path.
    fn prepare_namespace(&mut self, path: &Path) -> Result<(), ExecutionError> {
        if let Some(value) = self.records.commonjs_namespace_failure(path) {
            self.record_thrown_value(value);
            return Err(ExecutionError::Uncaught {
                instruction: 0,
                error_constructor: None,
                message: "CommonJS namespace getter failed".into(),
                trace: Vec::new(),
                stack: None,
            });
        }
        if self.records.commonjs_namespace_ready(path) {
            return Ok(());
        }
        if !self
            .graph
            .module(path)
            .is_some_and(|node| node.source == ModuleSource::CommonJs)
        {
            return Ok(());
        }
        self.execute_target(path, false)?;
        let value = self
            .commonjs_cache
            .get(path)
            .expect("CommonJS cache exists")
            .borrow()
            .clone();
        let keys: Vec<String> = match &value {
            Value::Object(object) => object
                .borrow()
                .keys()
                .map(|key| key.to_lossy_string())
                .filter(|key| key != "default")
                .collect(),
            Value::Function(function) => function
                .properties
                .borrow()
                .keys()
                .map(|key| key.to_lossy_string())
                .filter(|key| key != "default")
                .collect(),
            _ => Vec::new(),
        };
        // Most CommonJS modules expose only data. Snapshot those without constructing a VM;
        // if any accessor exists, read every key in order because a getter can mutate later keys.
        let data = match &value {
            Value::Object(object) => {
                let object = object.borrow();
                keys.iter()
                    .map(|key| object.get_own_value(key).map(|value| (key.clone(), value)))
                    .collect::<Option<HashMap<_, _>>>()
            }
            Value::Function(function) => {
                let object = function.properties.borrow();
                keys.iter()
                    .map(|key| match object.get_own_property(key) {
                        Some(Property::Value(value)) => Some((key.clone(), value.clone())),
                        _ => None,
                    })
                    .collect::<Option<HashMap<_, _>>>()
            }
            _ => Some(HashMap::new()),
        };
        if let Some(mut exports) = data {
            exports.insert("default".into(), value);
            self.records.publish_commonjs_namespace(path, exports);
            return Ok(());
        }
        let mut module = kiln_ir::ModuleProgram::default();
        module.program.local_count = 1 + keys.len() as u32;
        module.imports.push(kiln_ir::ImportBinding {
            source: String::new(),
            imported: "default".into(),
            local: "exports".into(),
            local_slot: 0,
        });
        for (index, key) in keys.iter().enumerate() {
            module
                .program
                .constants
                .push(kiln_ir::Constant::String(key.as_str().into()));
            module.program.code.extend([
                Instruction::LoadCell(0),
                Instruction::GetProperty(index as u32),
                Instruction::StoreLocal(1 + index as u32),
            ]);
            module.exports.push(kiln_ir::ExportBinding {
                exported: key.clone(),
                local_slot: 1 + index as u32,
            });
        }
        module.program.constants.push(kiln_ir::Constant::Undefined);
        module
            .program
            .code
            .extend([Instruction::Const(keys.len() as u32), Instruction::Return]);
        let total_steps = unsafe { *self.total_steps };
        let limits = ExecutionLimits {
            max_steps: self.limits.max_steps.saturating_sub(total_steps),
            ..self.limits
        };
        let result = unsafe {
            if self.node {
                execute_node_module_in_session_with_loader(
                    &module,
                    &[value.clone()],
                    &[],
                    &mut *self.host,
                    limits,
                    &mut *self.runtime,
                    (&mut *self.node_session).as_mut().expect("Node session"),
                    self.capabilities.clone(),
                    false,
                    self,
                    false,
                    Some(path.to_string_lossy().as_ref()),
                )
            } else {
                execute_module_in_runtime_with_loader(
                    &module,
                    &[value.clone()],
                    &[],
                    &mut *self.host,
                    limits,
                    &mut *self.runtime,
                    self.capabilities.clone(),
                    self,
                    false,
                    Some(path.to_string_lossy().as_ref()),
                )
            }
        }
        .map_err(|error| {
            if let Some(value) = &self.pending_thrown {
                self.records.fail_commonjs_namespace(path, value.clone());
            }
            error
        })?;
        unsafe {
            *self.total_steps = (*self.total_steps)
                .checked_add(result.execution.steps)
                .ok_or_else(|| ExecutionError::Host {
                    instruction: 0,
                    message: "CommonJS namespace step count overflow".into(),
                })?;
        }
        let mut exports = HashMap::from([("default".to_owned(), value)]);
        exports.extend(
            result
                .exports
                .into_iter()
                .map(|entry| (entry.name, entry.value)),
        );
        self.records.publish_commonjs_namespace(path, exports);
        Ok(())
    }

    fn execute_target(&mut self, path: &Path, enter_runtime: bool) -> Result<(), ExecutionError> {
        if self.records.is_started(path) {
            return Ok(());
        }
        let Some(module_node) = self.graph.module(path) else {
            return Err(ExecutionError::Host {
                instruction: 0,
                message: format!("CommonJS target `{}` is not in the graph", path.display()),
            });
        };
        if module_node.source != ModuleSource::CommonJs {
            return Ok(());
        }
        self.records.begin(path);
        let initial_exports = self
            .commonjs_cache
            .get(path)
            .ok_or_else(|| ExecutionError::Host {
                instruction: 0,
                message: format!("CommonJS cache has no entry for `{}`", path.display()),
            })?
            .borrow()
            .clone();
        let mut targets = Vec::with_capacity(module_node.dependencies.len());
        for dependency in &module_node.dependencies {
            // An included worker is a fresh entry: a builtin or JSON dependency used only by
            // that worker has not been evaluated during the parent's startup. Materialize it
            // before building this CommonJS module's fixed require table.
            if !self.commonjs_cache.contains_key(&dependency.target)
                && !self.records.is_evaluated(&dependency.target)
                && self.graph.module(&dependency.target).is_some_and(|node| {
                    matches!(
                        node.source,
                        ModuleSource::NodeBuiltin(_) | ModuleSource::Json
                    )
                })
            {
                self.ensure_target_value(&dependency.target, 0)?;
            }
            let value =
                if let Some(cell) = self.commonjs_cache.get(&dependency.target) {
                    std::rc::Rc::clone(cell)
                } else if let Some(provider) = self.records.exports(&dependency.target) {
                    if self
                        .graph
                        .module(&dependency.target)
                        .is_some_and(|node| match node.source {
                            ModuleSource::Json => true,
                            ModuleSource::NodeBuiltin(_) => true,
                            _ => false,
                        })
                    {
                        let value = provider.get("default").cloned().ok_or_else(|| {
                            ExecutionError::Host {
                                instruction: 0,
                                message: format!(
                                    "Target `{}` has no default",
                                    dependency.target.display()
                                ),
                            }
                        })?;
                        std::rc::Rc::new(std::cell::RefCell::new(read_through_cell(&value)))
                    } else {
                        // require(ESM) returns the canonical namespace, whose properties
                        // remain live views of the exporting module's binding cells.
                        std::rc::Rc::new(std::cell::RefCell::new(
                            self.records.namespace(&dependency.target),
                        ))
                    }
                } else {
                    return Err(ExecutionError::Host {
                        instruction: 0,
                        message: format!(
                            "CommonJS target `{}` has not been evaluated",
                            dependency.target.display()
                        ),
                    });
                };
            targets.push((
                dependency.specifier.clone(),
                dependency.target.to_string_lossy().into_owned(),
                value,
            ));
        }
        let module_object = ordinary_object([("exports".to_owned(), initial_exports.clone())]);
        self.module_objects
            .insert(path.to_path_buf(), module_object.clone());
        let imports = vec![
            initial_exports,
            Value::CommonJsRequire(std::rc::Rc::new(
                CommonJsRequire::for_module(path.to_string_lossy(), targets).with_resolutions(
                    module_node
                        .runtime_dependencies
                        .iter()
                        .filter_map(|dependency| {
                            (dependency.importer == path).then(|| {
                                (
                                    dependency.specifier.clone(),
                                    if matches!(&dependency.source, ModuleSource::NodeBuiltin(_)) {
                                        dependency.specifier.clone()
                                    } else {
                                        dependency.target.to_string_lossy().into_owned()
                                    },
                                )
                            })
                        }),
                ),
            )),
            module_object.clone(),
            Value::String(path.to_string_lossy().into_owned().into()),
            Value::String(
                path.parent()
                    .unwrap_or_else(|| Path::new(""))
                    .to_string_lossy()
                    .into_owned()
                    .into(),
            ),
        ];
        let total_steps = unsafe { *self.total_steps };
        if total_steps >= self.limits.max_steps {
            return Err(ExecutionError::StepLimitExceeded {
                steps: total_steps,
                limit: self.limits.max_steps,
            });
        }
        let remaining_limits = ExecutionLimits {
            max_steps: self.limits.max_steps - total_steps,
            ..self.limits
        };
        let is_entry = path == self.graph.entry();
        let result = unsafe {
            if self.node {
                execute_node_module_in_session_with_loader(
                    &module_node.program,
                    &imports,
                    &[],
                    &mut *self.host,
                    remaining_limits,
                    &mut *self.runtime,
                    (&mut *self.node_session)
                        .as_mut()
                        .expect("Node mode owns a session"),
                    self.capabilities.clone(),
                    is_entry,
                    self,
                    enter_runtime,
                    Some(path.to_string_lossy().as_ref()),
                )
            } else {
                execute_module_in_runtime_with_loader(
                    &module_node.program,
                    &imports,
                    &[],
                    &mut *self.host,
                    remaining_limits,
                    &mut *self.runtime,
                    self.capabilities.clone(),
                    self,
                    enter_runtime,
                    Some(path.to_string_lossy().as_ref()),
                )
            }
        }?;
        unsafe {
            *self.total_steps = (*self.total_steps)
                .checked_add(result.execution.steps)
                .ok_or(ExecutionError::Host {
                    instruction: 0,
                    message: "CommonJS execution step count overflow".to_owned(),
                })?;
            let final_exports = result.commonjs_exports.unwrap_or_else(|| {
                own_data_property(&module_object, "exports").unwrap_or(Value::Undefined)
            });
            let cache = self
                .commonjs_cache
                .get(path)
                .expect("cache entry checked above");
            *cache.borrow_mut() = final_exports.clone();
            let mut exports_map = HashMap::from([("default".to_owned(), final_exports.clone())]);
            if let Value::Object(object) = &final_exports {
                let object_ref = object.borrow();
                for key in object_ref.keys() {
                    if key != "default"
                        && let Some(value) = object_ref.get_own_value_units(key)
                    {
                        exports_map.insert(key.to_lossy_string(), value);
                    }
                }
            } else if let Value::Function(closure) = &final_exports {
                let properties_ref = closure.properties.borrow();
                for key in properties_ref.keys() {
                    if key != "default"
                        && let Some(Property::Value(val)) =
                            properties_ref.get_own_property_units(key)
                    {
                        exports_map.insert(key.to_lossy_string(), val.clone());
                    }
                }
            }
            self.records.publish(path, exports_map);
            if path == self.graph.entry() {
                *self.entry_result = Some(result.execution);
            }
        }
        Ok(())
    }
}

impl<H: HostOps + ?Sized> CommonJsModuleLoader for CommonJsLoader<'_, H> {
    fn module_evaluated(&mut self, module_path: &str, exports: &[ModuleExportValue]) {
        let path = Path::new(module_path);
        // Only the ES module this graph is evaluating right now; a CommonJS body publishes its
        // `module.exports` when its wrapper returns.
        if !matches!(self.records.status(path), Some(ModuleStatus::Evaluating))
            || !self.graph.module(path).is_some_and(|node| {
                matches!(
                    node.source,
                    ModuleSource::Source | ModuleSource::NodeBuiltin(_)
                )
            })
        {
            return;
        }
        let exports = GraphExecutor(self.graph).module_exports(path, exports, &self.records);
        self.records.publish(path, exports);
    }

    fn evaluate_module(&mut self, key: &str, instruction: usize) -> Result<(), ExecutionError> {
        let path = PathBuf::from(key);
        match self.graph.module(&path).map(|node| &node.source) {
            Some(ModuleSource::CommonJs) => self.prepare_namespace(&path),
            Some(ModuleSource::Json) => self.ensure_target_value(&path, instruction).map(|_| ()),
            _ => Err(ExecutionError::DynamicImportUnsupported {
                message: format!("`{key}` is not a CommonJS or JSON module of this graph"),
            }),
        }
    }

    fn prepare_module_namespace(
        &mut self,
        key: &str,
        _instruction: usize,
    ) -> Result<(), ExecutionError> {
        self.prepare_namespace(Path::new(key))
    }

    fn record_thrown_value(&mut self, value: Value) {
        LAST_THROWN.with(|last| *last.borrow_mut() = Some(value.clone()));
        self.pending_thrown = Some(value);
    }

    fn take_thrown_value(&mut self) -> Option<Value> {
        LAST_THROWN.with(|last| last.borrow_mut().take());
        self.pending_thrown.take()
    }

    fn get_builtin(
        &mut self,
        specifier: &str,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        let builtin = NodeBuiltin::parse(specifier).ok_or_else(|| ExecutionError::Host {
            instruction,
            message: format!("unknown builtin module `{specifier}`"),
        })?;
        // Engine-internal modules are not part of any program's builtin surface.
        if builtin.is_engine_internal() {
            return Ok(None);
        }
        let path = self.graph.module_map().iter().find_map(|(path, node)| {
            (node.source == ModuleSource::NodeBuiltin(builtin)).then_some(path)
        });
        let Some(path) = path else {
            return Ok(None);
        };
        if !self.records.is_evaluated(path) {
            self.ensure_target_value(path, instruction)?;
        }
        let provider = self
            .records
            .exports(path)
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: format!("builtin module `{specifier}` has not been evaluated"),
            })?;
        let value = provider
            .get("default")
            .cloned()
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: format!("builtin module `{specifier}` has no default export"),
            })?;
        Ok(Some(read_through_cell(&value)))
    }

    fn create_require(
        &mut self,
        importer: &str,
        _instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let path = if importer.starts_with("file:") {
            url::Url::parse(importer)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .ok_or_else(|| ExecutionError::TypeErrorMessage {
                    message: format!("invalid module filename `{importer}`"),
                })?
        } else {
            PathBuf::from(importer)
        };
        if !path.is_absolute() {
            return Err(ExecutionError::TypeErrorMessage {
                message: "The argument must be an absolute path or file URL".to_owned(),
            });
        }
        let resolutions = self
            .graph
            .module_map()
            .values()
            .flat_map(|node| node.runtime_dependencies.iter())
            .filter(|dependency| dependency.importer == path)
            .map(|dependency| {
                (
                    dependency.specifier.clone(),
                    if matches!(&dependency.source, ModuleSource::NodeBuiltin(_)) {
                        dependency.specifier.clone()
                    } else {
                        dependency.target.to_string_lossy().into_owned()
                    },
                )
            })
            .collect::<Vec<_>>();
        Ok(Value::CommonJsRequire(std::rc::Rc::new(
            CommonJsRequire::for_importer(path.to_string_lossy()).with_resolutions(resolutions),
        )))
    }

    fn load(
        &mut self,
        require: &CommonJsRequire,
        specifier: &str,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Some(cell) = require.target_cell(specifier) else {
            if let Some(importer) = require.importer() {
                if let Some(message) = self
                    .graph
                    .module_map()
                    .values()
                    .filter_map(|node| {
                        self.graph
                            .runtime_failures(&node.path)
                            .find(|((base, requested), _)| {
                                base == Path::new(importer) && requested == specifier
                            })
                            .map(|(_, message)| message.clone())
                    })
                    .next()
                {
                    return Err(ExecutionError::ModuleNotFound {
                        instruction,
                        message,
                    });
                }
                let runtime_dependency = self
                    .graph
                    .module_map()
                    .values()
                    .flat_map(|node| node.runtime_dependencies.iter())
                    .find(|dependency| {
                        dependency.importer == Path::new(importer)
                            && dependency.specifier == specifier
                            && !dependency.resolve_only
                    })
                    .cloned();
                if let Some(dependency) = runtime_dependency {
                    return self.ensure_target_value(&dependency.target, instruction);
                }
                if let Some(target) = self.graph.declared_target_for_runtime_specifier(specifier) {
                    return self.ensure_target_value(&target, instruction);
                }
                if let Some(owner) =
                    NodeBuiltin::parse(specifier).and_then(NodeBuiltin::internal_owner)
                {
                    return Err(ExecutionError::ModuleNotFound {
                        instruction,
                        message: format!("`{specifier}` is internal to {owner}"),
                    });
                }
                if NodeBuiltin::parse(specifier).is_some() {
                    return self.get_builtin(specifier, instruction)?.ok_or_else(|| {
                        ExecutionError::Host {
                            instruction,
                            message: format!("builtin module `{specifier}` is unavailable"),
                        }
                    });
                }
                let importer = PathBuf::from(importer);
                let dependency = self
                    .graph
                    .module(&importer)
                    .and_then(|node| {
                        node.dependencies
                            .iter()
                            .find(|dependency| dependency.specifier == specifier)
                            .cloned()
                    })
                    .ok_or_else(|| ExecutionError::ModuleNotFound {
                        instruction,
                        message: format!("Cannot find module '{specifier}'"),
                    })?;
                let target = dependency.target;
                if let Some(cell) = self.commonjs_cache.get(&target) {
                    self.execute_target(&target, false)?;
                    if matches!(self.records.status(&target), Some(ModuleStatus::Evaluating)) {
                        if let Some(module_object) = self.module_objects.get(&target) {
                            return Ok(own_data_property(module_object, "exports")
                                .unwrap_or_else(|| cell.borrow().clone()));
                        }
                    }
                    return Ok(cell.borrow().clone());
                }
                if let Some(provider) = self.records.exports(&target) {
                    if self.graph.module(&target).is_some_and(|node| {
                        node.source == ModuleSource::Json
                            || matches!(node.source, ModuleSource::NodeBuiltin(_))
                    }) {
                        return provider.get("default").cloned().ok_or_else(|| {
                            ExecutionError::Host {
                                instruction,
                                message: format!("Target `{}` has no default", target.display()),
                            }
                        });
                    }
                    return Ok(self.records.namespace(&target));
                }
                return Err(ExecutionError::Host {
                    instruction,
                    message: format!(
                        "CommonJS target `{}` has not been evaluated",
                        target.display()
                    ),
                });
            }
            return Err(ExecutionError::ModuleNotFound {
                instruction,
                message: format!("CommonJS require target `{specifier}` is not in the graph"),
            });
        };
        if let Some(path) = self.cache_targets.get(&std::rc::Rc::as_ptr(&cell)).cloned() {
            self.execute_target(&path, false)?;
            if matches!(self.records.status(&path), Some(ModuleStatus::Evaluating))
                && let Some(module_object) = self.module_objects.get(&path)
            {
                return Ok(own_data_property(module_object, "exports")
                    .unwrap_or_else(|| cell.borrow().clone()));
            }
            return Ok(cell.borrow().clone());
        }
        require
            .cached_value(specifier)
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: format!(
                    "CommonJS require target `{specifier}` has a cell without a cached value"
                ),
            })
    }
}
