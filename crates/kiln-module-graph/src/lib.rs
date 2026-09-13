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
    ExecutionLimits, ExecutionResult, ModuleBinding, ModulePlan, ModuleRealmSession,
    ModuleSelfExport, ModuleSelfImport, ModuleValueRef, PlannedModule, Value,
    execute_module_in_runtime_with_loader, execute_node_module_in_session_with_loader,
    node_builtin_commonjs_value, node_builtin_export_names, node_builtin_exports, ordinary_object,
    own_data_property, parse_json_module, read_through_cell,
};
use kiln_ir::{
    BYTECODE_VERSION, CompileDiagnostic, ExportBinding, Instruction, ModuleProgram, Program,
    compile_commonjs, compile_module,
};
use kiln_module_resolver::{
    ModuleResolver, NodeBuiltin, NodeModuleFormat, NodeRequestKind, PathRole, ResolutionKind,
    ResolutionPolicy, ResolvedModule, SUPPORTED_EXTENSIONS, is_asset_extension,
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
                || path.extension().and_then(|value| value.to_str()).is_some_and(is_asset_extension)
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
                        let target_in_graph =
                            node.dependencies.iter().any(|d| {
                                d.specifier == *specifier && self.module(&d.target).is_some()
                            }) || self.module(specifier).is_some()
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
        pending: &mut HashMap<(PathBuf, String), Cell>,
    ) -> Result<Cell, ModuleGraphError> {
        if import.imported == kiln_ir::NAMESPACE_IMPORT {
            return Err(ModuleGraphError::Source {
                path: importer.to_path_buf(),
                message: format!(
                    "`import * as` of `{}`, which imports it back before it has run, needs a \
                     live namespace this linker cannot yet build",
                    provider.display()
                ),
            });
        }
        let (origin, local_slot) = match self.resolved_export(provider, &import.imported) {
            Some(ResolvedExport::Local { module, local_slot }) => (module.clone(), *local_slot),
            Some(ResolvedExport::CommonJsNamed { module, .. }) => (module.clone(), 0),
            Some(ResolvedExport::Namespace { module }) => {
                return Err(ModuleGraphError::Source {
                    path: importer.to_path_buf(),
                    message: format!(
                        "`{}` re-exports the namespace of `{}` around a cycle, which needs a \
                         live namespace this linker cannot yet build",
                        provider.display(),
                        module.display()
                    ),
                });
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

        let position: HashMap<&Path, usize> = self
            .order
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

        let mut modules = Vec::with_capacity(self.order.len());
        for path in self.order.iter() {
            let node = self
                .module_node(path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
            // Only a source module has a body to run. A JSON module, a CommonJS wrapper and a
            // Node builtin each need their own materialisation, and answering with an empty
            // namespace for one would be a wrong answer rather than a missing feature.
            if node.source != ModuleSource::Source {
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
            modules.push(PlannedModule {
                key: path.to_string_lossy().into_owned(),
                program: std::rc::Rc::new(export_epilogue(&node.program.program, slots)),
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
        // A builtin has one canonical implementation and two language projections. `values`
        // holds its ESM bindings; this table holds Node's historical CommonJS `module.exports`
        // value so a later `require()` never has to reverse-engineer one projection from the
        // other.
        let builtin_commonjs: HashMap<PathBuf, Value> = self
            .modules
            .iter()
            .filter_map(|(path, node)| match node.source {
                ModuleSource::NodeBuiltin(builtin) if !kiln_builtins::is_ts_builtin(builtin) => {
                    Some((
                        path.clone(),
                        node_builtin_commonjs_value(builtin.specifier())
                            .expect("a linkable builtin has a CommonJS projection"),
                    ))
                }
                _ => None,
            })
            .collect();
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
        let mut node_session = node.then(ModuleRealmSession::default);
        let cache_targets = commonjs_cache
            .iter()
            .map(|(path, cell)| (std::rc::Rc::as_ptr(cell), path.clone()))
            .collect();
        let mut commonjs_states = HashMap::new();
        let mut commonjs_loader = CommonJsLoader {
            graph: self,
            builtin_commonjs: &builtin_commonjs,
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
        };

        for path in self.order.iter() {
            let module_node = self
                .module_node(path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
            if let ModuleSource::NodeBuiltin(builtin) = module_node.source
                && !kiln_builtins::is_ts_builtin(builtin)
            {
                let exports = node_builtin_exports(builtin.specifier())
                    .expect("only materializable builtins have linkable exports");
                values.insert(path.clone(), exports.into_iter().collect());
                continue;
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
                let value =
                    parse_json_module(source).map_err(|error| ModuleGraphError::Execution {
                        module: path.clone(),
                        error,
                    })?;
                values.insert(path.clone(), HashMap::from([("default".to_owned(), value)]));
                continue;
            }
            if module_node.source == ModuleSource::Asset {
                let path = module_node.data.as_deref().ok_or_else(|| ModuleGraphError::Source {
                    path: path.clone(),
                    message: "asset module has no path metadata".to_owned(),
                })?;
                values.insert(
                    module_node.path.clone(),
                    HashMap::from([("default".to_owned(), Value::String(path.to_owned()))]),
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
                        &mut pending_cycle_cells,
                    )?;
                    imports.push(Value::Cell(cell));
                    continue;
                };
                // `import * as ns` wants every export at once rather than one by name. The
                // specification orders a namespace object's keys, so this sorts them: a hash map
                // would otherwise make `Object.keys(ns)` differ run to run.
                if import.imported == kiln_ir::NAMESPACE_IMPORT {
                    let mut entries: Vec<(String, Value)> = provider
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect();
                    entries.sort_by(|left, right| left.0.cmp(&right.0));
                    imports.push(kiln_interpreter::namespace_object(entries));
                    continue;
                }
                let value = provider
                    .get(&import.imported)
                    .cloned()
                    .or_else(|| {
                        provider.get("default").and_then(|default_val| {
                            own_data_property(default_val, &import.imported)
                        })
                    })
                    .ok_or_else(|| ModuleGraphError::MissingExport {
                        importer: path.clone(),
                        provider: dependency.target.clone(),
                        name: import.imported.clone(),
                    })?;
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
            let result = if node {
                execute_node_module_in_session_with_loader(
                    &module_node.program,
                    &imports,
                    &self_imports,
                    host,
                    remaining_limits,
                    &mut runtime,
                    node_session.as_mut().expect("Node mode owns a session"),
                    capabilities.clone(),
                    path == &self.entry,
                    &mut commonjs_loader,
                    true,
                )
            } else {
                execute_module_in_runtime_with_loader(
                    &module_node.program,
                    &imports,
                    &self_imports,
                    host,
                    remaining_limits,
                    &mut runtime,
                    capabilities.clone(),
                    &mut commonjs_loader,
                    true,
                )
            }
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
            // Whatever this module was holding a cycle's place for is now real. The value is
            // read through its cell, because what is published is the binding and what a waiting
            // importer's cell has to contain is the value in it.
            for (name, value) in &exports {
                if let Some(cell) = pending_cycle_cells.remove(&(path.clone(), name.clone())) {
                    *cell.borrow_mut() = read_through_cell(value);
                }
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
    builtin_commonjs: &'graph HashMap<PathBuf, Value>,
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
}

impl<H: HostOps + ?Sized> CommonJsLoader<'_, H> {
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
            let value = if let Some(cell) = self.commonjs_cache.get(&dependency.target) {
                std::rc::Rc::clone(cell)
            } else if let Some(provider) = unsafe { (&*self.values).get(&dependency.target) } {
                if let Some(value) = self.builtin_commonjs.get(&dependency.target) {
                    std::rc::Rc::new(std::cell::RefCell::new(value.clone()))
                } else if self
                    .graph
                    .module_node(&dependency.target)
                    .is_some_and(|node| match node.source {
                        ModuleSource::Json => true,
                        ModuleSource::NodeBuiltin(builtin) => kiln_builtins::is_ts_builtin(builtin),
                        _ => false,
                    })
                {
                    let value =
                        provider
                            .get("default")
                            .cloned()
                            .ok_or_else(|| ExecutionError::Host {
                                instruction: 0,
                                message: format!(
                                    "Target `{}` has no default",
                                    dependency.target.display()
                                ),
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
            Value::String(path.to_string_lossy().into_owned()),
            Value::String(
                path.parent()
                    .unwrap_or_else(|| Path::new(""))
                    .to_string_lossy()
                    .into_owned(),
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
                        && let Some(val) = object_ref.get_own(key)
                    {
                        exports_map.insert(key.to_owned(), val.clone());
                    }
                }
            } else if let Value::Function(closure) = &final_exports {
                let properties_ref = closure.properties.borrow();
                for key in properties_ref.keys() {
                    if key != "default"
                        && let Some(val) = properties_ref.get_own(key)
                    {
                        exports_map.insert(key.to_owned(), val.clone());
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
    fn load(
        &mut self,
        require: &CommonJsRequire,
        specifier: &str,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Some(cell) = require.target_cell(specifier) else {
            return Err(ExecutionError::Host {
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
                message: format!("CommonJS require target `{specifier}` is not in the graph"),
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
}

enum WorkItem {
    Enter {
        path: PathBuf,
        source: ModuleSource,
    },
    /// A module whose own compilation is finished, waiting below its dependencies' work.
    ///
    /// The payload is exactly the [`ModuleNode`] this will become, so it is spelled as one --
    /// and boxed, because it is an order of magnitude larger than `Enter` and every item in the
    /// queue would otherwise be sized for it.
    Exit(Box<ModuleNode>),
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
        let mut work = vec![WorkItem::Enter {
            path,
            source: ModuleSource::Source,
        }];
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
                            ModuleSource::Source if path.extension()
                                .and_then(|extension| extension.to_str())
                                .is_some_and(is_asset_extension) => ModuleSource::Asset,
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
                                        Some(NodeModuleFormat::CommonJs) => ModuleSource::CommonJs,
                                        _ => ModuleSource::Source,
                                    }
                                }
                            }
                            source => source,
                        };
                        self.states.insert(path.clone(), VisitState::Visiting);
                        self.stack.push(path.clone());
                        let (program, data) = match source {
                            ModuleSource::Source | ModuleSource::CommonJs => {
                                let ModulePayload::Program(program) =
                                    (self.provider)(&path, source)?
                                else {
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
                                let ModulePayload::AssetPath(asset) = (self.provider)(&path, source)?
                                else {
                                    return Err(ModuleGraphError::Source {
                                        path: path.clone(),
                                        message: "asset provider returned a non-asset payload".to_owned(),
                                    });
                                };
                                (json_module_program(), Some(asset.to_string_lossy().into_owned()))
                            }
                            ModuleSource::NodeBuiltin(builtin) => {
                                if let Some(source) = kiln_builtins::builtin_source(builtin) {
                                    let path_str = kiln_builtins::builtin_logical_path(builtin);
                                    let hir = frontend_oxc::parse(path_str, source);
                                    let program = compile_module(&hir).map_err(|diagnostics| {
                                        ModuleGraphError::Compile {
                                            path: path.clone(),
                                            diagnostics,
                                        }
                                    })?;
                                    (program, None)
                                } else {
                                    (builtin_module_program(builtin), None)
                                }
                            }
                        };
                        validate_exports(&path, &program)?;
                        let mut dependencies = Vec::with_capacity(program.dependencies.len());
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
                            } else if resolved.path.extension()
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

                        // LIFO work preserves the original source order: the first dependency is
                        // entered first, while Exit remains below all child work.
                        work.push(WorkItem::Exit(Box::new(ModuleNode {
                            path,
                            source,
                            program,
                            data,
                            dependencies: dependencies.clone(),
                        })));
                        for (dependency, source) in
                            dependencies.into_iter().zip(dependency_sources).rev()
                        {
                            work.push(WorkItem::Enter {
                                path: dependency.target,
                                source,
                            });
                        }
                    }
                },
                WorkItem::Exit(node) => {
                    self.stack.pop();
                    self.states.insert(node.path.clone(), VisitState::Done);
                    self.order.push(node.path.clone());
                    self.modules.insert(node.path.clone(), *node);
                }
            }
        }
        Ok(())
    }
}

/// A builtin participates in ordinary graph linking but has no source body to parse. Its empty
/// program carries only the export shape needed by the linker; execution materializes those
/// exports directly from the interpreter's canonical builtin table without entering the VM.
fn builtin_module_program(builtin: NodeBuiltin) -> ModuleProgram {
    // Resolution can recognize future surface before execution advertises it. An empty export
    // table means importing a name fails at link time rather than producing a placeholder.
    let names = node_builtin_export_names(builtin.specifier()).unwrap_or(&[]);
    ModuleProgram {
        program: Program {
            version: BYTECODE_VERSION,
            constants: vec![],
            code: vec![Instruction::Return],
            local_count: names.len() as u32,
            functions: vec![],
            template_sites: vec![],
            // A builtin has no source body at all, which is why it has no functions either.
            source: "".into(),
        },
        dependencies: vec![],
        imports: vec![],
        exports: names
            .iter()
            .enumerate()
            .map(|(slot, name)| ExportBinding {
                exported: (*name).to_owned(),
                local_slot: slot as u32,
            })
            .collect(),
        // A builtin is a module, and a module is strict. The body is one `Return`, so this
        // decides nothing that runs; saying it anyway keeps the answer the same whether the
        // node came from the compiler or from here.
        strictness: kiln_ir::Strictness {
            body: true,
            functions: vec![],
        },
        link_facts: kiln_ir::ModuleLinkFacts {
            immutable_bindings: names.iter().map(|s| (*s).to_owned()).collect(),
            direct_eval: false,
            new_function: false,
            dynamic_imports: vec![],
            removable_declarations: vec![],
        },
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
            immutable_bindings: vec!["default".to_owned()],
            direct_eval: false,
            new_function: false,
            dynamic_imports: vec![],
            removable_declarations: vec![],
        },
    }
}

fn validate_exports(path: &Path, program: &ModuleProgram) -> Result<(), ModuleGraphError> {
    let mut names = HashSet::new();
    for export in &program.exports {
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
}

/// What one `ResolveExport` question answers.
///
/// `Circular` is kept apart from `None`: both mean the importer has no binding to bind, but only
/// the first has to stop a `export *` search from treating the absence as a real answer.
enum Resolution {
    Found(ResolvedExport),
    Ambiguous,
    None,
}

impl<'graph> ExportResolver<'graph> {
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

    fn reexports_star(&self, from: &Path, target: &Path, visited: &mut HashSet<PathBuf>) -> bool {
        if !visited.insert(from.to_path_buf()) {
            return false;
        }
        let Some(node) = self.modules.get(from) else {
            return false;
        };
        for specifier in self.star_sources(node) {
            if let Some(dep) = self.dependency(node, specifier)
                && (dep.path == target || self.reexports_star(&dep.path, target, visited))
            {
                return true;
            }
        }
        false
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
            for other in self.modules.values() {
                for import in &other.program.imports {
                    if import.imported != kiln_ir::NAMESPACE_IMPORT
                        && !names.contains(&import.imported)
                        && other.dependencies.iter().any(|dep| {
                            dep.specifier == import.source
                                && (dep.target == *path
                                    || self.reexports_star(&dep.target, path, &mut HashSet::new()))
                        })
                    {
                        names.push(import.imported.clone());
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
    let resolver = ExportResolver { modules };
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
