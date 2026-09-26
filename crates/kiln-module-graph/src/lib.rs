//! Entry-rooted module graph construction and scalar module execution.
//!
//! This crate deliberately owns no filesystem or frontend policy. Callers provide a source
//! loader and a HIR frontend; the resolver is used only to apply the explicit, canonical
//! relative/absolute filesystem policy shared by the rest of Kiln.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use kiln_hir::Module;
use kiln_host_abi::HostOps;
use kiln_interpreter::{
    Cell, CommonJsModuleLoader, CommonJsRequire, ExecutionCapabilities, ExecutionError,
    ExecutionLimits, ExecutionResult, ImportAttributes, ModuleBinding, ModuleLoadError,
    ModuleLoadRequest, ModuleLoaderService, ModulePlan, ModuleRealmSession, ModuleSelfExport,
    ModuleSelfImport, ModuleValueRef, PlannedModule, Property, Value,
    execute_module_in_runtime_with_loader, execute_node_module_in_session_with_loader,
    ordinary_object, own_data_property, parse_json_module, read_through_cell,
};
use kiln_ir::{
    BYTECODE_VERSION, CompileDiagnostic, ExportBinding, Instruction, ModuleProgram, Program,
    cjs_static_module_syntax_diagnostics, compile_commonjs, compile_module,
};
use kiln_module_resolver::{
    ModuleResolver, NodeBuiltin, NodeModuleFormat, NodeRequestKind, PathRole, ResolutionKind,
    ResolutionPolicy, ResolvedModule, SUPPORTED_EXTENSIONS, is_asset_extension,
    node_format_is_ambiguous,
};
use kiln_runtime::{ClockCapability, RealmCapabilities, RealmRuntime};

const COMMONJS_WRAPPER_SOURCE: &str = "*commonjs-wrapper*";

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

/// Re-exported because [`ModuleGraphError::Resolve`] carries one: a caller that has to tell a
/// missing module from an unresolvable specifier should not need a second dependency to read it.
pub use kiln_module_resolver::ResolveError;

/// A callback that supplies source text for one canonical module path.
pub trait SourceLoader {
    type Error: fmt::Display;

    fn load(&mut self, path: &Path) -> Result<String, Self::Error>;
}

impl<F, E> SourceLoader for F
where
    F: FnMut(&Path) -> Result<String, E>,
    E: fmt::Display,
{
    type Error = E;

    fn load(&mut self, path: &Path) -> Result<String, E> {
        self(path)
    }
}

/// A callback that lowers one source file into Kiln's owned HIR.
pub trait Frontend {
    fn parse(&mut self, path: &str, source: &str) -> Module;

    /// Parse a CommonJS script and return its statically discoverable literal dependencies.
    /// Scope and hoisting semantics belong to the frontend that owns the source AST.
    fn parse_commonjs(&mut self, path: &str, source: &str) -> CommonJsParse;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonJsParse {
    pub module: Module,
    pub static_requires: Vec<String>,
}

impl<F> Frontend for F
where
    F: FnMut(&str, &str) -> Module,
{
    fn parse(&mut self, path: &str, source: &str) -> Module {
        self(path, source)
    }

    fn parse_commonjs(&mut self, path: &str, source: &str) -> CommonJsParse {
        CommonJsParse {
            module: self(path, source),
            static_requires: Vec::new(),
        }
    }
}

/// One canonical runtime edge in a module graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// The exact source specifier written by the importer.
    pub specifier: String,
    /// The canonical target selected by [`ModuleResolver`].
    pub target: PathBuf,
}

/// A literal lazy loader edge.  Unlike an ordinary dependency, its resolver base is captured
/// from the `createRequire` call site (which may be a different file than the module containing
/// the call), so both values are part of the graph identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDependency {
    pub importer: PathBuf,
    pub specifier: String,
    pub target: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFailure {
    pub importer: PathBuf,
    pub specifier: String,
    pub message: String,
}
fn runtime_importer_path(raw: &str) -> Result<PathBuf, String> {
    if raw.starts_with("file:") {
        let url =
            url::Url::parse(raw).map_err(|error| format!("invalid file URL `{raw}`: {error}"))?;
        return url
            .to_file_path()
            .map_err(|()| format!("file URL does not identify a local path: {raw}"));
    }
    Ok(PathBuf::from(raw))
}

/// Closed-world demand for one executable: which modules are live, which
/// export slots each one must keep, and which import edges are live.
///
/// Produced by whole-graph selection in `kiln-liveness` and consumed by
/// [`ModuleGraph::reduce_to_live`] plus the backends that need slot-level
/// detail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveDemand {
    /// Canonical paths of modules the executable evaluates.
    pub live: BTreeSet<PathBuf>,
    /// Demanded export slots per live module; drives slot-level pruning.
    pub slots: BTreeMap<PathBuf, BTreeSet<u32>>,
    /// Live import edges as `(importer, index into its `program.imports`)`.
    /// The CommonJS wrapper import is inert rather than live and is always
    /// retained; every other retained edge is live by construction.
    pub live_imports: BTreeSet<(PathBuf, usize)>,
}

/// A compiled canonical module and its resolved runtime dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleNode {
    pub path: PathBuf,
    /// Where this module's interface and implementation come from.
    ///
    /// Builtins retain their typed identity all the way through the graph. Their virtual path is
    /// only a stable graph key; it is never handed to a source loader or inferred from a suffix.
    pub source: ModuleSource,
    pub program: ModuleProgram,
    /// Authoritative source text for a data module. It is captured by the same provider as code
    /// modules and parsed once when the realm materializes the cached default export.
    pub data: Option<String>,
    pub dependencies: Vec<Dependency>,
    /// Literal loader targets retained for lazy `createRequire`/`getBuiltinModule` lookup.
    pub runtime_dependencies: Vec<RuntimeDependency>,
}

/// The provider responsible for one graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleSource {
    Source,
    CommonJs,
    Json,
    /// A file imported with `with { type: "file" }`. Assets are path values, not source text;
    /// retaining this identity prevents binary files from crossing a UTF-8 loader boundary.
    Asset,
    NodeBuiltin(NodeBuiltin),
}

/// The typed result supplied by a module provider.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum ModulePayload {
    Program(ModuleProgram),
    JsonText(String),
    AssetPath(PathBuf),
}

