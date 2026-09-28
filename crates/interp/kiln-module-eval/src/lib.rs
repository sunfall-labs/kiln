mod realm_bootstrap;
pub use realm_bootstrap::realm_bootstrap;

use kiln_host_abi::HostOps;
use kiln_interpreter::{
    Cell, CommonJsModuleLoader, CommonJsRequire, ExecutionCapabilities, ExecutionError,
    ExecutionLimits, ExecutionResult, ImportAttributes, ModuleBinding, ModuleLoadError,
    ModuleLoadRequest, ModuleLoaderService, ModulePlan, ModuleRealmSession, ModuleSelfExport,
    ModuleSelfImport, ModuleValueRef, PlannedModule, Property, Value,
    execute_module_in_runtime_with_loader, execute_node_module_in_session_with_loader,
    ordinary_object, own_data_property, parse_json_module, read_through_cell,
};
use kiln_ir::{BYTECODE_VERSION, Instruction, Program};
use kiln_module_graph::{ModuleGraph, ModuleGraphError, ModuleNode, ModuleSource, ResolvedExport};
use kiln_module_resolver::NodeBuiltin;
use kiln_runtime::{ClockCapability, RealmCapabilities, RealmRuntime};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
    fn execute_node<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
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
    fn execute_node<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        GraphExecutor(self).execute_node(host, limits, capabilities)
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
}

impl kiln_interpreter::WorkerHost for GraphWorkers {
    fn has_module(&self, path: &str) -> bool {
        self.graph.module(path).is_some()
    }