/// An entry-rooted module graph. ESM cycles are rejected during construction; CommonJS `require`
/// cycles are retained so execution can expose their cache's partially initialized exports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleGraph {
    entry: PathBuf,
    project_root: PathBuf,
    modules: Arc<BTreeMap<PathBuf, ModuleNode>>,
    /// Dependencies-first, deterministic (source-occurrence ordered) module order. CommonJS
    /// cycles appear once in the order, with the back-edge omitted from traversal.
    order: Arc<Vec<PathBuf>>,
    /// Link-time export identity for every visible, non-missing name.
    export_resolutions: Arc<HashMap<PathBuf, BTreeMap<String, ResolvedExport>>>,
    /// Body replacements layered over the shared immutable graph storage.
    patched_modules: Arc<BTreeMap<PathBuf, ModuleNode>>,
    dynamic_dependencies: Arc<HashMap<PathBuf, Vec<Dependency>>>,
    dynamic_failures: Arc<BTreeMap<(PathBuf, String), String>>,
    runtime_failures: Arc<BTreeMap<PathBuf, BTreeMap<(PathBuf, String), String>>>,
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
            .unwrap_or_else(|| self.graph.entry.clone());
        let node = self.graph.module(&importer).ok_or_else(|| {
            ModuleLoadError::NotFound(format!(
                "importer `{}` is not in the module graph",
                importer.display()
            ))
        })?;
        if let Some(message) = self
            .graph
            .dynamic_failures
            .get(&(importer.clone(), request.specifier.clone()))
        {
            return Err(ModuleLoadError::Unsupported(message.clone()));
        }
        let dependency = self
            .graph
            .dynamic_dependencies
            .get(&importer)
            .and_then(|dependencies| {
                dependencies
                    .iter()
                    .find(|dependency| dependency.specifier == request.specifier)
            })
            .or_else(|| {
                node.dependencies
                    .iter()
                    .find(|dependency| dependency.specifier == request.specifier)
            });
        let target = dependency
            .map(|dependency| dependency.target.clone())
            .or_else(|| {
                if let Some(builtin) = NodeBuiltin::parse(&request.specifier) {
                    self.graph.modules.iter().find_map(|(path, node)| {
                        (node.source == ModuleSource::NodeBuiltin(builtin)).then(|| path.clone())
                    })
                } else {
                    None
                }
            })
            .or_else(|| {
                let direct = PathBuf::from(&request.specifier);
                if self.graph.modules.contains_key(&direct) {
                    return Some(direct);
                }
                if let Some(parent) = importer.parent() {
                    let relative = parent.join(&request.specifier);
                    if self.graph.modules.contains_key(&relative) {
                        return Some(relative);
                    }
                    for ext in &[".ts", ".js", ".json", "/index.ts", "/index.js"] {
                        let candidate = parent.join(format!("{}{ext}", request.specifier));
                        if self.graph.modules.contains_key(&candidate) {
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

/// Link-time identity of one visible export.
///
/// A missing name has no entry. `Ambiguous` is retained so namespace construction can omit it and
/// consumers can report the distinction from a missing export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedExport {
    Local { module: PathBuf, local_slot: u32 },
    Namespace { module: PathBuf },
    CommonJsNamed { module: PathBuf, property: String },
    Ambiguous,
}

/// Closed-world analysis verdict for a module graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClosedWorldVerdict {
    Closed,
    Open { reasons: Vec<String> },
}

impl ClosedWorldVerdict {
    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }

    pub fn reasons(&self) -> &[String] {
        match self {
            Self::Closed => &[],
            Self::Open { reasons } => reasons,
        }
    }
}

/// User-facing failures produced while constructing or executing a graph.
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleGraphError {
    NonAbsoluteEntry {
        path: PathBuf,
    },
    Entry {
        path: PathBuf,
        kind: io::ErrorKind,
    },
    Source {
        path: PathBuf,
        message: String,
    },
    FrontendPath {
        path: PathBuf,
    },
    Compile {
        path: PathBuf,
        diagnostics: Vec<CompileDiagnostic>,
    },
    /// Static module syntax reached a CommonJS-classified file.
    ///
    /// Never surfaces directly: [`Builder::visit_static`] retries ambiguous files as ESM
    /// (Node 22+ syntax detection) and converts explicit-CJS ones into [`Self::Compile`].
    /// Providers return this instead of compiling so the retry decision stays in one place.
    CjsStaticModuleSyntax {
        path: PathBuf,
        diagnostics: Vec<CompileDiagnostic>,
    },
    Resolve {
        importer: PathBuf,
        specifier: String,
        error: ResolveError,
    },
    DuplicateExport {
        module: PathBuf,
        name: String,
    },
    UnresolvedImport {
        importer: PathBuf,
        source: String,
    },
    MissingExport {
        importer: PathBuf,
        provider: PathBuf,
        name: String,
    },
    AmbiguousExport {
        importer: PathBuf,
        provider: PathBuf,
        name: String,
    },
    Execution {
        module: PathBuf,
        error: ExecutionError,
    },
    StepOverflow,
    InternalMissingModule {
        path: PathBuf,
    },
}

impl fmt::Display for ModuleGraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonAbsoluteEntry { path } => {
                write!(f, "entry path must be absolute: {}", path.display())
            }
            Self::Entry { path, kind } => {
                write!(f, "could not open entry {} ({kind:?})", path.display())
            }
            Self::Source { path, message } => {
                write!(f, "could not load module {}: {message}", path.display())
            }
            Self::FrontendPath { path } => {
                write!(f, "module path is not valid UTF-8: {}", path.display())
            }
            Self::Compile { path, diagnostics } => {
                write!(f, "could not compile module {}", path.display())?;
                for diagnostic in diagnostics {
                    write!(f, ": {}", diagnostic.message)?;
                }
                Ok(())
            }
            Self::CjsStaticModuleSyntax { path, diagnostics } => {
                write!(f, "could not compile module {}", path.display())?;
                for diagnostic in diagnostics {
                    write!(f, ": {}", diagnostic.message)?;
                }
                Ok(())
            }
            Self::Resolve {
                importer,
                specifier,
                error,
            } => write!(
                f,
                "could not resolve `{specifier}` imported by {}: {error}",
                importer.display()
            ),
            Self::DuplicateExport { module, name } => {
                write!(
                    f,
                    "module {} exports `{name}` more than once",
                    module.display()
                )
            }
            Self::UnresolvedImport { importer, source } => {
                write!(
                    f,
                    "module {} has an unresolved import source `{source}`",
                    importer.display()
                )
            }
            Self::MissingExport {
                importer,
                provider,
                name,
            } => write!(
                f,
                "module {} imports missing export `{name}` from {}",
                importer.display(),
                provider.display()
            ),
            Self::AmbiguousExport {
                importer,
                provider,
                name,
            } => write!(
                f,
                "module {} imports ambiguous export `{name}` from {}",
                importer.display(),
                provider.display()
            ),
            Self::Execution { module, error } => {
                write!(f, "could not execute module {}: {error}", module.display())
            }
            Self::StepOverflow => f.write_str("module execution step count overflowed"),
            Self::InternalMissingModule { path } => {
                write!(
                    f,
                    "internal graph error: module {} is missing",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ModuleGraphError {}

impl ModuleGraph {
    /// Build a graph rooted at an absolute entry file.
    pub fn build<L, F>(
        entry: impl AsRef<Path>,
        project_root: impl AsRef<Path>,
        mut loader: L,
        mut frontend: F,
    ) -> Result<Self, ModuleGraphError>
    where
        L: SourceLoader,
        F: Frontend,
    {
        let provider = move |path: &Path| -> Result<ModuleProgram, ModuleGraphError> {
            let source = loader
                .load(path)
                .map_err(|error| ModuleGraphError::Source {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                })?;
            let path_string = path
                .to_str()
                .ok_or_else(|| ModuleGraphError::FrontendPath {
                    path: path.to_path_buf(),
                })?;
            let hir = frontend.parse(path_string, &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        };
        Self::build_with_provider(entry, project_root, provider)
    }

    /// Build using a caller-owned HIR provider.
    ///
    /// This is the integration seam for incremental databases and editor overlays: the graph
    /// performs traversal, resolution, and linking, while the provider can return an already
    /// memoized `engine.parse(path).module` without reading or reparsing source text.
    pub fn build_with_hir<P, E>(
        entry: impl AsRef<Path>,
        project_root: impl AsRef<Path>,
        mut provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path) -> Result<Module, E>,
        E: fmt::Display,
    {
        let mut provider = move |path: &Path| -> Result<ModuleProgram, ModuleGraphError> {
            let hir = provider(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        };
        Self::build_with_provider(entry, project_root, &mut provider)
    }

    /// Build using already-compiled module IR from a caller-owned provider.
    ///
    /// This is the lowest-level integration seam. A daemon can pass the result of its memoized
    /// `module_ir(file)` query and avoid both source I/O and a second compilation.
    pub fn build_with_modules<P>(
        entry: impl AsRef<Path>,
        project_root: impl AsRef<Path>,
        mut provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path) -> ModuleProgram,
    {
        let provider = move |path: &Path| Ok(provider(path));
        Self::build_with_provider(entry, project_root, provider)
    }

    /// Result-returning variant of [`Self::build_with_modules`] for providers that can report
    /// stale/missing memo entries. Compilation diagnostics should generally be handled before
    /// constructing the graph, because this boundary receives only `ModuleProgram` values.
    pub fn build_with_module_results<P, E>(
        entry: impl AsRef<Path>,
        project_root: impl AsRef<Path>,
        mut provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path) -> Result<ModuleProgram, E>,
        E: fmt::Display,
    {
        let provider = move |path: &Path| {
            provider(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })
        };
        Self::build_with_provider(entry, project_root, provider)
    }

    /// Build using a provider that can return already-structured graph errors.
    ///
    /// This is useful to callers whose memoized module query already distinguishes compile
    /// diagnostics from source/provider failures; those errors are propagated without flattening
    /// them into a display string.
    pub fn build_with_module_provider<P>(
        entry: impl AsRef<Path>,
        project_root: impl AsRef<Path>,
        provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path) -> Result<ModuleProgram, ModuleGraphError>,
    {
        Self::build_with_provider(entry, project_root, provider)
    }

    /// Build using a caller-selected resolution policy and already-structured module provider.
    ///
    /// The graph owns traversal and linking, while the policy owns how one source spelling maps
    /// to a canonical module. Keeping that policy behind [`ResolutionPolicy`] lets package and
    /// explicit-filesystem adapters share this graph without teaching the graph package rules.
    pub fn build_with_policy<P, R>(
        entry: impl AsRef<Path>,
        resolver: R,
        provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path) -> Result<ModuleProgram, ModuleGraphError>,
        R: ResolutionPolicy,
    {
        Self::build_with_policy_provider(entry, resolver, provider)
    }

    /// Build with a provider that receives the resolver-selected source kind.
    ///
    /// Node package format is resolution metadata, not a parser guess. This boundary lets a
    /// caller select the corresponding frontend goal and lowering path while leaving ordinary
    /// ESM-only callers source-compatible with [`Self::build_with_policy`].
    pub fn build_with_typed_policy<P, R>(
        entry: impl AsRef<Path>,
        resolver: R,
        provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path, ModuleSource) -> Result<ModulePayload, ModuleGraphError>,
        R: ResolutionPolicy,
    {
        Self::build_with_typed_policy_provider(entry, resolver, provider)
    }

    /// Build a Node-format graph directly from source.
    ///
    /// This is the canonical non-incremental adapter: the resolver chooses format, the frontend
    /// chooses the matching grammar goal, and this layer selects ESM or CommonJS lowering. Static
    /// literal `require()` calls become ordinary graph dependencies; dynamic require remains a
    /// runtime concern.
    pub fn build_node<L, F, R>(
        entry: impl AsRef<Path>,
        resolver: R,
        mut loader: L,
        mut frontend: F,
    ) -> Result<Self, ModuleGraphError>
    where
        L: SourceLoader,
        F: Frontend,
        R: ResolutionPolicy,
    {
        Self::build_with_typed_policy(entry, resolver, move |path, source_kind| {
            if source_kind == ModuleSource::Asset
                || path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(is_asset_extension)
            {
                return Ok(ModulePayload::AssetPath(path.to_path_buf()));
            }
            let source = loader
                .load(path)
                .map_err(|error| ModuleGraphError::Source {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                })?;
            let path_string = path
                .to_str()
                .ok_or_else(|| ModuleGraphError::FrontendPath {
                    path: path.to_path_buf(),
                })?;
            if source_kind == ModuleSource::Json {
                let json_text =
                    kiln_data_formats::parse_data_to_json(path, &source).map_err(|err| {
                        ModuleGraphError::Source {
                            path: path.to_path_buf(),
                            message: err.to_string(),
                        }
                    })?;
                return Ok(ModulePayload::JsonText(json_text));
            }
            let commonjs = (source_kind == ModuleSource::CommonJs)
                .then(|| frontend.parse_commonjs(path_string, &source));
            let hir = commonjs.as_ref().map_or_else(
                || frontend.parse(path_string, &source),
                |parsed| parsed.module.clone(),
            );
            if source_kind == ModuleSource::CommonJs && hir.has_static_module_syntax() {
                return Err(ModuleGraphError::CjsStaticModuleSyntax {
                    path: path.to_path_buf(),
                    diagnostics: cjs_static_module_syntax_diagnostics(&hir),
                });
            }
            let mut program = if source_kind == ModuleSource::CommonJs {
                compile_commonjs(&hir)
            } else {
                compile_module(&hir)
            }
            .map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })?;
            if source_kind == ModuleSource::CommonJs {
                program.dependencies = commonjs.expect("CommonJS parse exists").static_requires;
            }
            Ok(ModulePayload::Program(program))
        })
    }

    fn build_with_provider<P>(
        entry: impl AsRef<Path>,
        project_root: impl AsRef<Path>,
        provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path) -> Result<ModuleProgram, ModuleGraphError>,
    {
        let resolver =
            ModuleResolver::new(project_root).map_err(|error| ModuleGraphError::Resolve {
                importer: PathBuf::new(),
                specifier: String::new(),
                error,
            })?;
        Self::build_with_policy_provider(entry, resolver, provider)
    }

    fn build_with_policy_provider<P, R>(
        entry: impl AsRef<Path>,
        resolver: R,
        provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path) -> Result<ModuleProgram, ModuleGraphError>,
        R: ResolutionPolicy,
    {
        let mut provider = provider;
        Self::build_with_policy_provider_mode(
            entry,
            resolver,
            move |path, _source| provider(path).map(ModulePayload::Program),
            false,
        )
    }

    fn build_with_typed_policy_provider<P, R>(
        entry: impl AsRef<Path>,
        resolver: R,
        provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path, ModuleSource) -> Result<ModulePayload, ModuleGraphError>,
        R: ResolutionPolicy,
    {
        Self::build_with_policy_provider_mode(entry, resolver, provider, true)
    }

    fn build_with_policy_provider_mode<P, R>(
        entry: impl AsRef<Path>,
        resolver: R,
        provider: P,
        typed_sources: bool,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path, ModuleSource) -> Result<ModulePayload, ModuleGraphError>,
        R: ResolutionPolicy,
    {
        if !entry.as_ref().is_absolute() {
            return Err(ModuleGraphError::NonAbsoluteEntry {
                path: entry.as_ref().to_path_buf(),
            });
        }
        let entry_path =
            fs::canonicalize(entry.as_ref()).map_err(|error| ModuleGraphError::Entry {
                path: entry.as_ref().to_path_buf(),
                kind: error.kind(),
            })?;
        let root = resolver.project_root().to_path_buf();
        if !entry_path.starts_with(&root) {
            return Err(ModuleGraphError::Resolve {
                importer: entry_path,
                specifier: String::new(),
                error: ResolveError::OutsideProject {
                    path: entry.as_ref().to_path_buf(),
                    project_root: root,
                },
            });
        }
        if !entry_path.is_file() {
            return Err(ModuleGraphError::Resolve {
                importer: entry_path.clone(),
                specifier: String::new(),
                error: ResolveError::NotRegularFile {
                    role: PathRole::Target,
                    path: entry_path,
                },
            });
        }
        if !entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| SUPPORTED_EXTENSIONS.contains(&extension))
        {
            return Err(ModuleGraphError::Resolve {
                importer: entry_path.clone(),
                specifier: String::new(),
                error: ResolveError::UnsupportedExtension { path: entry_path },
            });
        }

        let mut builder = Builder {
            resolver,
            provider,
            typed_sources,
            modules: BTreeMap::new(),
            states: HashMap::new(),
            stack: Vec::new(),
            order: Vec::new(),
            dynamic_dependencies: HashMap::new(),
            dynamic_failures: HashMap::new(),
            runtime_failures: BTreeMap::new(),
            dynamic_work: Vec::new(),
            runtime_work: Vec::new(),
            dynamic_journal: None,
        };
        builder.visit(entry_path.clone())?;
        let export_resolutions = resolve_exports(&builder.modules, &builder.order)?;
        validate_links(&builder.modules, &builder.order, &export_resolutions)?;
        Ok(Self {
            entry: entry_path,
            project_root: builder.resolver.project_root().to_path_buf(),
            modules: Arc::new(builder.modules),
            order: Arc::new(builder.order),
            export_resolutions: Arc::new(export_resolutions),
            patched_modules: Arc::new(BTreeMap::new()),
            dynamic_dependencies: Arc::new(builder.dynamic_dependencies),
            dynamic_failures: Arc::new(builder.dynamic_failures.into_iter().collect()),
            runtime_failures: Arc::new(builder.runtime_failures),
        })
    }

    pub fn entry(&self) -> &Path {
        &self.entry
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    fn module_node(&self, path: &Path) -> Option<&ModuleNode> {
        self.patched_modules
            .get(path)
            .or_else(|| self.modules.get(path))
    }

    pub fn module(&self, path: impl AsRef<Path>) -> Option<&ModuleNode> {
        self.module_node(path.as_ref()).or_else(|| {
            fs::canonicalize(path.as_ref())
                .ok()
                .and_then(|canonical| self.module_node(&canonical))
        })
    }

    pub fn modules(&self) -> impl Iterator<Item = &ModuleNode> {
        self.order.iter().filter_map(|path| self.module_node(path))
    }

    pub fn module_order(&self) -> &[PathBuf] {
        &self.order
    }

    /// Dependencies reached by literal `import()` from one importer.
    pub fn dynamic_dependencies(&self, importer: impl AsRef<Path>) -> &[Dependency] {
        self.dynamic_dependencies
            .get(importer.as_ref())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Return the deferred failure recorded for one literal dynamic import, if any.
    pub fn dynamic_failure(&self, importer: impl AsRef<Path>, specifier: &str) -> Option<&str> {
        self.dynamic_failures
            .get(&(importer.as_ref().to_path_buf(), specifier.to_owned()))
            .map(String::as_str)
    }

    /// Return deferred dynamic failures in deterministic importer/specifier order.
    pub fn dynamic_failures(&self) -> impl Iterator<Item = (&(PathBuf, String), &String)> {
        self.dynamic_failures.iter()
    }

    pub fn runtime_failures(
        &self,
        owner: impl AsRef<Path>,
    ) -> impl Iterator<Item = (&(PathBuf, String), &String)> {
        self.runtime_failures
            .get(owner.as_ref())
            .into_iter()
            .flat_map(BTreeMap::iter)
    }

    /// Return the entry module and all modules reachable through static dependencies.
    ///
    /// Dynamic imports are intentionally excluded: callers use this set to identify startup
    /// evaluation and may materialize dynamic targets lazily.
    pub fn statically_reachable(&self) -> BTreeSet<PathBuf> {
        let mut reachable = BTreeSet::new();
        let mut pending = vec![self.entry.clone()];
        while let Some(path) = pending.pop() {
            if !reachable.insert(path.clone()) {
                continue;
            }
            if let Some(node) = self.module_node(&path) {
                pending.extend(
                    node.dependencies
                        .iter()
                        .map(|dependency| dependency.target.clone()),
                );
            }
        }
        reachable
    }

    /// Return a new graph with one source module's body replaced when its link shape is unchanged.
    ///
    /// This is intentionally copy-on-write at the graph boundary: the original graph is never
    /// mutated, so an interpreter or native build holding an older [`Arc<ModuleGraph>`] can keep
    /// using its immutable snapshot while the daemon prepares the replacement. The graph's order,
    /// resolved export table, and dependency targets are retained verbatim. `None` means the
    /// caller must perform a complete traversal and link; this is the safe fallback for an import,
    /// export, or dependency-shape edit.
    pub fn patch_module(&self, path: impl AsRef<Path>, program: ModuleProgram) -> Option<Self> {
        let path = path.as_ref();
        let canonical = if self.modules.contains_key(path) {
            path.to_path_buf()
        } else {
            fs::canonicalize(path).ok()?
        };
        let node = self.module_node(&canonical)?;
        if node.source != ModuleSource::Source
            || node.program.dependencies != program.dependencies
            || node.program.imports != program.imports
            || node.program.exports != program.exports
        {
            return None;
        }

        let mut patched_node = node.clone();
        patched_node.program = program;
        let mut patched = self.clone();
        let mut patched_modules = (*self.patched_modules).clone();
        patched_modules.insert(canonical, patched_node);
        patched.patched_modules = Arc::new(patched_modules);
        Some(patched)
    }

    /// Replace several programs and rebuild the graph's export tables.
    ///
    /// Unlike [`Self::patch_module`], this accepts a smaller import/export
    /// interface. It is intended for a closed-world specialization that has
    /// already reduced the graph to live nodes and reconciled every replacement
    /// with the node's resolved dependencies. Relinking is all-or-nothing: an
    /// invalid replacement returns the ordinary graph error and leaves `self`
    /// untouched.
    pub fn replace_programs_and_relink(
        &self,
        mut replacements: BTreeMap<PathBuf, ModuleProgram>,
    ) -> Result<Self, ModuleGraphError> {
        let mut modules = BTreeMap::new();
        for path in self.order.iter() {
            let mut node = self
                .module_node(path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?
                .clone();
            if let Some(program) = replacements.remove(path) {
                node.program = program;
            }
            modules.insert(path.clone(), node);
        }
        if let Some((path, _)) = replacements.into_iter().next() {
            return Err(ModuleGraphError::InternalMissingModule { path });
        }
        let export_resolutions = resolve_exports(&modules, &self.order)?;
        validate_links(&modules, &self.order, &export_resolutions)?;
        Ok(Self {
            entry: self.entry.clone(),
            project_root: self.project_root.clone(),
            modules: Arc::new(modules),
            order: Arc::clone(&self.order),
            export_resolutions: Arc::new(export_resolutions),
            patched_modules: Arc::new(BTreeMap::new()),
            dynamic_dependencies: Arc::clone(&self.dynamic_dependencies),
            dynamic_failures: Arc::clone(&self.dynamic_failures),
            runtime_failures: Arc::clone(&self.runtime_failures),
        })
    }

    /// Return the subgraph of live modules as a self-consistent graph.
    ///
    /// Filters `modules` and `order` to `demand.live` (dependency-first
    /// relative order preserved), prunes each live module's `program.imports`
    /// to its live edges (plus the inert CommonJS wrapper import) and its
    /// `program`/`node` dependencies to live targets, then rebuilds the
    /// export tables with [`resolve_exports`] and re-runs [`validate_links`].
    /// Every remaining import edge is live by construction, so consumers wire
    /// all of them: the interpreter loop skips dead modules with no
    /// per-engine liveness code, and the native launcher needs no dead-slot
    /// skipping. Returns the reduced graph with its remapped demand
    /// (import indices shift when dead edges are pruned).
    ///
    /// Errors loudly rather than silently changing linkage: a missing entry,
    /// a live edge naming a dropped target, or a re-link failure surfaces as
    /// [`ModuleGraphError`]. Dropping a module whose evaluation has effects
    /// the program can observe is a selection bug, not a reduction bug: only
    /// pass demands whose `live` set was computed closed-world, and note that
    /// packages declaring `sideEffects: false` opt their unimported modules
    /// out of evaluation, matching bundler semantics.
    pub fn reduce_to_live(
        &self,
        demand: &LiveDemand,
    ) -> Result<(Self, LiveDemand), ModuleGraphError> {
        if !demand.live.contains(&self.entry) {
            return Err(ModuleGraphError::InternalMissingModule {
                path: self.entry.clone(),
            });
        }
        let order: Vec<PathBuf> = self
            .order
            .iter()
            .filter(|path| demand.live.contains(*path))
            .cloned()
            .collect();
        let mut modules = BTreeMap::new();
        let mut remapped_imports = BTreeSet::new();
        let reduce_node = |node: &ModuleNode,
                           remapped_imports: &mut BTreeSet<(PathBuf, usize)>|
         -> Result<ModuleNode, ModuleGraphError> {
            let mut node = node.clone();
            let original_imports = std::mem::take(&mut node.program.imports);
            let original_dependencies = std::mem::take(&mut node.dependencies);
            node.dependencies = original_dependencies
                .iter()
                .filter(|dependency| demand.live.contains(&dependency.target))
                .cloned()
                .collect();
            node.runtime_dependencies = node
                .runtime_dependencies
                .iter()
                .filter(|dependency| demand.live.contains(&dependency.target))
                .cloned()
                .collect();
            let mut imports = Vec::with_capacity(original_imports.len());
            for (index, original_import) in original_imports.iter().enumerate() {
                let live = demand.live_imports.contains(&(node.path.clone(), index));
                // An indirect re-export is an import into a synthetic slot
                // plus an export of that slot. The module body does not read
                // it, but resolve_exports still needs the binding metadata.
                let links_export = node
                    .program
                    .exports
                    .iter()
                    .any(|export| export.local_slot == original_import.local_slot);
                if !live && !links_export && original_import.source != COMMONJS_WRAPPER_SOURCE {
                    continue;
                }

                let mut import = original_import.clone();
                if live && import.source != COMMONJS_WRAPPER_SOURCE {
                    let dependency = original_dependencies
                        .iter()
                        .find(|dependency| dependency.specifier == import.source)
                        .ok_or_else(|| ModuleGraphError::UnresolvedImport {
                            importer: node.path.clone(),
                            source: import.source.clone(),
                        })?;
                    if !demand.live.contains(&dependency.target) {
                        let (target, imported) =
                            match self.resolved_export(&dependency.target, &import.imported) {
                                Some(ResolvedExport::Local { module, local_slot }) => {
                                    let imported = self
                                        .module(module)
                                        .and_then(|origin| {
                                            origin.program.exports.iter().find(|export| {
                                                export.local_slot == *local_slot
                                                    && !export
                                                        .exported
                                                        .starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
                                            })
                                        })
                                        .map(|export| export.exported.clone())
                                        .ok_or_else(|| ModuleGraphError::MissingExport {
                                            importer: node.path.clone(),
                                            provider: module.clone(),
                                            name: import.imported.clone(),
                                        })?;
                                    (module.clone(), imported)
                                }
                                Some(ResolvedExport::CommonJsNamed { module, property }) => {
                                    (module.clone(), property.clone())
                                }
                                Some(ResolvedExport::Namespace { module }) => {
                                    (module.clone(), kiln_ir::NAMESPACE_IMPORT.to_owned())
                                }
                                Some(ResolvedExport::Ambiguous) | None => {
                                    return Err(ModuleGraphError::MissingExport {
                                        importer: node.path.clone(),
                                        provider: dependency.target.clone(),
                                        name: import.imported.clone(),
                                    });
                                }
                            };
                        if !demand.live.contains(&target) {
                            return Err(ModuleGraphError::InternalMissingModule { path: target });
                        }
                        let specifier = format!("{}#kiln-live-{index}", import.source);
                        import.source = specifier.clone();
                        import.imported = imported;
                        node.dependencies.push(Dependency { specifier, target });
                    }
                }

                let next_index = imports.len();
                if live {
                    remapped_imports.insert((node.path.clone(), next_index));
                }
                imports.push(import);
            }
            node.program.imports = imports;
            Ok(node)
        };
        for path in &order {
            let node = self
                .modules
                .get(path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
            let node = reduce_node(node, &mut remapped_imports)?;
            modules.insert(path.clone(), node);
        }
        let mut patched_modules = BTreeMap::new();
        for (path, node) in self.patched_modules.iter() {
            if !demand.live.contains(path) {
                continue;
            }
            let node = reduce_node(node, &mut remapped_imports)?;
            patched_modules.insert(path.clone(), node);
        }
        let export_resolutions = resolve_exports(&modules, &order)?;
        validate_links(&modules, &order, &export_resolutions)?;
        let mut slots = BTreeMap::new();
        for (path, set) in demand.slots.iter() {
            if demand.live.contains(path) {
                slots.insert(path.clone(), set.clone());
            }
        }
        // Dynamic imports are resolved by the loader attached to the reduced graph, so carry
        // only importer/target pairs that still exist in this closed-world executable. A stale
        // edge would let a lazy request reach a module the evaluator deliberately removed.
        let dynamic_dependencies = self
            .dynamic_dependencies
            .iter()
            .filter(|(importer, _)| demand.live.contains(*importer))
            .map(|(importer, dependencies)| {
                (
                    importer.clone(),
                    dependencies
                        .iter()
                        .filter(|dependency| demand.live.contains(&dependency.target))
                        .cloned()
                        .collect(),
                )
            })
            .filter(|(_, dependencies): &(PathBuf, Vec<Dependency>)| !dependencies.is_empty())
            .collect();
        // Failed dynamic resolutions have no target to retain, but their importer-scoped error
        // is observable and must survive reduction for a live importer.
        let dynamic_failures = self
            .dynamic_failures
            .iter()
            .filter(|((importer, _), _)| demand.live.contains(importer))
            .map(|(key, message)| (key.clone(), message.clone()))
            .collect();
        let runtime_failures = self
            .runtime_failures
            .iter()
            .filter(|(owner, _)| demand.live.contains(*owner))
            .map(|(owner, failures)| (owner.clone(), failures.clone()))
            .collect();
        Ok((
            Self {
                entry: self.entry.clone(),
                project_root: self.project_root.clone(),
                modules: Arc::new(modules),
                order: Arc::new(order),
                export_resolutions: Arc::new(export_resolutions),
                patched_modules: Arc::new(patched_modules),
                dynamic_dependencies: Arc::new(dynamic_dependencies),
                dynamic_failures: Arc::new(dynamic_failures),
                runtime_failures: Arc::new(runtime_failures),
            },
            LiveDemand {
                live: demand.live.clone(),
                slots,
                live_imports: remapped_imports,
            },
        ))
    }

    /// Return the canonical link-time origin of `name` as exported by `module`.
    ///
    /// `None` means the name is missing. Callers that construct their own module ABI wiring must
    /// use this result rather than searching the provider's raw [`ModuleProgram::exports`] table:
    /// a star export has no raw entry for each name it forwards.
    pub fn resolved_export(&self, module: impl AsRef<Path>, name: &str) -> Option<&ResolvedExport> {
        let module = module.as_ref();
        self.export_resolutions
            .get(module)
            .or_else(|| {
                fs::canonicalize(module)
                    .ok()
                    .and_then(|canonical| self.export_resolutions.get(&canonical))
            })?
            .get(name)
    }

    /// Determine whether the module graph satisfies the closed-world invariants:
    /// - No direct `eval`
    /// - No `new Function` / `Function(...)`
    /// - No out-of-graph dynamic imports
    pub fn closed_world_verdict(&self) -> ClosedWorldVerdict {
        let mut reasons = Vec::new();
        for node in self.modules() {
            if node.program.link_facts.direct_eval {
                reasons.push(format!("{}: contains direct eval", node.path.display()));
            }
            if node.program.link_facts.new_function {
                reasons.push(format!(
                    "{}: contains Function constructor call",
                    node.path.display()
                ));
            }
            for dynamic_import in &node.program.link_facts.dynamic_imports {
                match dynamic_import {
                    kiln_ir::DynamicImportTarget::Dynamic => {
                        reasons.push(format!(
                            "{}: dynamic import has non-static specifier",
                            node.path.display()
                        ));
                    }
                    kiln_ir::DynamicImportTarget::Static(specifier) => {
                        if self.dynamic_failure(&node.path, specifier).is_some() {
                            continue;
                        }
                        let target_in_graph =
                            node.dependencies.iter().any(|d| {
                                d.specifier == *specifier && self.module(&d.target).is_some()
                            }) || self.module(specifier).is_some()
                                || self.dynamic_dependencies.get(&node.path).is_some_and(
                                    |dependencies| {
                                        dependencies.iter().any(|d| {
                                            d.specifier == *specifier
                                                && self.module(&d.target).is_some()
                                        })
                                    },
                                )
                                || node.path.parent().is_some_and(|dir| {
                                    let candidate = dir.join(specifier);
                                    self.module(&candidate).is_some()
                                        || fs::canonicalize(&candidate)
                                            .ok()
                                            .and_then(|canon| self.module(&canon))
                                            .is_some()
                                        || [".js", ".mjs", ".cjs", ".json"].iter().any(|ext| {
                                            let with_ext = dir.join(format!("{specifier}{ext}"));
                                            self.module(&with_ext).is_some()
                                                || fs::canonicalize(&with_ext)
                                                    .ok()
                                                    .and_then(|canon| self.module(&canon))
                                                    .is_some()
                                        })
                                });
                        if !target_in_graph {
                            reasons.push(format!(
                                "{}: dynamic import \"{}\" targets module outside graph",
                                node.path.display(),
                                specifier
                            ));
                        }
                    }
                }
            }
        }
        if reasons.is_empty() {
            ClosedWorldVerdict::Closed
        } else {
            ClosedWorldVerdict::Open { reasons }
        }
    }

    /// Convenience predicate for closed-world status.
    pub fn is_closed_world(&self) -> bool {
        self.closed_world_verdict().is_closed()
    }

    /// Return the immutable bindings declared/exported by a specific module.
    pub fn immutable_bindings(&self, module_path: impl AsRef<Path>) -> Option<&[String]> {
        self.module(module_path)
            .map(|node| node.program.link_facts.immutable_bindings.as_slice())
    }

    /// Return all immutable module bindings across the graph, keyed by module path.
    pub fn all_immutable_bindings(&self) -> BTreeMap<PathBuf, Vec<String>> {
        self.modules()
            .map(|node| {
                (
                    node.path.clone(),
                    node.program.link_facts.immutable_bindings.clone(),
                )
            })
            .collect()
    }

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
    ) -> Result<Cell, ModuleGraphError> {
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
                });
            }
            None => {
                return Err(ModuleGraphError::MissingExport {
                    importer: importer.to_path_buf(),
                    provider: provider.to_path_buf(),
                    name: import.imported.clone(),
                });
            }
        };
        // The resolution names a slot; the finished module will publish names. The name that
        // slot is exported under is what bridges the two, and it is the key the cell waits on.
        let exported = self
            .module_node(&origin)
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
                    .module_node(module)?
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

    /// Return the sorted names that are visibly exported by `module`.
    ///
    /// Ambiguous star exports are intentionally omitted: they remain available through
    /// [`Self::resolved_export`] so named-import callers can report the distinction from a missing
    /// name, but they are not part of a namespace's visible surface. The lookup accepts either a
    /// canonical graph path or a path that canonicalizes to one; an unknown module returns `None`.
    pub fn resolved_export_names(&self, module: impl AsRef<Path>) -> Option<Vec<String>> {
        let module = module.as_ref();
        let resolutions = self.export_resolutions.get(module).or_else(|| {
            fs::canonicalize(module)
                .ok()
                .and_then(|canonical| self.export_resolutions.get(&canonical))
        })?;
        Some(
            resolutions
                .iter()
                .filter_map(|(name, resolution)| match resolution {
                    ResolvedExport::Local { .. }
                    | ResolvedExport::Namespace { .. }
                    | ResolvedExport::CommonJsNamed { .. } => Some(name.clone()),
                    ResolvedExport::Ambiguous => None,
                })
                .collect(),
        )
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
    ) -> Result<ExecutionResult, ModuleGraphError> {
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
    pub fn import_plan(&self) -> Result<ModulePlan, ModuleGraphError> {
        // Every slot anything in this graph will want out of any module, gathered before an
        // epilogue is written: a module's array has to hold every value its importers, and the
        // entry's namespace, will later look up by index.
        let mut wanted: BTreeMap<PathBuf, BTreeSet<u32>> = BTreeMap::new();
        for resolutions in self.export_resolutions.values() {
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
            .order
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
                .module_node(path)
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
                });
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
                        });
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
                    });
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
            .plan_namespace(&self.entry, &slot_index)
            .ok_or_else(|| ModuleGraphError::InternalMissingModule {
                path: self.entry.clone(),
            })?;
        Ok(ModulePlan {
            key: self.entry.to_string_lossy().into_owned(),
            modules,
            namespace,
        })
    }

    /// Build a plan rooted at a particular graph node, retaining the immutable graph snapshot.
    /// Dynamic imports use this instead of accidentally returning the original entry plan.
    pub fn import_plan_for(&self, entry: impl AsRef<Path>) -> Result<ModulePlan, ModuleGraphError> {
        let entry = entry.as_ref();
        let canonical = if self.modules.contains_key(entry) {
            entry.to_path_buf()
        } else {
            fs::canonicalize(entry).map_err(|_| ModuleGraphError::InternalMissingModule {
                path: entry.to_path_buf(),
            })?
        };
        if self.module_node(&canonical).is_none() {
            return Err(ModuleGraphError::InternalMissingModule { path: canonical });
        }
        let mut rooted = self.clone();
        rooted.entry = canonical;
        let mut reachable = HashSet::new();
        let mut pending = vec![rooted.entry.clone()];
        while let Some(path) = pending.pop() {
            if !reachable.insert(path.clone()) {
                continue;
            }
            if let Some(node) = rooted.module_node(&path) {
                pending.extend(
                    node.dependencies
                        .iter()
                        .map(|dependency| dependency.target.clone()),
                );
            }
        }
        rooted.order = Arc::new(
            rooted
                .order
                .iter()
                .filter(|path| reachable.contains(*path))
                .cloned()
                .collect(),
        );
        rooted.import_plan()
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
    ) -> Result<ModulePlan, ModuleGraphError> {
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
            });
        }
        if let Some(ImportAttributes { type_: Some(kind) }) = attributes {
            if kind != "file" {
                return Err(ModuleGraphError::Source {
                    path: node.path.clone(),
                    message: "unsupported import attribute `type`".to_owned(),
                });
            }
            if node.source != ModuleSource::Asset {
                return Err(ModuleGraphError::Source {
                    path: node.path.clone(),
                    message: format!("module `{}` is not a file asset", node.path.display()),
                });
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
        let resolutions = self.export_resolutions.get(module)?;
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
    ) -> Result<ExecutionResult, ModuleGraphError> {
        self.execute_mode(host, limits, capabilities, true)
    }

    fn execute_mode<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        capabilities: ExecutionCapabilities,
        node: bool,
    ) -> Result<ExecutionResult, ModuleGraphError> {
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
            .modules
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
            ModuleGraphError::Execution {
                module: self.entry.clone(),
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

        {
            // `URL` and `Date` in every mode, as the native runtime has it; the fetch and web classes,
            // which are built on it, under the adapter.
            let date = kiln_builtins::DATE_BOOTSTRAP_SOURCE;
            let source = if node {
                format!(
                    "{date}\n{}\n{}",
                    kiln_builtins::URL_BOOTSTRAP_SOURCE,
                    kiln_builtins::WEB_BOOTSTRAP_SOURCE
                )
            } else {
                // The Buffer bootstrap (Node mode) captures and deletes this bridge; without
                // it the bridge would be the one global node does not have -- the native
                // bootstrap does the same.
                format!(
                    "{date}\n{}\ndelete globalThis.__kilnBufferToString;\n",
                    kiln_builtins::URL_BOOTSTRAP_SOURCE
                )
            };
            let hir = frontend_oxc::parse("@kiln/web-bootstrap.ts", &source);
            let bootstrap_program =
                compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                    path: PathBuf::from("@kiln/web-bootstrap.ts"),
                    diagnostics,
                })?;
            let res = execute_node_module_in_session_with_loader(
                &bootstrap_program,
                &[],
                &[],
                host,
                limits,
                &mut runtime,
                node_session.as_mut().expect("Node mode owns a session"),
                capabilities.clone(),
                false,
                &mut commonjs_loader,
                true,
                Some("@kiln/web-bootstrap.ts"),
            )
            .map_err(|error| ModuleGraphError::Execution {
                module: PathBuf::from("@kiln/web-bootstrap.ts"),
                error,
            })?;
            total_steps = total_steps.saturating_add(res.execution.steps);
        }

        let static_reachable = self.statically_reachable();
        for path in self.order.iter() {
            if !static_reachable.contains(path) {
                continue;
            }
            let module_node = self
                .module_node(path)
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
                let value =
                    parse_json_module(source).map_err(|error| ModuleGraphError::Execution {
                        module: path.clone(),
                        error,
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
                if path == &self.entry {
                    commonjs_loader
                        .execute_target(path, true)
                        .map_err(|error| ModuleGraphError::Execution {
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
                        .module_node(&dependency.target)
                        .is_some_and(|node| node.source == ModuleSource::CommonJs)
                {
                    commonjs_loader
                        .execute_target(&dependency.target, true)
                        .map_err(|error| ModuleGraphError::Execution {
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
                                let origin_node = self.module_node(origin);
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
                            });
                        }
                        None => {
                            return Err(ModuleGraphError::MissingExport {
                                importer: path.clone(),
                                provider: dependency.target.clone(),
                                name: import.imported.clone(),
                            });
                        }
                    },
                };
                imports.push(value);
            }

            if total_steps >= limits.max_steps {
                return Err(ModuleGraphError::Execution {
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
                path == &self.entry,
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
                ModuleGraphError::Execution {
                    module: path.clone(),
                    error,
                }
            })?;
            total_steps = total_steps
                .checked_add(result.execution.steps)
                .ok_or(ModuleGraphError::StepOverflow)?;
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
                            self.export_resolutions
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
            if let Some(resolutions) = self.export_resolutions.get(path) {
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
                                let origin_node = self.module_node(origin);
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
                                let origin_node = self.module_node(origin);
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
            if path == &self.entry {
                entry_result = Some(result.execution);
            }
        }

        let mut result = entry_result.ok_or_else(|| ModuleGraphError::InternalMissingModule {
            path: self.entry.clone(),
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
        let Some(module_node) = self.graph.module_node(target) else {
            return Err(ExecutionError::Host {
                instruction,
                message: format!(
                    "CommonJS runtime target `{}` is not in the graph",
                    target.display()
                ),
            });
        };
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
                && let Some(node) = self.graph.module_node(&dependency.target)
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
        let Some(module_node) = self.graph.module_node(path) else {
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
            let value =
                if let Some(cell) = self.commonjs_cache.get(&dependency.target) {
                    std::rc::Rc::clone(cell)
                } else if let Some(provider) = unsafe { (&*self.values).get(&dependency.target) } {
                    if self
                        .graph
                        .module_node(&dependency.target)
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
            Value::CommonJsRequire(std::rc::Rc::new(CommonJsRequire::new(targets))),
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
        let is_entry = path == self.graph.entry;
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
            if path == self.graph.entry {
                *self.entry_result = Some(result.execution);
            }
        }
        Ok(())
    }
}

impl<H: HostOps + ?Sized> CommonJsModuleLoader for CommonJsLoader<'_, H> {
    fn record_thrown_value(&mut self, value: Value) {
        self.pending_thrown = Some(value);
    }

    fn take_thrown_value(&mut self) -> Option<Value> {
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
        let path = self.graph.modules.iter().find_map(|(path, node)| {
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
                    .modules
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
                    .modules
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
                    .module_node(&importer)
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
                    if self.graph.module_node(&target).is_some_and(|node| {
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

struct Builder<P, R> {
    resolver: R,
    provider: P,
    typed_sources: bool,
    modules: BTreeMap<PathBuf, ModuleNode>,
    states: HashMap<PathBuf, VisitState>,
    stack: Vec<PathBuf>,
    order: Vec<PathBuf>,
    dynamic_dependencies: HashMap<PathBuf, Vec<Dependency>>,
    dynamic_failures: HashMap<(PathBuf, String), String>,
    runtime_failures: BTreeMap<PathBuf, BTreeMap<(PathBuf, String), String>>,
    dynamic_work: Vec<DynamicWork>,
    runtime_work: Vec<RuntimeWork>,
    dynamic_journal: Option<DynamicJournal>,
}

enum WorkItem {
    Enter {
        path: PathBuf,
        source: Option<ModuleSource>,
    },
    /// A module whose own compilation is finished, waiting below its dependencies' work.
    ///
    /// The payload is exactly the [`ModuleNode`] this will become, so it is spelled as one --
    /// and boxed, because it is an order of magnitude larger than `Enter` and every item in the
    /// queue would otherwise be sized for it.
    Exit(Box<ModuleNode>),
}

struct DynamicWork {
    importer: PathBuf,
    specifier: String,
    path: PathBuf,
    source: ModuleSource,
}

struct RuntimeWork {
    owner: PathBuf,
    importer: PathBuf,
    specifier: String,
    path: PathBuf,
    source: ModuleSource,
}

enum DeferredWork {
    Dynamic(DynamicWork),
    Runtime(RuntimeWork),
}

#[derive(Default)]
struct DynamicJournal {
    order_before: usize,
    stack_before: usize,
    dynamic_pending_before: usize,
    runtime_pending_before: usize,
    new_modules: Vec<PathBuf>,
    new_states: Vec<PathBuf>,
    new_dynamic_dependencies: Vec<(PathBuf, String)>,
    new_failures: Vec<(PathBuf, String)>,
    new_runtime_failures: Vec<(PathBuf, PathBuf, String)>,
}

#[derive(Clone, Copy)]
enum VisitState {
    Visiting,
    Done,
}

impl<P, R> Builder<P, R>
where
    P: FnMut(&Path, ModuleSource) -> Result<ModulePayload, ModuleGraphError>,
    R: ResolutionPolicy,
{
    fn visit(&mut self, path: PathBuf) -> Result<(), ModuleGraphError> {
        self.visit_static(path, None)?;
        let export_resolutions = resolve_exports(&self.modules, &self.order)?;
        validate_links(&self.modules, &self.order, &export_resolutions)?;
        loop {
            let Some(work) = self
                .dynamic_work
                .pop()
                .map(DeferredWork::Dynamic)
                .or_else(|| self.runtime_work.pop().map(DeferredWork::Runtime))
            else {
                break;
            };
            match work {
                DeferredWork::Dynamic(dynamic) => {
                    self.dynamic_journal = Some(DynamicJournal {
                        order_before: self.order.len(),
                        stack_before: self.stack.len(),
                        dynamic_pending_before: self.dynamic_work.len(),
                        runtime_pending_before: self.runtime_work.len(),
                        ..DynamicJournal::default()
                    });
                    let result = self
                        .visit_static(dynamic.path.clone(), Some(dynamic.source))
                        .and_then(|_| {
                            let export_resolutions = resolve_exports(&self.modules, &self.order)?;
                            validate_links(&self.modules, &self.order, &export_resolutions)
                        });
                    let journal = self.dynamic_journal.take().expect("dynamic journal exists");
                    if let Err(error) = result {
                        self.rollback_dynamic(journal);
                        self.remove_dynamic_dependency(&dynamic.importer, &dynamic.specifier);
                        self.dynamic_failures.insert(
                            (dynamic.importer, dynamic.specifier),
                            format!("dynamic import failed: {error}"),
                        );
                    }
                }
                DeferredWork::Runtime(runtime) => {
                    self.dynamic_journal = Some(DynamicJournal {
                        order_before: self.order.len(),
                        stack_before: self.stack.len(),
                        dynamic_pending_before: self.dynamic_work.len(),
                        runtime_pending_before: self.runtime_work.len(),
                        ..DynamicJournal::default()
                    });
                    let result = self
                        .visit_static(runtime.path.clone(), Some(runtime.source))
                        .and_then(|_| {
                            let export_resolutions = resolve_exports(&self.modules, &self.order)?;
                            validate_links(&self.modules, &self.order, &export_resolutions)
                        });
                    let journal = self.dynamic_journal.take().expect("runtime journal exists");
                    if let Err(error) = result {
                        self.rollback_dynamic(journal);
                        self.remove_runtime_dependency(
                            &runtime.owner,
                            &runtime.importer,
                            &runtime.specifier,
                        );
                        self.record_runtime_failure(
                            runtime.owner,
                            runtime.importer,
                            runtime.specifier,
                            format!("runtime request failed: {error}"),
                        );
                    }
                }
            }
        }
        while let Some(runtime) = self.runtime_work.pop() {
            self.dynamic_journal = Some(DynamicJournal {
                order_before: self.order.len(),
                stack_before: self.stack.len(),
                runtime_pending_before: self.runtime_work.len(),
                ..DynamicJournal::default()
            });
            let result = self
                .visit_static(runtime.path.clone(), Some(runtime.source))
                .and_then(|_| {
                    let export_resolutions = resolve_exports(&self.modules, &self.order)?;
                    validate_links(&self.modules, &self.order, &export_resolutions)
                });
            let journal = self.dynamic_journal.take().expect("runtime journal exists");
            if let Err(error) = result {
                self.rollback_runtime(journal);
                self.remove_runtime_dependency(
                    &runtime.owner,
                    &runtime.importer,
                    &runtime.specifier,
                );
                self.record_runtime_failure(
                    runtime.owner,
                    runtime.importer,
                    runtime.specifier,
                    format!("runtime request failed: {error}"),
                );
            }
        }
        Ok(())
    }

    fn rollback_dynamic(&mut self, journal: DynamicJournal) {
        self.dynamic_work.truncate(journal.dynamic_pending_before);
        self.runtime_work.truncate(journal.runtime_pending_before);
        self.stack.truncate(journal.stack_before);
        self.order.truncate(journal.order_before);
        for path in journal.new_modules {
            self.modules.remove(&path);
        }
        for path in journal.new_states {
            self.states.remove(&path);
        }
        for (importer, specifier) in journal.new_dynamic_dependencies {
            if let Some(dependencies) = self.dynamic_dependencies.get_mut(&importer) {
                if let Some(index) = dependencies
                    .iter()
                    .position(|dependency| dependency.specifier == specifier)
                {
                    dependencies.remove(index);
                }
                if dependencies.is_empty() {
                    self.dynamic_dependencies.remove(&importer);
                }
            }
        }
        for key in journal.new_failures {
            self.dynamic_failures.remove(&key);
        }
        for (owner, importer, specifier) in journal.new_runtime_failures {
            if let Some(failures) = self.runtime_failures.get_mut(&owner) {
                failures.remove(&(importer, specifier));
                if failures.is_empty() {
                    self.runtime_failures.remove(&owner);
                }
            }
        }
    }

    fn rollback_runtime(&mut self, journal: DynamicJournal) {
        self.runtime_work.truncate(journal.runtime_pending_before);
        self.stack.truncate(journal.stack_before);
        self.order.truncate(journal.order_before);
        for path in journal.new_modules {
            self.modules.remove(&path);
        }
        for path in journal.new_states {
            self.states.remove(&path);
        }
    }

    fn record_state(&mut self, path: &Path) {
        if !self.states.contains_key(path) {
            if let Some(journal) = &mut self.dynamic_journal {
                journal.new_states.push(path.to_path_buf());
            }
        }
        self.states.insert(path.to_path_buf(), VisitState::Visiting);
    }

    fn record_module(&mut self, node: ModuleNode) {
        if !self.modules.contains_key(&node.path) {
            if let Some(journal) = &mut self.dynamic_journal {
                journal.new_modules.push(node.path.clone());
            }
        }
        self.modules.insert(node.path.clone(), node);
    }

    fn record_dynamic_dependency(&mut self, path: &Path, dependency: Dependency) {
        if let Some(journal) = &mut self.dynamic_journal {
            journal
                .new_dynamic_dependencies
                .push((path.to_path_buf(), dependency.specifier.clone()));
        }
        self.dynamic_dependencies
            .entry(path.to_path_buf())
            .or_default()
            .push(dependency);
    }

    fn remove_dynamic_dependency(&mut self, importer: &Path, specifier: &str) {
        if let Some(dependencies) = self.dynamic_dependencies.get_mut(importer) {
            if let Some(index) = dependencies
                .iter()
                .position(|dependency| dependency.specifier == specifier)
            {
                dependencies.remove(index);
            }
            if dependencies.is_empty() {
                self.dynamic_dependencies.remove(importer);
            }
        }
    }

    fn record_dynamic_failure(&mut self, key: (PathBuf, String), message: String) {
        if !self.dynamic_failures.contains_key(&key) {
            if let Some(journal) = &mut self.dynamic_journal {
                journal.new_failures.push(key.clone());
            }
        }
        self.dynamic_failures.insert(key, message);
    }

    fn record_runtime_failure(
        &mut self,
        owner: PathBuf,
        importer: PathBuf,
        specifier: String,
        message: String,
    ) {
        let key = (importer.clone(), specifier.clone());
        let failures = self.runtime_failures.entry(owner.clone()).or_default();
        if !failures.contains_key(&key) {
            if let Some(journal) = &mut self.dynamic_journal {
                journal.new_runtime_failures.push((
                    owner.clone(),
                    importer.clone(),
                    specifier.clone(),
                ));
            }
        }
        failures.insert(key, message);
    }

    fn remove_runtime_dependency(&mut self, owner: &Path, importer: &Path, specifier: &str) {
        if let Some(node) = self.modules.get_mut(owner) {
            node.runtime_dependencies.retain(|dependency| {
                !(dependency.importer == importer && dependency.specifier == specifier)
            });
        }
    }

    fn visit_static(
        &mut self,
        path: PathBuf,
        source: Option<ModuleSource>,
    ) -> Result<(), ModuleGraphError> {
        let mut work = vec![WorkItem::Enter { path, source }];
        while let Some(item) = work.pop() {
            match item {
                WorkItem::Enter { path, source } => match self.states.get(&path).copied() {
                    Some(VisitState::Done) => continue,
                    // A back edge: this module is an ancestor of itself, so the graph has a
                    // cycle. The edge stays recorded on the parent that is being built; only the
                    // descent stops, and that is the whole of what a cycle costs here.
                    //
                    // Stopping the descent is not an approximation of the specification's
                    // `InnerModuleEvaluation` -- it *is* it. That algorithm returns immediately
                    // when it meets a module whose `[[Status]]` is already `evaluating`, and
                    // evaluates each body on the way back out of the recursion. So the order a
                    // depth-first walk leaves behind, with back edges skipped, is exactly the
                    // order the specification evaluates a cyclic graph in. `[[DFSIndex]]` and
                    // `[[DFSAncestorIndex]]` compute a cycle *root*, which decides where an
                    // asynchronous cycle's completion is collected and across which modules an
                    // evaluation error is recorded -- neither of which changes the order.
                    Some(VisitState::Visiting) => continue,
                    None => {
                        let source = match source {
                            Some(source) => source,
                            None => match ModuleSource::Source {
                                ModuleSource::Source
                                    if path
                                        .extension()
                                        .and_then(|extension| extension.to_str())
                                        .is_some_and(is_asset_extension) =>
                                {
                                    ModuleSource::Asset
                                }
                                ModuleSource::Source if !self.typed_sources => ModuleSource::Source,
                                ModuleSource::Source => {
                                    if path
                                        .extension()
                                        .and_then(|extension| extension.to_str())
                                        .is_some_and(kiln_data_formats::is_data_extension)
                                    {
                                        ModuleSource::Json
                                    } else {
                                        match self
                                            .resolver
                                            .node_format(&ResolvedModule {
                                                path: path.clone(),
                                                kind: ResolutionKind::Relative,
                                            })
                                            .map_err(|error| ModuleGraphError::Resolve {
                                                importer: path.clone(),
                                                specifier: String::new(),
                                                error,
                                            })? {
                                            Some(NodeModuleFormat::CommonJs) => {
                                                ModuleSource::CommonJs
                                            }
                                            _ => ModuleSource::Source,
                                        }
                                    }
                                }
                                source => source,
                            },
                        };
                        self.record_state(&path);
                        self.stack.push(path.clone());
                        // Node 22+ syntax detection: a CommonJS-classified file that is
                        // ambiguous (no definitive extension, no explicit `"type"`) and
                        // carries static module syntax runs as ESM. The provider signals
                        // the syntax instead of compiling so this retry stays in one
                        // place; explicit-CJS files keep the compile refusal. The retry
                        // cannot loop: ESM requests never produce the signal.
                        let mut source = source;
                        let (program, data) = match source {
                            ModuleSource::Source | ModuleSource::CommonJs => {
                                let payload = loop {
                                    match (self.provider)(&path, source) {
                                        Err(ModuleGraphError::CjsStaticModuleSyntax {
                                            diagnostics: _,
                                            ..
                                        }) if source == ModuleSource::CommonJs
                                            && node_format_is_ambiguous(&path) =>
                                        {
                                            source = ModuleSource::Source;
                                            continue;
                                        }
                                        Err(ModuleGraphError::CjsStaticModuleSyntax {
                                            path,
                                            diagnostics,
                                        }) => {
                                            return Err(ModuleGraphError::Compile {
                                                path,
                                                diagnostics,
                                            });
                                        }
                                        result => {
                                            break result?;
                                        }
                                    }
                                };
                                let ModulePayload::Program(program) = payload else {
                                    return Err(ModuleGraphError::Source {
                                        path: path.clone(),
                                        message: "code module provider returned JSON text"
                                            .to_owned(),
                                    });
                                };
                                (program, None)
                            }
                            ModuleSource::Json => {
                                let ModulePayload::JsonText(text) = (self.provider)(&path, source)?
                                else {
                                    return Err(ModuleGraphError::Source {
                                        path: path.clone(),
                                        message: "JSON module provider returned a program"
                                            .to_owned(),
                                    });
                                };
                                (json_module_program(), Some(text))
                            }
                            ModuleSource::Asset => {
                                let ModulePayload::AssetPath(asset) =
                                    (self.provider)(&path, source)?
                                else {
                                    return Err(ModuleGraphError::Source {
                                        path: path.clone(),
                                        message: "asset provider returned a non-asset payload"
                                            .to_owned(),
                                    });
                                };
                                (
                                    json_module_program(),
                                    Some(asset.to_string_lossy().into_owned()),
                                )
                            }
                            ModuleSource::NodeBuiltin(builtin) => {
                                let source = kiln_builtins::builtin_source(builtin);
                                let path_str = kiln_builtins::builtin_logical_path(builtin);
                                let hir = frontend_oxc::parse(path_str, source);
                                let program = compile_module(&hir).map_err(|diagnostics| {
                                    ModuleGraphError::Compile {
                                        path: path.clone(),
                                        diagnostics,
                                    }
                                })?;
                                (program, None)
                            }
                        };
                        validate_exports(&path, &program)?;
                        let mut dependencies = Vec::with_capacity(program.dependencies.len());
                        let mut runtime_dependencies = Vec::new();
                        let mut dynamic_dependencies = Vec::new();
                        let mut dynamic_sources = Vec::new();
                        let mut runtime_sources = Vec::new();
                        let mut dependency_sources = Vec::with_capacity(program.dependencies.len());
                        let request = if source == ModuleSource::CommonJs {
                            NodeRequestKind::Require
                        } else {
                            NodeRequestKind::Import
                        };
                        for specifier in &program.dependencies {
                            let resolved = match self
                                .resolver
                                .resolve_for_request(specifier, &path, request)
                            {
                                Ok(resolved) => resolved,
                                // CommonJS resolution is synchronous execution, not linking.
                                // Keep unresolved literals out of the captured target map so an
                                // executed `require()` throws where JavaScript can catch it.
                                Err(_) if source == ModuleSource::CommonJs => continue,
                                Err(error) => {
                                    return Err(ModuleGraphError::Resolve {
                                        importer: path.clone(),
                                        specifier: specifier.clone(),
                                        error,
                                    });
                                }
                            };
                            let source = if let Some(builtin) = resolved.builtin() {
                                ModuleSource::NodeBuiltin(builtin)
                            } else if resolved
                                .path
                                .extension()
                                .and_then(|extension| extension.to_str())
                                .is_some_and(is_asset_extension)
                            {
                                ModuleSource::Asset
                            } else if !self.typed_sources {
                                ModuleSource::Source
                            } else if resolved
                                .path
                                .extension()
                                .and_then(|extension| extension.to_str())
                                .is_some_and(kiln_data_formats::is_data_extension)
                            {
                                ModuleSource::Json
                            } else if self.resolver.node_format(&resolved).map_err(|error| {
                                ModuleGraphError::Resolve {
                                    importer: path.clone(),
                                    specifier: specifier.clone(),
                                    error,
                                }
                            })? == Some(NodeModuleFormat::CommonJs)
                            {
                                ModuleSource::CommonJs
                            } else {
                                ModuleSource::Source
                            };
                            dependencies.push(Dependency {
                                specifier: specifier.clone(),
                                target: resolved.path,
                            });
                            dependency_sources.push(source);
                        }
                        for request in &program.link_facts.node_runtime_requests {
                            let importer = match runtime_importer_path(&request.importer) {
                                Ok(importer) => importer,
                                Err(message) => {
                                    self.record_runtime_failure(
                                        path.clone(),
                                        PathBuf::from(&request.importer),
                                        request.specifier.clone(),
                                        format!(
                                            "runtime request failed for {}: {}",
                                            request.specifier, message
                                        ),
                                    );
                                    continue;
                                }
                            };
                            let resolved = match self.resolver.resolve_for_request(
                                &request.specifier,
                                &importer,
                                NodeRequestKind::Require,
                            ) {
                                Ok(resolved) => resolved,
                                Err(error) => {
                                    // `createRequire` and `getBuiltinModule` execute lazily. An
                                    // unavailable literal is therefore a catchable runtime
                                    // failure, just like a dynamic import that cannot be resolved;
                                    // it must not make graph construction fail eagerly.
                                    self.record_runtime_failure(
                                        path.clone(),
                                        importer.clone(),
                                        request.specifier.clone(),
                                        format!(
                                            "runtime request failed to resolve {} from {}: {}",
                                            request.specifier,
                                            importer.display(),
                                            error
                                        ),
                                    );
                                    continue;
                                }
                            };
                            let runtime_source = if let Some(builtin) = resolved.builtin() {
                                ModuleSource::NodeBuiltin(builtin)
                            } else if !self.typed_sources {
                                ModuleSource::Source
                            } else if resolved
                                .path
                                .extension()
                                .and_then(|e| e.to_str())
                                .is_some_and(kiln_data_formats::is_data_extension)
                            {
                                ModuleSource::Json
                            } else {
                                match self.resolver.node_format(&resolved) {
                                    Ok(Some(NodeModuleFormat::CommonJs)) => ModuleSource::CommonJs,
                                    Ok(_) => ModuleSource::Source,
                                    Err(error) => {
                                        self.record_runtime_failure(
                                            path.clone(),
                                            importer.clone(),
                                            request.specifier.clone(),
                                            format!(
                                                "runtime request failed while classifying {}: {}",
                                                request.specifier, error
                                            ),
                                        );
                                        continue;
                                    }
                                }
                            };
                            runtime_dependencies.push(RuntimeDependency {
                                importer,
                                specifier: request.specifier.clone(),
                                target: resolved.path,
                            });
                            runtime_sources.push(runtime_source);
                        }
                        // Closed-world native dynamic imports need their statically literal
                        // targets materialized too. This is especially important for file
                        // assets: their bytes must never pass through the source loader.
                        for target in &program.link_facts.dynamic_imports {
                            let kiln_ir::DynamicImportTarget::Static(specifier) = target else {
                                continue;
                            };
                            let resolved = match self.resolver.resolve_for_request(
                                specifier,
                                &path,
                                NodeRequestKind::Import,
                            ) {
                                Ok(resolved) => resolved,
                                Err(error) => {
                                    self.record_dynamic_failure(
                                        (path.clone(), specifier.clone()),
                                        format!(
                                            "dynamic import failed: could not resolve {} from {}: {}",
                                            specifier,
                                            path.display(),
                                            error
                                        ),
                                    );
                                    continue;
                                }
                            };
                            let target_path = resolved.path.clone();
                            let node_format = if self.typed_sources
                                && resolved.builtin().is_none()
                                && !target_path
                                    .extension()
                                    .and_then(|ext| ext.to_str())
                                    .is_some_and(is_asset_extension)
                                && !target_path
                                    .extension()
                                    .and_then(|ext| ext.to_str())
                                    .is_some_and(kiln_data_formats::is_data_extension)
                            {
                                match self.resolver.node_format(&resolved) {
                                    Ok(format) => format,
                                    Err(error) => {
                                        self.record_dynamic_failure(
                                            (path.clone(), specifier.clone()),
                                            format!(
                                                "dynamic import failed while classifying {}: {}",
                                                specifier, error
                                            ),
                                        );
                                        continue;
                                    }
                                }
                            } else {
                                None
                            };
                            let source = if let Some(builtin) = resolved.builtin() {
                                ModuleSource::NodeBuiltin(builtin)
                            } else if target_path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(is_asset_extension)
                            {
                                ModuleSource::Asset
                            } else if target_path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(kiln_data_formats::is_data_extension)
                            {
                                ModuleSource::Json
                            } else if !self.typed_sources {
                                ModuleSource::Source
                            } else if node_format == Some(NodeModuleFormat::CommonJs) {
                                ModuleSource::CommonJs
                            } else {
                                ModuleSource::Source
                            };
                            dynamic_dependencies.push(Dependency {
                                specifier: specifier.clone(),
                                target: target_path,
                            });
                            for dependency in dynamic_dependencies.drain(..) {
                                self.record_dynamic_dependency(&path, dependency);
                            }
                            dynamic_sources.push(source);
                        }

                        // LIFO work preserves the original source order: the first dependency is
                        // entered first, while Exit remains below all child work.
                        work.push(WorkItem::Exit(Box::new(ModuleNode {
                            path: path.clone(),
                            source,
                            program,
                            data,
                            dependencies: dependencies.clone(),
                            runtime_dependencies: runtime_dependencies.clone(),
                        })));
                        for (dependency, source) in
                            dependencies.into_iter().zip(dependency_sources).rev()
                        {
                            work.push(WorkItem::Enter {
                                path: dependency.target,
                                source: Some(source),
                            });
                        }
                        for (dependency, source) in runtime_dependencies
                            .iter()
                            .cloned()
                            .zip(runtime_sources.iter().cloned())
                        {
                            self.runtime_work.push(RuntimeWork {
                                owner: path.clone(),
                                importer: dependency.importer,
                                specifier: dependency.specifier,
                                path: dependency.target,
                                source,
                            });
                        }
                        if let Some(dynamic) = self.dynamic_dependencies.get(&path).cloned() {
                            for (dependency, source) in dynamic.iter().zip(dynamic_sources).rev() {
                                self.dynamic_work.push(DynamicWork {
                                    importer: path.clone(),
                                    specifier: dependency.specifier.clone(),
                                    path: dependency.target.clone(),
                                    source,
                                });
                            }
                        }
                    }
                },
                WorkItem::Exit(node) => {
                    self.stack.pop();
                    self.states.insert(node.path.clone(), VisitState::Done);
                    self.order.push(node.path.clone());
                    self.record_module(*node);
                }
            }
        }
        Ok(())
    }
}

/// A JSON node has one immutable module binding whose value is materialized directly by the
/// loader. The value itself remains an ordinary mutable JavaScript object, matching Node's
/// cached `require()` result and ESM default export behavior.
fn json_module_program() -> ModuleProgram {
    ModuleProgram {
        program: Program {
            version: BYTECODE_VERSION,
            constants: vec![],
            code: vec![Instruction::Return],
            local_count: 1,
            functions: vec![],
            template_sites: vec![],
            // The JSON text is the node's `data`, not a program the compiler produced.
            source: "".into(),
        },
        dependencies: vec![],
        imports: vec![],
        exports: vec![ExportBinding {
            exported: "default".to_owned(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness {
            body: true,
            functions: vec![],
        },
        link_facts: kiln_ir::ModuleLinkFacts {
            node_runtime_requests: Vec::new(),
            immutable_bindings: vec!["default".to_owned()],
            direct_eval: false,
            new_function: false,
            dynamic_imports: vec![],
            removable_declarations: vec![],
        },
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

fn validate_exports(path: &Path, program: &ModuleProgram) -> Result<(), ModuleGraphError> {
    let mut names = HashSet::new();
    for export in &program.exports {
        if export.exported.starts_with(kiln_ir::STAR_REEXPORT_PREFIX) {
            continue;
        }
        if !names.insert(export.exported.clone()) {
            return Err(ModuleGraphError::DuplicateExport {
                module: path.to_path_buf(),
                name: export.exported.clone(),
            });
        }
    }
    Ok(())
}

fn validate_links(
    modules: &BTreeMap<PathBuf, ModuleNode>,
    order: &[PathBuf],
    export_resolutions: &HashMap<PathBuf, BTreeMap<String, ResolvedExport>>,
) -> Result<(), ModuleGraphError> {
    for importer_path in order {
        let importer =
            modules
                .get(importer_path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule {
                    path: importer_path.clone(),
                })?;
        for import in &importer.program.imports {
            if importer.source == ModuleSource::CommonJs && import.source == COMMONJS_WRAPPER_SOURCE
            {
                continue;
            }
            let dependency = importer
                .dependencies
                .iter()
                .find(|dependency| dependency.specifier == import.source)
                .ok_or_else(|| ModuleGraphError::UnresolvedImport {
                    importer: importer_path.clone(),
                    source: import.source.clone(),
                })?;
            // Looked up for its own sake: a dependency naming a module the graph does not hold
            // is an internal inconsistency, and saying so here is clearer than the missing-export
            // error the check below would otherwise produce.
            if !modules.contains_key(&dependency.target) {
                return Err(ModuleGraphError::InternalMissingModule {
                    path: dependency.target.clone(),
                });
            }
            // A namespace import names no export, so there is nothing to check: it takes
            // whatever the provider has, including nothing.
            if import.imported != kiln_ir::NAMESPACE_IMPORT {
                if let Some(target_node) = modules.get(&dependency.target)
                    && target_node.source == ModuleSource::CommonJs
                {
                    continue;
                }
                match export_resolutions
                    .get(&dependency.target)
                    .and_then(|resolutions| resolutions.get(&import.imported))
                {
                    Some(
                        ResolvedExport::Local { .. }
                        | ResolvedExport::Namespace { .. }
                        | ResolvedExport::CommonJsNamed { .. },
                    ) => {}
                    Some(ResolvedExport::Ambiguous) => {
                        return Err(ModuleGraphError::AmbiguousExport {
                            importer: importer_path.clone(),
                            provider: dependency.target.clone(),
                            name: import.imported.clone(),
                        });
                    }
                    None => {
                        return Err(ModuleGraphError::MissingExport {
                            importer: importer_path.clone(),
                            provider: dependency.target.clone(),
                            name: import.imported.clone(),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

/// Resolve every visible export of every module, the way the specification does.
///
/// A cyclic graph has no dependency-first order to resolve names in: two modules can each
/// re-export a name the other provides, so neither table can be finished before the other is
/// started. The specification does not build tables at all. It answers one question at a time --
/// `ResolveExport(module, name)` -- and carries a *resolve set* of the `(module, name)` pairs
/// already on the current question's path. Meeting a pair twice is the definition of a circular
/// import request, and the answer is "no such export", which is what turns a self-resolving
/// indirect export into the `SyntaxError` the specification asks for.
///
/// `GetExportedNames` is the same shape for the other half: the set of names a module offers,
/// with a set of already-visited modules cutting `export *` cycles.
///
/// This is written recursively, as the specification is. Both walks terminate because each cuts
/// on a set that only grows, and the depth either can reach is bounded by the number of distinct
/// `(module, name)` pairs in the graph.
struct ExportResolver<'graph> {
    modules: &'graph BTreeMap<PathBuf, ModuleNode>,
    commonjs_imported_names: HashMap<PathBuf, HashSet<String>>,
}

/// What one `ResolveExport` question answers.
///
/// `Circular` is kept apart from `None`: both mean the importer has no binding to bind, but only
/// the first has to stop a `export *` search from treating the absence as a real answer.
#[derive(Debug)]
enum Resolution {
    Found(ResolvedExport),
    Ambiguous,
    None,
}

impl<'graph> ExportResolver<'graph> {
    fn new(modules: &'graph BTreeMap<PathBuf, ModuleNode>) -> Self {
        // Collect direct star edges: path -> [target_path]
        let mut star_direct: HashMap<&Path, Vec<&Path>> = HashMap::new();
        for (path, node) in modules {
            let stars: Vec<&Path> = node
                .program
                .exports
                .iter()
                .filter_map(|export| export.exported.strip_prefix(kiln_ir::STAR_REEXPORT_PREFIX))
                .filter_map(|specifier| {
                    node.dependencies
                        .iter()
                        .find(|dep| dep.specifier == specifier)
                        .map(|dep| dep.target.as_path())
                })
                .collect();
            if !stars.is_empty() {
                star_direct.insert(path.as_path(), stars);
            }
        }

        // Transitive closure of star edges
        let mut star_reachable: HashMap<&Path, HashSet<&Path>> = HashMap::new();
        for &start in star_direct.keys() {
            let mut reachable = HashSet::new();
            let mut queue = Vec::new();
            if let Some(targets) = star_direct.get(start) {
                for &t in targets {
                    if reachable.insert(t) {
                        queue.push(t);
                    }
                }
            }
            while let Some(current) = queue.pop() {
                if let Some(targets) = star_direct.get(current) {
                    for &t in targets {
                        if reachable.insert(t) {
                            queue.push(t);
                        }
                    }
                }
            }
            star_reachable.insert(start, reachable);
        }

        // Collect all named exports requested from CommonJS modules by any importer in the graph.
        let mut commonjs_imported_names: HashMap<PathBuf, HashSet<String>> = HashMap::new();
        for other in modules.values() {
            for import in &other.program.imports {
                if import.imported == kiln_ir::NAMESPACE_IMPORT {
                    continue;
                }
                let Some(dep) = other
                    .dependencies
                    .iter()
                    .find(|d| d.specifier == import.source)
                else {
                    continue;
                };
                if let Some(target_node) = modules.get(&dep.target) {
                    if target_node.source == ModuleSource::CommonJs {
                        commonjs_imported_names
                            .entry(dep.target.clone())
                            .or_default()
                            .insert(import.imported.clone());
                    }
                }
                if let Some(reachable) = star_reachable.get(dep.target.as_path()) {
                    for &cjs_path in reachable {
                        if let Some(target_node) = modules.get(cjs_path) {
                            if target_node.source == ModuleSource::CommonJs {
                                commonjs_imported_names
                                    .entry(cjs_path.to_path_buf())
                                    .or_default()
                                    .insert(import.imported.clone());
                            }
                        }
                    }
                }
            }
        }

        Self {
            modules,
            commonjs_imported_names,
        }
    }

    /// The dependency a module reaches through one written specifier.
    fn dependency(&self, node: &ModuleNode, specifier: &str) -> Option<&'graph ModuleNode> {
        let target = &node
            .dependencies
            .iter()
            .find(|dependency| dependency.specifier == specifier)?
            .target;
        self.modules.get(target)
    }

    /// The import entry that makes `slot` an *indirect* export rather than a local one.
    ///
    /// `export { a } from './x.js'` is compiled as an import into a slot no source can name plus
    /// an export of that slot, so an indirect export is recognised by its slot being an import's.
    fn indirect<'node>(
        &self,
        node: &'node ModuleNode,
        slot: u32,
    ) -> Option<&'node kiln_ir::ImportBinding> {
        node.program
            .imports
            .iter()
            .filter(|import| import.source != COMMONJS_WRAPPER_SOURCE)
            .find(|import| import.local_slot == slot)
    }

    /// Every `export * from` specifier this module writes.
    fn star_sources<'node>(&self, node: &'node ModuleNode) -> impl Iterator<Item = &'node str> {
        node.program
            .exports
            .iter()
            .filter_map(|export| export.exported.strip_prefix(kiln_ir::STAR_REEXPORT_PREFIX))
    }

    /// The names a module offers, `export *` included. `default` never arrives through a star.
    fn exported_names(&self, path: &Path, visited: &mut HashSet<PathBuf>) -> Vec<String> {
        if !visited.insert(path.to_path_buf()) {
            return Vec::new();
        }
        let Some(node) = self.modules.get(path) else {
            return Vec::new();
        };
        let mut names = Vec::new();
        // Node's ESM bridge exposes the final `module.exports` value as the default export. The
        // value is materialized by CommonJS execution; the linker only needs its typed shape.
        // It also provides any named export requested from it by an importer in the graph.
        if node.source == ModuleSource::CommonJs {
            names.push("default".to_owned());
            if let Some(imported) = self.commonjs_imported_names.get(path) {
                for name in imported {
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
            }
        }
        for export in &node.program.exports {
            if !export.exported.starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
                && !names.contains(&export.exported)
            {
                names.push(export.exported.clone());
            }
        }
        for specifier in self.star_sources(node).collect::<Vec<_>>() {
            let Some(provider) = self.dependency(node, specifier) else {
                continue;
            };
            for name in self.exported_names(&provider.path, visited) {
                if name != "default" && !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// Where one name exported by one module ultimately comes from.
    fn resolve(
        &self,
        path: &Path,
        name: &str,
        resolve_set: &mut HashSet<(PathBuf, String)>,
    ) -> Resolution {
        // The circular import request. The specification returns null here, and the caller that
        // asked on behalf of an `import` or an indirect export turns that into a SyntaxError.
        if !resolve_set.insert((path.to_path_buf(), name.to_owned())) {
            return Resolution::None;
        }
        let Some(node) = self.modules.get(path) else {
            return Resolution::None;
        };
        if node.source == ModuleSource::CommonJs {
            if name == "default" {
                return Resolution::Found(ResolvedExport::Local {
                    module: path.to_path_buf(),
                    local_slot: 0,
                });
            }
            return Resolution::Found(ResolvedExport::CommonJsNamed {
                module: path.to_path_buf(),
                property: name.to_owned(),
            });
        }
        if let Some(export) = node
            .program
            .exports
            .iter()
            .filter(|export| !export.exported.starts_with(kiln_ir::STAR_REEXPORT_PREFIX))
            .find(|export| export.exported == name)
        {
            let Some(import) = self.indirect(node, export.local_slot) else {
                // A binding this module declares itself. This is where every chain ends.
                return Resolution::Found(ResolvedExport::Local {
                    module: path.to_path_buf(),
                    local_slot: export.local_slot,
                });
            };
            let Some(provider) = self.dependency(node, &import.source) else {
                return Resolution::None;
            };
            if import.imported == kiln_ir::NAMESPACE_IMPORT {
                return Resolution::Found(ResolvedExport::Namespace {
                    module: provider.path.clone(),
                });
            }
            return self.resolve(&provider.path, &import.imported, resolve_set);
        }
        // `export *` never forwards a default: a module's default is its own.
        if name == "default" {
            return Resolution::None;
        }
        let mut star = None;
        for specifier in self.star_sources(node).collect::<Vec<_>>() {
            let Some(provider) = self.dependency(node, specifier) else {
                continue;
            };
            match self.resolve(&provider.path, name, &mut resolve_set.clone()) {
                Resolution::Ambiguous => return Resolution::Ambiguous,
                Resolution::None => {}
                Resolution::Found(found) => match &star {
                    None => star = Some(found),
                    // Two stars offering the same name from different origins is the one case
                    // the specification calls ambiguous: neither wins, and the name is visible
                    // only in the sense that asking for it is an error rather than a miss.
                    Some(existing) if *existing != found => return Resolution::Ambiguous,
                    Some(_) => {}
                },
            }
        }
        match star {
            Some(found) => Resolution::Found(found),
            None => Resolution::None,
        }
    }
}

/// Resolve every exported name of every module in the graph.
///
/// The result is the same shape a bottom-up pass produced when cycles were rejected -- a table
/// per module, keyed by visible name -- so nothing downstream has to know that the walk beneath
/// it changed. A name with no resolution is absent, which is how `validate_links` tells a missing
/// export from an ambiguous one.
fn resolve_exports(
    modules: &BTreeMap<PathBuf, ModuleNode>,
    order: &[PathBuf],
) -> Result<HashMap<PathBuf, BTreeMap<String, ResolvedExport>>, ModuleGraphError> {
    let resolver = ExportResolver::new(modules);
    let mut tables: HashMap<PathBuf, BTreeMap<String, ResolvedExport>> = HashMap::new();
    for module_path in order {
        if !modules.contains_key(module_path) {
            return Err(ModuleGraphError::InternalMissingModule {
                path: module_path.clone(),
            });
        }
        let mut table = BTreeMap::new();
        for name in resolver.exported_names(module_path, &mut HashSet::new()) {
            match resolver.resolve(module_path, &name, &mut HashSet::new()) {
                Resolution::Found(found) => {
                    table.insert(name, found);
                }
                // Retained rather than dropped so namespace construction can omit it and an
                // importer can report the difference from a name that is simply not there.
                Resolution::Ambiguous => {
                    table.insert(name, ResolvedExport::Ambiguous);
                }
                Resolution::None => {}
            }
        }
        tables.insert(module_path.clone(), table);
    }
    Ok(tables)
}

#[cfg(test)]
mod tests;