    fn run(&self, worker: Arc<kiln_runtime::workers::WorkerContext>) -> i32 {
        use kiln_runtime::workers::WorkerEntry;
        let graph = match &worker.spec.entry {
            WorkerEntry::File(path) => self
                .graph
                .with_entry(path)
                .ok_or_else(|| format!("Cannot find module '{path}'")),
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
            inner: kiln_runtime::CaptureHost::new()
                .with_fs(Box::new(kiln_host_abi::OsFileSystem)),
        };
        let capabilities = self.capabilities.clone().with_argv(worker.spec.argv.clone());
        LAST_THROWN.with(|last| last.borrow_mut().take());
        let outcome = graph.execute_node(&mut host, self.limits, capabilities);
        let thrown = LAST_THROWN.with(|last| last.borrow_mut().take());
        match outcome {
            Ok(_) => 0,
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
                    None => kiln_interpreter::worker_error_tokens(
                        &Value::String(message.into()),
                        None,
                    ),
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
fn eval_graph(source: &str) -> Result<ModuleGraph, String> {
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
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let build = |name: &str| {
        let resolver = kiln_module_resolver::NodeRuntimeResolver::new(&cwd)
            .map_err(|error| ModuleGraphError::Source {
                path: cwd.clone(),
                message: error.to_string(),
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
}

impl std::ops::Deref for GraphExecutor<'_> {
    type Target = ModuleGraph;
    fn deref(&self) -> &Self::Target {
        self.0
    }
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
pub struct GraphModuleLoader {
    graph: Arc<ModuleGraph>,
}

impl GraphModuleLoader {
    pub fn new(graph: Arc<ModuleGraph>) -> Self {
        Self { graph }
    }
}

impl ModuleLoaderService for GraphModuleLoader {
    fn load(&self, request: ModuleLoadRequest) -> Result<ModulePlan, ModuleLoadError> {
        let importer = request
            .importer
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.graph.entry().to_path_buf());
        let node = self.graph.module(&importer).ok_or_else(|| {
            ModuleLoadError::NotFound(format!(
                "importer `{}` is not in the module graph",
                importer.display()
            ))
        })?;
        if let Some(message) = self.graph.dynamic_failure(&importer, &request.specifier) {
            return Err(ModuleLoadError::Unsupported(message.to_owned()));
        }
        let dependency = self
            .graph
            .dynamic_dependencies(&importer)
            .iter()
            .find(|dependency| dependency.specifier == request.specifier)
            .or_else(|| {
                node.dependencies
                    .iter()
                    .find(|dependency| dependency.specifier == request.specifier)
            });
        let target = dependency
            .map(|dependency| dependency.target.clone())
            .or_else(|| {
                self.graph
                    .declared_target_for_runtime_specifier(&request.specifier)
            })
            .or_else(|| {
                if let Some(builtin) = NodeBuiltin::parse(&request.specifier) {
                    self.graph.module_map().iter().find_map(|(path, node)| {
                        (node.source == ModuleSource::NodeBuiltin(builtin)).then(|| path.clone())
                    })
                } else {
                    None
                }
            })
            .or_else(|| {
                let direct = PathBuf::from(&request.specifier);
                if self.graph.module_map().contains_key(&direct) {
                    return Some(direct);
                }
                if let Some(parent) = importer.parent() {
                    let relative = parent.join(&request.specifier);
                    if self.graph.module_map().contains_key(&relative) {
                        return Some(relative);
                    }
                    for ext in &[".ts", ".js", ".json", "/index.ts", "/index.js"] {
                        let candidate = parent.join(format!("{}{ext}", request.specifier));
                        if self.graph.module_map().contains_key(&candidate) {
                            return Some(candidate);
                        }
                    }
                }
                None
            })
            .ok_or_else(|| {
                ModuleLoadError::NotFound(format!(
                    "could not resolve module `{}` from `{}`",
                    request.specifier,
                    importer.display()
                ))
            })?;
        self.graph
            .import_plan_for_request(&target, request.attributes.as_ref())
            .map_err(|error| ModuleLoadError::Unsupported(error.to_string()))
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
        values: &HashMap<PathBuf, HashMap<String, Value>>,
    ) -> Option<ModuleSelfImport> {
        if import.imported == kiln_ir::NAMESPACE_IMPORT {
            // Sorted, because that is the order a namespace holds its names in and the object is
            // built once and only ever refreshed in place afterwards.
            let mut names = self.resolved_export_names(path)?;
            names.sort();
            let mut exports = Vec::with_capacity(names.len());
            for name in names {
                let source = self.self_export(path, &name, values)?;
                exports.push((name, source));
            }
            return Some(ModuleSelfImport::Namespace {
                import_slot: import.local_slot,
                exports,
            });
        }
        match self.self_export(path, &import.imported, values)? {
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

    /// The cell a back-edge import binds to, created on first demand.
    ///
    /// The importer is running before the module that owns the binding, so there is no value to
    /// hand over. What can be handed over is the *place* the value will arrive in, which is what
    /// an import slot holds anyway -- the interpreter boxes every import precisely so that an
    /// import is a live view of the exporter's binding rather than a copy of it.
    ///
    /// Two shapes are refused instead of guessed. A namespace import of a module that has not
    /// run would need an object whose every property is a live view, and this linker has one of
    /// those only for a module importing from *itself*. An ambiguous or absent name has no
    /// binding to point at in the first place. Both are named, because a cycle that cannot be
    /// bound is a smaller thing than a cycle that cannot be built, and saying which is which is
    /// the difference between a refusal and a wrong answer.
    fn cycle_cell(
        &self,
        importer: &Path,
        provider: &Path,
        import: &kiln_ir::ImportBinding,
        values: &HashMap<PathBuf, HashMap<String, Value>>,
        pending: &mut HashMap<(PathBuf, String), Cell>,
    ) -> Result<Cell, ModuleEvaluationError> {
        if import.imported == kiln_ir::NAMESPACE_IMPORT {
            if let Some(exports) = values.get(provider) {
                let mut entries: Vec<(String, Value)> = exports
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                let ns = kiln_interpreter::namespace_object(entries);
                return Ok(std::rc::Rc::new(std::cell::RefCell::new(ns)));
            }
            return Ok(pending
                .entry((
                    provider.to_path_buf(),
                    kiln_ir::NAMESPACE_IMPORT.to_string(),
                ))
                .or_insert_with(|| {
                    std::rc::Rc::new(std::cell::RefCell::new(Value::Native(
                        kiln_interpreter::Native::Uninitialized,
                        kiln_interpreter::RealmId::PRIMARY,
                    )))
                })
                .clone());
        }
        let (origin, local_slot) = match self.resolved_export(provider, &import.imported) {
            Some(ResolvedExport::Local { module, local_slot }) => (module.clone(), *local_slot),
            Some(ResolvedExport::CommonJsNamed { module, .. }) => (module.clone(), 0),
            Some(ResolvedExport::Namespace { module }) => {
                if let Some(exports) = values.get(module) {
                    let mut entries: Vec<(String, Value)> = exports
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect();
                    entries.sort_by(|left, right| left.0.cmp(&right.0));
                    let ns = kiln_interpreter::namespace_object(entries);
                    return Ok(std::rc::Rc::new(std::cell::RefCell::new(ns)));
                }
                return Ok(pending
                    .entry((module.clone(), kiln_ir::NAMESPACE_IMPORT.to_string()))
                    .or_insert_with(|| {
                        std::rc::Rc::new(std::cell::RefCell::new(Value::Native(
                            kiln_interpreter::Native::Uninitialized,
                            kiln_interpreter::RealmId::PRIMARY,
                        )))
                    })
                    .clone());
            }
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
        Ok(pending
            .entry((origin, exported))
            .or_insert_with(|| {
                // The primary realm: this cell is a placeholder for a binding a cycle has not
                // initialised yet, and `Uninitialized` carries no realm-observable identity.
                std::rc::Rc::new(std::cell::RefCell::new(Value::Native(
                    kiln_interpreter::Native::Uninitialized,
                    kiln_interpreter::RealmId::PRIMARY,
                )))
            })
            .clone())
    }

    /// Where one name exported by `module` gets its value, from `module`'s own point of view.
    fn self_export(
        &self,
        path: &Path,
        name: &str,
        values: &HashMap<PathBuf, HashMap<String, Value>>,
    ) -> Option<ModuleSelfExport> {
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
                let value = values.get(module)?.get(&exported.exported)?;
                Some(ModuleSelfExport::Settled(value.clone()))
            }
            ResolvedExport::Namespace { module } => {
                let mut entries: Vec<(String, Value)> = values
                    .get(module)?
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                Some(ModuleSelfExport::Settled(
                    kiln_interpreter::namespace_object(entries),
                ))
            }
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
    /// Each planned module's program ends by handing back an array of the values anything else
    /// in the plan refers to. Building that epilogue belongs here for the same reason: choosing
    /// the slots needs the linker's answer about where each visible export lives, and the
    /// interpreter has no way to ask.
    pub fn import_plan(&self) -> Result<ModulePlan, ModuleEvaluationError> {
        // Every slot anything in this graph will want out of any module, gathered before an
        // epilogue is written: a module's array has to hold every value its importers, and the
        // entry's namespace, will later look up by index.
        let mut wanted: BTreeMap<PathBuf, BTreeSet<u32>> = BTreeMap::new();
        for resolutions in self.export_resolutions().values() {
            for resolution in resolutions.values() {
                if let ResolvedExport::Local { module, local_slot } = resolution {
                    wanted
                        .entry(module.clone())
                        .or_default()
                        .insert(*local_slot);
                } else if let ResolvedExport::CommonJsNamed { module, .. } = resolution {
                    wanted.entry(module.clone()).or_default().insert(0);
                }
            }
        }

        let static_reachable = self.statically_reachable();
        let plan_order = self
            .module_order()
            .iter()
            .filter(|path| static_reachable.contains(*path))
            .collect::<Vec<_>>();
        let position: HashMap<&Path, usize> = plan_order
            .iter()
            .enumerate()
            .map(|(index, path)| (path.as_path(), index))
            .collect();
        let slot_index = |module: &Path, slot: u32| -> Option<ModuleValueRef> {
            Some(ModuleValueRef {
                module: *position.get(module)?,
                index: wanted.get(module)?.iter().position(|held| *held == slot)?,
            })
        };

        let mut modules = Vec::with_capacity(plan_order.len());
        for path in plan_order {
            let node = self
                .module(path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
            // Only a source module has a body to run. A JSON module and a CommonJS wrapper each
            // need their own materialisation, and answering with an empty namespace for one
            // would be a wrong answer rather than a missing feature.
            if !matches!(
                node.source,
                ModuleSource::Source | ModuleSource::Asset | ModuleSource::NodeBuiltin(_)
            ) {
                return Err(ModuleGraphError::Source {
                    path: path.clone(),
                    message: format!(
                        "`import()` of a {:?} module is not implemented",
                        node.source
                    ),
                }
                .into());
            }
            let mut bindings = Vec::with_capacity(node.program.imports.len());
            for import in &node.program.imports {
                let dependency = node
                    .dependencies
                    .iter()
                    .find(|dependency| dependency.specifier == import.source)
                    .ok_or_else(|| ModuleGraphError::UnresolvedImport {
                        importer: path.clone(),
                        source: import.source.clone(),
                    })?;
                // A module that imports from itself is evaluated once, so there is no earlier
                // entry in the plan to take the value from. Nothing reads the slot either: the
                // only import a module can have from itself is the placeholder a re-export
                // desugars to, whose name no source can write, and the linker resolves the
                // export it stands for straight through to its origin. Any *other* self-import
                // would be read, so it is refused rather than left holding `undefined`.
                let binding = if dependency.target == *path {
                    if !import.local.starts_with(kiln_ir::REEXPORT_LOCAL_PREFIX) {
                        return Err(ModuleGraphError::Source {
                            path: path.clone(),
                            message: format!(
                                "module imports `{}` from itself, which this linker cannot bind",
                                import.imported
                            ),
                        }
                        .into());
                    }
                    Some(ModuleBinding::Ready(Value::Undefined))
                } else if import.imported == kiln_ir::NAMESPACE_IMPORT {
                    self.plan_namespace(&dependency.target, &slot_index)
                        .map(ModuleBinding::Namespace)
                } else {
                    match self.resolved_export(&dependency.target, &import.imported) {
                        Some(ResolvedExport::Local { module, local_slot }) => {
                            slot_index(module, *local_slot).map(ModuleBinding::Value)
                        }
                        Some(ResolvedExport::Namespace { module }) => self
                            .plan_namespace(module, &slot_index)
                            .map(ModuleBinding::Namespace),
                        Some(ResolvedExport::CommonJsNamed { module, .. }) => {
                            slot_index(module, 0).map(ModuleBinding::Value)
                        }
                        Some(ResolvedExport::Ambiguous) | None => None,
                    }
                };
                let binding = binding.ok_or_else(|| ModuleGraphError::MissingExport {
                    importer: path.clone(),
                    provider: dependency.target.clone(),
                    name: import.imported.clone(),
                })?;
                // A plan is a flat list the importing realm walks once, front to back, taking
                // each module's values out of the array the one before it returned. A cycle
                // makes a module ask for a value from an entry that has not run, and there is
                // no array to take it from. The graph executor answers that with a dead-zone
                // cell it fills in later; a plan has nowhere to put one, so this is refused by
                // name rather than left pointing at an index whose contents do not exist.
                let here = position.get(path.as_path()).copied().unwrap_or(0);
                let reaches_forward = match &binding {
                    ModuleBinding::Value(reference) => reference.module >= here,
                    ModuleBinding::Namespace(entries) => entries
                        .iter()
                        .any(|(_, reference)| reference.module >= here),
                    ModuleBinding::Ready(_) => false,
                };
                if reaches_forward {
                    return Err(ModuleGraphError::Source {
                        path: path.clone(),
                        message: format!(
                            "`import()` of a module graph whose cycle crosses `{}` is not \
                             implemented",
                            dependency.target.display()
                        ),
                    }
                    .into());
                }
                bindings.push((import.local_slot, binding));
            }
            let slots = wanted
                .get(path)
                .map(|slots| slots.iter().copied().collect())
                .unwrap_or_default();
            let plan_program = if node.source == ModuleSource::Asset {
                asset_module_program(node.data.as_deref().ok_or_else(|| {
                    ModuleGraphError::Source {
                        path: path.clone(),
                        message: "asset module has no path metadata".to_owned(),
                    }
                })?)
            } else {
                export_epilogue(&node.program.program, slots)
            };
            modules.push(PlannedModule {
                key: path.to_string_lossy().into_owned(),
                program: std::rc::Rc::new(plan_program),
                bindings,
            });
        }

        let namespace = self
            .plan_namespace(self.entry(), &slot_index)
            .ok_or_else(|| ModuleGraphError::InternalMissingModule {
                path: self.entry().to_path_buf(),
            })?;
        Ok(ModulePlan {
            key: self.entry().to_string_lossy().into_owned(),
            modules,
            namespace,
        })
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

    /// Every visible export of `module`, paired with where in the plan its value will be.
    fn plan_namespace(
        &self,
        module: &Path,
        slot_index: &impl Fn(&Path, u32) -> Option<ModuleValueRef>,
    ) -> Option<Vec<(String, ModuleValueRef)>> {
        let resolutions = self.export_resolutions().get(module)?;
        let mut entries = Vec::with_capacity(resolutions.len());
        for (name, resolution) in resolutions {
            // Only a name whose value the plan can point at is included. An ambiguous star
            // export has no value, and `export * as ns from` names an object rather than a slot;
            // leaving either out keeps the namespace honest about what it holds, where putting
            // a wrong value in would not.
            if let ResolvedExport::Local { module, local_slot } = resolution
                && let Some(reference) = slot_index(module, *local_slot)
            {
                entries.push((name.clone(), reference));
            }
        }
        Some(entries)
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
        self.execute_mode(host, limits, capabilities, true)
    }

    fn execute_mode<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        node: bool,
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        // Every realm this graph runs in -- the one the modules share and any a module makes --
        // runs the shared built-ins first, in the set this mode has (see `realm_bootstrap`).
        let capabilities = capabilities.with_realm_bootstrap(realm_bootstrap(node));
        // A Node program can start workers, each a realm running another of this graph's
        // modules on a thread of its own (realms issue 06).
        let capabilities = if node {
            let workers = GraphWorkers {
                graph: (*self.0).clone(),
                limits,
                capabilities: capabilities.clone(),
            };
            capabilities.with_worker_host(Arc::new(workers))
        } else {
            capabilities
        };
        // A program run on a main thread numbers its workers from 1, as a node process does.
        kiln_runtime::workers::begin_program();
        let outcome = self.execute_mode_in(host, limits, capabilities, node);
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
    ) -> Result<ExecutionResult, ModuleEvaluationError> {
        let mut values: HashMap<PathBuf, HashMap<String, Value>> = HashMap::new();
        // One cell per binding that a cycle forces somebody to hold before the module that owns
        // it has run. The cell starts in the dead zone, so reading the binding too early is the
        // `ReferenceError` the specification asks for rather than `undefined`, and it is filled
        // in when the owning module finishes. Keyed by the *origin* of the name rather than by
        // the specifier it arrived through, because a re-export chain can cross a cycle several
        // times and every crossing has to land on the same cell.
        let mut pending_cycle_cells: HashMap<(PathBuf, String), Cell> = HashMap::new();
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
        let realm_capabilities = if node {
            RealmCapabilities {
                clock: ClockCapability::Host,
                ..RealmCapabilities::default()
            }
        } else {
            RealmCapabilities::default()
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
        let cache_targets = commonjs_cache
            .iter()
            .map(|(path, cell)| (std::rc::Rc::as_ptr(cell), path.clone()))
            .collect();
        let mut commonjs_states = HashMap::new();
        let mut commonjs_loader = CommonJsLoader {
            graph: self,
            commonjs_cache: &commonjs_cache,
            cache_targets,
            values: &mut values as *mut _,
            states: &mut commonjs_states as *mut _,
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
                values.insert(path.clone(), HashMap::from([("default".to_owned(), value)]));
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
                values.insert(
                    module_node.path.clone(),
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
                    let bound = self.self_import(path, import, &values);
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
                if !values.contains_key(&dependency.target)
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
                // A dependency with no value table has not run, and in an acyclic graph that
                // cannot happen. So this is a back edge: the importer is an ancestor of the
                // module it is importing from, and the binding it wants will not exist until
                // later in this very loop.
                let Some(provider) = values.get(&dependency.target) else {
                    let cell = self.cycle_cell(
                        path,
                        &dependency.target,
                        import,
                        &values,
                        &mut pending_cycle_cells,
                    )?;
                    imports.push(Value::Cell(cell));
                    continue;
                };
                let value = match linked_import_value(provider, &import.imported, false) {
                    Some(value) => value,
                    None => match self.resolved_export(&dependency.target, &import.imported) {
                        Some(ResolvedExport::Local {
                            module: origin,
                            local_slot,
                        }) => {
                            if let Some(origin_exports) = values.get(origin) {
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
                                linked_import_value(origin_exports, exported_name, false)
                                    .ok_or_else(|| ModuleGraphError::MissingExport {
                                        importer: path.clone(),
                                        provider: origin.clone(),
                                        name: import.imported.clone(),
                                    })?
                            } else {
                                let cell = self.cycle_cell(
                                    path,
                                    &dependency.target,
                                    import,
                                    &values,
                                    &mut pending_cycle_cells,
                                )?;
                                imports.push(Value::Cell(cell));
                                continue;
                            }
                        }
                        Some(ResolvedExport::CommonJsNamed {
                            module: origin,
                            property,
                        }) => {
                            if let Some(origin_exports) = values.get(origin) {
                                linked_import_value(origin_exports, property, false).ok_or_else(
                                    || ModuleGraphError::MissingExport {
                                        importer: path.clone(),
                                        provider: origin.clone(),
                                        name: import.imported.clone(),
                                    },
                                )?
                            } else {
                                let cell = self.cycle_cell(
                                    path,
                                    &dependency.target,
                                    import,
                                    &values,
                                    &mut pending_cycle_cells,
                                )?;
                                imports.push(Value::Cell(cell));
                                continue;
                            }
                        }
                        Some(ResolvedExport::Namespace { module: origin }) => {
                            if let Some(origin_exports) = values.get(origin) {
                                linked_import_value(
                                    origin_exports,
                                    kiln_ir::NAMESPACE_IMPORT,
                                    false,
                                )
                                .ok_or_else(|| {
                                    ModuleGraphError::MissingExport {
                                        importer: path.clone(),
                                        provider: origin.clone(),
                                        name: import.imported.clone(),
                                    }
                                })?
                            } else {
                                let cell = self.cycle_cell(
                                    path,
                                    &dependency.target,
                                    import,
                                    &values,
                                    &mut pending_cycle_cells,
                                )?;
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
            )
            .map_err(|error| {
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
            // A `export * from` entry stands for the provider's names rather than for one of
            // its own. Local exports are inserted first so that a name this module exports
            // itself wins over the same name arriving through a star, which is what the
            // specification says and what a barrel file that overrides one name relies on.
            let mut exports: HashMap<String, Value> = result
                .exports
                .iter()
                .filter(|export| !export.name.starts_with(kiln_ir::STAR_REEXPORT_PREFIX))
                .map(|export| (export.name.clone(), export.value.clone()))
                .collect();
            for star in result
                .exports
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
                            if let Some(origin_exports) = values.get(origin) {
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
                                let cell = pending_cycle_cells
                                    .entry((origin.clone(), origin_exported))
                                    .or_insert_with(|| {
                                        std::rc::Rc::new(std::cell::RefCell::new(Value::Native(
                                            kiln_interpreter::Native::Uninitialized,
                                            kiln_interpreter::RealmId::PRIMARY,
                                        )))
                                    })
                                    .clone();
                                exports.insert(name.clone(), Value::Cell(cell));
                            }
                        }
                        ResolvedExport::CommonJsNamed {
                            module: origin,
                            property,
                        } => {
                            if let Some(origin_exports) = values.get(origin) {
                                if let Some(val) = origin_exports.get(property) {
                                    exports.insert(name.clone(), val.clone());
                                }
                            }
                        }
                        ResolvedExport::Namespace { module: origin } => {
                            if let Some(origin_exports) = values.get(origin) {
                                let mut entries: Vec<(String, Value)> = origin_exports
                                    .iter()
                                    .map(|(n, v)| (n.clone(), read_through_cell(v)))
                                    .collect();
                                entries.sort_by(|l, r| l.0.cmp(&r.0));
                                exports.insert(
                                    name.clone(),
                                    kiln_interpreter::namespace_object(entries),
                                );
                            }
                        }
                        ResolvedExport::Ambiguous => {}
                    }
                }
            }
            // Whatever this module was holding a cycle's place for is now real. The value is
            // read through its cell, because what is published is the binding and what a waiting
            // importer's cell has to contain is the value in it.
            for (name, value) in &exports {
                if let Some(cell) = pending_cycle_cells.remove(&(path.clone(), name.clone())) {
                    *cell.borrow_mut() = read_through_cell(value);
                }
            }
            if let Some(cell) =
                pending_cycle_cells.remove(&(path.clone(), kiln_ir::NAMESPACE_IMPORT.to_string()))
            {
                let mut entries: Vec<(String, Value)> = exports
                    .iter()
                    .map(|(name, value)| (name.clone(), read_through_cell(value)))
                    .collect();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                *cell.borrow_mut() = kiln_interpreter::namespace_object(entries);
            }
            values.insert(path.clone(), exports);
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum CommonJsExecutionState {
    Executing,
    Done,
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
    values: *mut HashMap<PathBuf, HashMap<String, Value>>,
    states: *mut HashMap<PathBuf, CommonJsExecutionState>,
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

fn linked_import_value(
    provider: &HashMap<String, Value>,
    imported: &str,
    read_cells: bool,
) -> Option<Value> {
    if imported == kiln_ir::NAMESPACE_IMPORT {
        let mut entries: Vec<_> = provider
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    if read_cells {
                        read_through_cell(value)
                    } else {
                        value.clone()
                    },
                )
            })
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        return Some(kiln_interpreter::namespace_object(entries));
    }
    provider
        .get(imported)
        .map(|value| {
            if read_cells {
                read_through_cell(value)
            } else {
                value.clone()
            }
        })
        .or_else(|| {
            provider
                .get("default")
                .and_then(|default_val| own_data_property(default_val, imported))
        })
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
        if let Some(provider) = unsafe { (&*self.values).get(target) }
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
            let source = module_node.data.as_deref().ok_or_else(|| ExecutionError::Host {
                instruction,
                message: format!("JSON module `{}` has no captured source text", target.display()),
            })?;
            let value = parse_json_module(source)?;
            unsafe {
                (&mut *self.values).insert(
                    target.to_path_buf(),
                    HashMap::from([("default".to_owned(), value.clone())]),
                );
            }
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
            if !unsafe { (&*self.values).contains_key(&dependency.target) }
                && let Some(node) = self.graph.module(&dependency.target)
            {
                if node.source == ModuleSource::CommonJs {
                    self.execute_target(&dependency.target, false)?;
                } else if matches!(node.source, ModuleSource::NodeBuiltin(_)) {
                    self.ensure_target_value(&dependency.target, instruction)?;
                }
            }
            let provider = unsafe { (&*self.values).get(&dependency.target) }.ok_or_else(|| {
                ExecutionError::Host {
                    instruction,
                    message: format!(
                        "lazy builtin dependency `{}` has not been evaluated",
                        dependency.target.display()
                    ),
                }
            })?;
            imports.push(
                linked_import_value(provider, &import.imported, true).ok_or_else(|| {
                    ExecutionError::Host {
                        instruction,
                        message: format!(
                            "lazy builtin dependency `{}` has no export `{}`",
                            dependency.target.display(),
                            import.imported
                        ),
                    }
                })?,
            );
        }
        let result = unsafe {
            if self.node {
                execute_node_module_in_session_with_loader(
                    &program,
                    &imports,
                    &[],
                    &mut *self.host,
                    self.limits,
                    &mut *self.runtime,
                    (&mut *self.node_session)
                        .as_mut()
                        .expect("Node mode owns a session"),
                    self.capabilities.clone(),
                    false,
                    self,
                    false,
                    Some(target.to_string_lossy().as_ref()),
                )
            } else {
                execute_module_in_runtime_with_loader(
                    &program,
                    &imports,
                    &[],
                    &mut *self.host,
                    self.limits,
                    &mut *self.runtime,
                    self.capabilities.clone(),
                    self,
                    false,
                    Some(target.to_string_lossy().as_ref()),
                )
            }
        }?;
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
        unsafe {
            (&mut *self.values).insert(target.to_path_buf(), exports);
        }
        Ok(read_through_cell(&value))
    }

    fn execute_target(&mut self, path: &Path, enter_runtime: bool) -> Result<(), ExecutionError> {
        match unsafe { (&*self.states).get(path).copied() } {
            Some(CommonJsExecutionState::Executing | CommonJsExecutionState::Done) => return Ok(()),
            None => {}
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
        unsafe {
            (&mut *self.states).insert(path.to_path_buf(), CommonJsExecutionState::Executing);
        }
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
                && unsafe { (&*self.values).get(&dependency.target).is_none() }
                && self
                    .graph
                    .module(&dependency.target)
                    .is_some_and(|node| {
                        matches!(node.source, ModuleSource::NodeBuiltin(_) | ModuleSource::Json)
                    })
            {
                self.ensure_target_value(&dependency.target, 0)?;
            }
            let value =
                if let Some(cell) = self.commonjs_cache.get(&dependency.target) {
                    std::rc::Rc::clone(cell)
                } else if let Some(provider) = unsafe { (&*self.values).get(&dependency.target) } {
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
                        let mut entries: Vec<_> = provider
                            .iter()
                            .map(|(name, value)| (name.clone(), read_through_cell(value)))
                            .collect();
                        entries.sort_by(|left, right| left.0.cmp(&right.0));
                        std::rc::Rc::new(std::cell::RefCell::new(ordinary_object(entries)))
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
        let imports = vec![
            initial_exports,
            Value::CommonJsRequire(std::rc::Rc::new(CommonJsRequire::for_module(
                path.to_string_lossy(),
                targets,
            ))),
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
                        && let Some(Property::Value(val)) = object_ref.get_own_property_units(key)
                    {
                        exports_map.insert(key.to_lossy_string(), val.clone());
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
            (&mut *self.values).insert(path.to_path_buf(), exports_map);
            (&mut *self.states).insert(path.to_path_buf(), CommonJsExecutionState::Done);
            if path == self.graph.entry() {
                *self.entry_result = Some(result.execution);
            }
        }
        Ok(())
    }
}

impl<H: HostOps + ?Sized> CommonJsModuleLoader for CommonJsLoader<'_, H> {
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
        let path = self.graph.module_map().iter().find_map(|(path, node)| {
            (node.source == ModuleSource::NodeBuiltin(builtin)).then_some(path)
        });
        let Some(path) = path else {
            return Ok(None);
        };
        if unsafe { (&*self.values).get(path).is_none() } {
            self.ensure_target_value(path, instruction)?;
        }
        let provider =
            unsafe { (&*self.values).get(path) }.ok_or_else(|| ExecutionError::Host {
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
        Ok(Value::CommonJsRequire(std::rc::Rc::new(
            CommonJsRequire::for_importer(path.to_string_lossy()),
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
                    })
                    .cloned();
                if let Some(dependency) = runtime_dependency {
                    return self.ensure_target_value(&dependency.target, instruction);
                }
                if let Some(target) = self.graph.declared_target_for_runtime_specifier(specifier) {
                    return self.ensure_target_value(&target, instruction);
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
                    return Ok(cell.borrow().clone());
                }
                if let Some(provider) = unsafe { (&*self.values).get(&target) } {
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

fn asset_module_program(path: &str) -> Program {
    Program {
        version: BYTECODE_VERSION,
        constants: vec![kiln_ir::Constant::String(path.to_owned().into())],
        code: vec![
            Instruction::Const(0),
            Instruction::NewArray { count: 1 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![],
        template_sites: vec![],
        source: "".into(),
    }
}
