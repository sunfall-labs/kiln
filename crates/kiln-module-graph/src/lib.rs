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

use kiln_hir::Module;
use kiln_host_abi::HostOps;
use kiln_interpreter::{
    Cell, CommonJsModuleLoader, CommonJsRequire, ExecutionError, ExecutionLimits, ExecutionResult,
    ModuleBinding, ModulePlan, ModuleRealmSession, ModuleSelfExport, ModuleSelfImport,
    ModuleValueRef, PlannedModule, Value, execute_module_in_runtime_with_loader,
    execute_node_module_in_session_with_loader, node_builtin_commonjs_value,
    node_builtin_export_names, node_builtin_exports, ordinary_object, own_data_property,
    parse_json_module, read_through_cell,
};
use kiln_ir::{
    BYTECODE_VERSION, CompileDiagnostic, ExportBinding, Instruction, ModuleProgram, Program,
    compile_commonjs, compile_module,
};
use kiln_module_resolver::{
    ModuleResolver, NodeBuiltin, NodeModuleFormat, NodeRequestKind, PathRole, ResolutionKind,
    ResolutionPolicy, ResolvedModule, SUPPORTED_EXTENSIONS,
};
use kiln_runtime::RealmRuntime;

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
    NodeBuiltin(NodeBuiltin),
}

/// The typed result supplied by a module provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModulePayload {
    Program(ModuleProgram),
    JsonText(String),
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
    Ambiguous,
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
    Cycle {
        /// The cycle is shown closed: the first path is repeated at the end.
        paths: Vec<PathBuf>,
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
            Self::Cycle { paths } => {
                f.write_str("module dependency cycle: ")?;
                for (index, path) in paths.iter().enumerate() {
                    if index != 0 {
                        f.write_str(" -> ")?;
                    }
                    write!(f, "{}", path.display())?;
                }
                Ok(())
            }
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
                return Ok(ModulePayload::JsonText(source));
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
            stack_sources: Vec::new(),
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
            ResolvedExport::Ambiguous => None,
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
                    ResolvedExport::Local { .. } | ResolvedExport::Namespace { .. } => {
                        Some(name.clone())
                    }
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
    ) -> Result<ExecutionResult, ModuleGraphError> {
        self.execute_mode(host, limits, false)
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
                        Some(ResolvedExport::Ambiguous) | None => None,
                    }
                };
                let binding = binding.ok_or_else(|| ModuleGraphError::MissingExport {
                    importer: path.clone(),
                    provider: dependency.target.clone(),
                    name: import.imported.clone(),
                })?;
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
    ) -> Result<ExecutionResult, ModuleGraphError> {
        self.execute_mode(host, limits, true)
    }

    fn execute_mode<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
        node: bool,
    ) -> Result<ExecutionResult, ModuleGraphError> {
        let mut values: HashMap<PathBuf, HashMap<String, Value>> = HashMap::new();
        // A builtin has one canonical implementation and two language projections. `values`
        // holds its ESM bindings; this table holds Node's historical CommonJS `module.exports`
        // value so a later `require()` never has to reverse-engineer one projection from the
        // other.
        let builtin_commonjs: HashMap<PathBuf, Value> = self
            .modules
            .iter()
            .filter_map(|(path, node)| match node.source {
                ModuleSource::NodeBuiltin(builtin) => Some((
                    path.clone(),
                    node_builtin_commonjs_value(builtin.specifier())
                        .expect("a linkable builtin has a CommonJS projection"),
                )),
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
        let mut runtime = RealmRuntime::new().map_err(|error| ModuleGraphError::Execution {
            module: self.entry.clone(),
            error: ExecutionError::Host {
                instruction: 0,
                message: error.to_string(),
            },
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
            node,
        };

        for path in self.order.iter() {
            let module_node = self
                .module_node(path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
            if let ModuleSource::NodeBuiltin(builtin) = module_node.source {
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
                let provider = values.get(&dependency.target).ok_or_else(|| {
                    ModuleGraphError::InternalMissingModule {
                        path: dependency.target.clone(),
                    }
                })?;
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
                let value = provider.get(&import.imported).ok_or_else(|| {
                    ModuleGraphError::MissingExport {
                        importer: path.clone(),
                        provider: dependency.target.clone(),
                        name: import.imported.clone(),
                    }
                })?;
                imports.push(value.clone());
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
                            Some(ResolvedExport::Local { .. } | ResolvedExport::Namespace { .. })
                        )
                    {
                        exports.entry(name).or_insert(value);
                    }
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
            let value =
                if let Some(cell) = self.commonjs_cache.get(&dependency.target) {
                    std::rc::Rc::clone(cell)
                } else if let Some(provider) = unsafe { (&*self.values).get(&dependency.target) } {
                    if let Some(value) = self.builtin_commonjs.get(&dependency.target) {
                        std::rc::Rc::new(std::cell::RefCell::new(value.clone()))
                    } else if self
                        .graph
                        .module_node(&dependency.target)
                        .is_some_and(|node| node.source == ModuleSource::Json)
                    {
                        let value = provider.get("default").cloned().ok_or_else(|| {
                            ExecutionError::Host {
                                instruction: 0,
                                message: format!(
                                    "JSON target `{}` has no default",
                                    dependency.target.display()
                                ),
                            }
                        })?;
                        std::rc::Rc::new(std::cell::RefCell::new(value))
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
            (&mut *self.values).insert(
                path.to_path_buf(),
                HashMap::from([("default".to_owned(), final_exports)]),
            );
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
    stack_sources: Vec<ModuleSource>,
    order: Vec<PathBuf>,
}

enum WorkItem {
    Enter {
        path: PathBuf,
        source: ModuleSource,
    },
    Exit {
        path: PathBuf,
        source: ModuleSource,
        program: ModuleProgram,
        data: Option<String>,
        dependencies: Vec<Dependency>,
    },
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
                    Some(VisitState::Visiting) => {
                        let start = self
                            .stack
                            .iter()
                            .position(|item| item == &path)
                            .unwrap_or(0);
                        // CommonJS requires observe a cache entry even while that module is
                        // executing. Keep the edge in the already-built parent's dependency
                        // list, but do not descend into it again. Any ESM node in the cycle keeps
                        // the established cycle error rather than getting CommonJS semantics.
                        if self.stack_sources[start..]
                            .iter()
                            .all(|source| *source == ModuleSource::CommonJs)
                        {
                            continue;
                        }
                        // A module that names itself -- `export { x as y } from "./this.js"` is
                        // how a module gives one of its own bindings a second name -- is a cycle
                        // only in the graph's bookkeeping. There is no evaluation order to get
                        // wrong: one module, evaluated once, and every name it re-exports from
                        // itself resolves to a slot it already owns. The edge stays recorded on
                        // the parent; only the descent stops.
                        if self.stack.last() == Some(&path) {
                            continue;
                        }
                        let mut cycle = self.stack[start..].to_vec();
                        cycle.push(path);
                        return Err(ModuleGraphError::Cycle { paths: cycle });
                    }
                    None => {
                        let source = match source {
                            ModuleSource::Source if !self.typed_sources => ModuleSource::Source,
                            ModuleSource::Source => {
                                if path.extension().and_then(|extension| extension.to_str())
                                    == Some("json")
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
                        self.stack_sources.push(source);
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
                            ModuleSource::NodeBuiltin(builtin) => {
                                (builtin_module_program(builtin), None)
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
                            } else if !self.typed_sources {
                                ModuleSource::Source
                            } else if resolved
                                .path
                                .extension()
                                .and_then(|extension| extension.to_str())
                                == Some("json")
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
                        work.push(WorkItem::Exit {
                            path,
                            source,
                            program,
                            data,
                            dependencies: dependencies.clone(),
                        });
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
                WorkItem::Exit {
                    path,
                    source,
                    program,
                    data,
                    dependencies,
                } => {
                    self.stack.pop();
                    self.stack_sources.pop();
                    self.states.insert(path.clone(), VisitState::Done);
                    self.order.push(path.clone());
                    self.modules.insert(
                        path.clone(),
                        ModuleNode {
                            path,
                            source,
                            program,
                            data,
                            dependencies,
                        },
                    );
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
        },
        dependencies: vec![],
        imports: vec![],
        exports: vec![ExportBinding {
            exported: "default".to_owned(),
            local_slot: 0,
        }],
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
                match export_resolutions
                    .get(&dependency.target)
                    .and_then(|resolutions| resolutions.get(&import.imported))
                {
                    Some(ResolvedExport::Local { .. } | ResolvedExport::Namespace { .. }) => {}
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

/// Resolve every exported name in dependency-first order.
///
/// ESM cycles are rejected by the graph builder, so every star provider has a completed table by
/// the time its importer is visited. CommonJS cycles expose only their synthetic default and do
/// not participate in star expansion. This avoids both recursive stack growth and an arbitrary
/// maximum star depth.
fn resolve_exports(
    modules: &BTreeMap<PathBuf, ModuleNode>,
    order: &[PathBuf],
) -> Result<HashMap<PathBuf, BTreeMap<String, ResolvedExport>>, ModuleGraphError> {
    let mut tables: HashMap<PathBuf, BTreeMap<String, ResolvedExport>> = HashMap::new();
    for module_path in order {
        let node =
            modules
                .get(module_path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule {
                    path: module_path.clone(),
                })?;
        let mut table = BTreeMap::new();
        let mut explicit_names = HashSet::new();

        // Node's ESM bridge exposes the final `module.exports` value as the default export. The
        // value is materialized by CommonJS execution; the linker only needs its typed shape.
        if node.source == ModuleSource::CommonJs {
            table.insert(
                "default".to_owned(),
                ResolvedExport::Local {
                    module: module_path.clone(),
                    local_slot: 0,
                },
            );
            explicit_names.insert("default".to_owned());
        }

        for export in node
            .program
            .exports
            .iter()
            .filter(|export| !export.exported.starts_with(kiln_ir::STAR_REEXPORT_PREFIX))
        {
            explicit_names.insert(export.exported.clone());
            let resolution = if let Some(import) = node
                .program
                .imports
                .iter()
                .filter(|import| import.source != COMMONJS_WRAPPER_SOURCE)
                .find(|import| import.local_slot == export.local_slot)
            {
                let dependency = node
                    .dependencies
                    .iter()
                    .find(|dependency| dependency.specifier == import.source)
                    .ok_or_else(|| ModuleGraphError::UnresolvedImport {
                        importer: module_path.clone(),
                        source: import.source.clone(),
                    })?;
                if import.imported == kiln_ir::NAMESPACE_IMPORT {
                    ResolvedExport::Namespace {
                        module: dependency.target.clone(),
                    }
                } else {
                    // A module re-exporting from itself has no finished table yet: it is the
                    // one being built. The names it declared before this line are already in
                    // it, which is exactly the set a self re-export can name.
                    let provider = if dependency.target == *module_path {
                        Some(&table)
                    } else {
                        tables.get(&dependency.target)
                    };
                    match provider.and_then(|provider| provider.get(&import.imported)) {
                        Some(resolution) => resolution.clone(),
                        None => continue,
                    }
                }
            } else {
                ResolvedExport::Local {
                    module: module_path.clone(),
                    local_slot: export.local_slot,
                }
            };
            table.insert(export.exported.clone(), resolution);
        }

        for source in node
            .program
            .exports
            .iter()
            .filter_map(|export| export.exported.strip_prefix(kiln_ir::STAR_REEXPORT_PREFIX))
        {
            let dependency = node
                .dependencies
                .iter()
                .find(|dependency| dependency.specifier == source)
                .ok_or_else(|| ModuleGraphError::UnresolvedImport {
                    importer: module_path.clone(),
                    source: source.to_owned(),
                })?;
            let provider = tables.get(&dependency.target).ok_or_else(|| {
                ModuleGraphError::InternalMissingModule {
                    path: dependency.target.clone(),
                }
            })?;
            for (name, resolution) in provider {
                if name == "default" || explicit_names.contains(name) {
                    continue;
                }
                match (table.get(name), resolution) {
                    (None, resolution) => {
                        table.insert(name.clone(), resolution.clone());
                    }
                    (Some(existing), candidate) if existing != candidate => {
                        table.insert(name.clone(), ResolvedExport::Ambiguous);
                    }
                    (Some(_), _) => {}
                }
            }
        }
        tables.insert(module_path.clone(), table);
    }
    Ok(tables)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_host_abi::{HostError, HostOp, HostValue};
    use kiln_ir::{BYTECODE_VERSION, Instruction, Program};
    use kiln_module_resolver::{NodePackageResolver, NodeRuntimeResolver};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            // nextest runs each test in its own process, so a process-local counter alone makes
            // every parallel test claim `kiln-module-graph-0` and race on deletion and writes.
            let root =
                std::env::temp_dir().join(format!("kiln-module-graph-{}-{id}", std::process::id()));
            fs::create_dir_all(&root).expect("create temporary project");
            Self { root }
        }

        fn write(&self, name: &str, source: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::write(&path, source).expect("write source");
            path
        }

        fn graph(&self, entry: &Path) -> Result<ModuleGraph, ModuleGraphError> {
            fn load(path: &Path) -> io::Result<String> {
                fs::read_to_string(path)
            }

            ModuleGraph::build(entry, &self.root, load, frontend_oxc::parse)
        }
    }

    #[test]
    fn node_graph_records_package_commonjs_source_identity() {
        let project = TempProject::new();
        fs::create_dir_all(project.root.join("node_modules/pkg")).unwrap();
        project.write(
            "node_modules/pkg/package.json",
            r#"{"name":"pkg","main":"index.js","type":"commonjs"}"#,
        );
        let package = project.write("node_modules/pkg/index.js", "module.exports = 1;");
        let entry = project.write("main.mjs", "import value from 'pkg'; console.log(value);");
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_typed_policy(&entry, resolver, |path, source_kind| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = match source_kind {
                ModuleSource::CommonJs => {
                    frontend_oxc::parse_commonjs(path.to_str().unwrap(), &source)
                }
                _ => frontend_oxc::parse(path.to_str().unwrap(), &source),
            };
            let mut program = match source_kind {
                ModuleSource::CommonJs => kiln_ir::compile_commonjs(&hir),
                _ => compile_module(&hir),
            }
            .map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })?;
            if fs::canonicalize(path).ok() == fs::canonicalize(&package).ok() {
                program.exports.push(ExportBinding {
                    exported: "default".into(),
                    local_slot: 0,
                });
            }
            Ok(ModulePayload::Program(program))
        })
        .expect("build package graph");
        assert_eq!(
            graph.module(&package).unwrap().source,
            ModuleSource::CommonJs
        );
        assert_eq!(graph.module(&entry).unwrap().source, ModuleSource::Source);
    }

    #[test]
    fn commonjs_prototype_method_carries_module_bindings_across_the_graph() {
        let project = TempProject::new();
        let package_dir = project.root.join("node_modules/router");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("package.json"),
            r#"{"name":"router","main":"layer.js","type":"commonjs"}"#,
        )
        .unwrap();
        let layer = project.write(
            "node_modules/router/layer.js",
            r#"
                const MODULE_RE = /x/;
                module.exports = Layer;
                function Layer() {}
                Layer.prototype.match = function match(path) {
                    return MODULE_RE.exec(path);
                };
            "#,
        );
        let entry = project.write(
            "main.mjs",
            "import Layer from 'router'; new Layer().match('x');",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build CommonJS graph");
        let module = &graph.module(&layer).expect("layer node").program;
        let method = module
            .program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("match"))
            .expect("match proto");
        assert!(
            method.code.contains(&Instruction::LoadUpvalue(0)),
            "{method:?}"
        );
        assert!(
            !method
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
            "{method:?}"
        );
    }

    #[test]
    fn express_router_match_has_no_caller_relative_module_reads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/server/packages/node_modules/router/lib/layer.js");
        if !path.exists() {
            return;
        }
        let source = fs::read_to_string(&path).expect("read pinned router layer");
        let hir = frontend_oxc::parse_commonjs(path.to_str().unwrap(), &source);
        assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
        let module = kiln_ir::compile_commonjs(&hir).expect("compile pinned router layer");
        let methods = module
            .program
            .functions
            .iter()
            .filter(|proto| proto.name.as_deref() == Some("match"))
            .collect::<Vec<_>>();
        assert!(!methods.is_empty(), "match proto");
        for method in methods {
            let globals = method
                .code
                .iter()
                .filter_map(|instruction| match instruction {
                    Instruction::LoadGlobal(slot) | Instruction::StoreGlobal(slot) => Some(*slot),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert!(
                globals.is_empty(),
                "caller-relative module slots: {globals:?}; {method:?}"
            );
        }
    }

    #[test]
    fn path_to_regexp_parameter_defaults_capture_the_module_delimiter() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/server/packages/node_modules/path-to-regexp/dist/index.js");
        if !path.exists() {
            return;
        }
        let source = fs::read_to_string(&path).expect("read pinned path-to-regexp");
        let hir = frontend_oxc::parse_commonjs(path.to_str().unwrap(), &source);
        let function = hir
            .body
            .iter()
            .find_map(|statement| match &statement.kind {
                kiln_hir::StatementKind::Function(function)
                    if function.name.as_deref() == Some("match") =>
                {
                    Some(function)
                }
                _ => None,
            })
            .expect("match declaration");
        assert_eq!(function.parameters.len(), 2, "{function:#?}");
        let module = kiln_ir::compile_commonjs(&hir).expect("compile path-to-regexp");
        let proto = module
            .program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("match"))
            .expect("match proto");
        assert!(
            proto
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadUpvalue(_))),
            "parameter {:?}; proto {proto:?}",
            function.parameters[1].pattern
        );
    }

    #[test]
    fn pinned_express_graph_has_no_function_with_caller_relative_globals() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let entry = root.join("conformance/server/packages/s05-express.mjs");
        let express = root.join("conformance/server/packages/node_modules/express/package.json");
        if !entry.exists() || !express.is_file() {
            return;
        }
        let resolver = NodeRuntimeResolver::new(&root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build pinned Express graph");
        let mut offenders = Vec::new();
        for (path, node) in graph.modules.iter() {
            let module_globals = node.program.program.local_count;
            for proto in &node.program.program.functions {
                let globals = proto
                    .code
                    .iter()
                    .filter_map(|instruction| match instruction {
                        Instruction::LoadGlobal(slot) | Instruction::StoreGlobal(slot) => {
                            Some(slot)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if !globals.is_empty() {
                    offenders.push((
                        path.strip_prefix(&root).unwrap_or(path).to_path_buf(),
                        proto.name.clone(),
                        module_globals,
                        globals,
                        proto.code.clone(),
                    ));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "caller-relative globals: {offenders:#?}"
        );
    }

    #[test]
    fn node_graph_selects_dual_package_exports_for_import_and_require() {
        let project = TempProject::new();
        let package_dir = project.root.join("node_modules/pkg");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("package.json"),
            r#"{"exports":{"import":"./esm.mjs","require":"./cjs.cjs"}}"#,
        )
        .expect("write dual package manifest");
        project.write("node_modules/pkg/esm.mjs", "export default 'import';");
        project.write("node_modules/pkg/cjs.cjs", "module.exports = 'require';");
        project.write("bridge.cjs", "module.exports = require('pkg');");
        let entry = project.write(
            "main.mjs",
            "import imported from 'pkg'; import required from './bridge.cjs'; console.log(imported, required);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build dual package graph");
        assert_eq!(
            graph
                .module(project.root.join("node_modules/pkg/esm.mjs"))
                .expect("import target node")
                .source,
            ModuleSource::Source
        );
        assert_eq!(
            graph
                .module(project.root.join("node_modules/pkg/cjs.cjs"))
                .expect("require target node")
                .source,
            ModuleSource::CommonJs
        );

        let mut host = TestHost::default();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute dual package graph");
        assert_eq!(host.output, ["import require"]);
    }

    #[test]
    fn node_modules_share_one_next_tick_and_promise_checkpoint() {
        let project = TempProject::new();
        project.write(
            "dep.mjs",
            r#"
                process.nextTick(() => console.log('dep-tick'));
                Promise.resolve().then(() => console.log('dep-promise'));
                console.log('dep-body');
                export const ready = 1;
            "#,
        );
        let entry = project.write(
            "main.mjs",
            r#"
                import { ready } from './dep.mjs';
                process.nextTick(() => console.log('entry-tick'));
                Promise.resolve().then(() => console.log('entry-promise'));
                console.log(ready);
            "#,
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build shared-realm graph");

        let mut host = TestHost::default();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute shared-realm graph");
        assert_eq!(
            host.output,
            [
                "dep-body",
                "1",
                "dep-tick",
                "entry-tick",
                "dep-promise",
                "entry-promise",
            ]
        );
    }

    struct OxcFrontend;

    impl Frontend for OxcFrontend {
        fn parse(&mut self, path: &str, source: &str) -> Module {
            frontend_oxc::parse(path, source)
        }

        fn parse_commonjs(&mut self, path: &str, source: &str) -> CommonJsParse {
            let (module, static_requires) =
                frontend_oxc::parse_commonjs_with_requires(path, source);
            CommonJsParse {
                module,
                static_requires,
            }
        }
    }

    #[test]
    fn node_builder_compiles_commonjs_wrapper_and_require_edges() {
        let project = TempProject::new();
        project.write("package.json", r#"{"type":"module"}"#);
        fs::create_dir_all(project.root.join("pkg")).unwrap();
        project.write("pkg/package.json", r#"{"type":"commonjs"}"#);
        let dependency = project.write("pkg/dep.js", "exports.answer = 42;");
        let package = project.write(
            "pkg/index.js",
            "const dep = require('./dep.js'); module.exports = dep.answer;",
        );
        let entry = project.write("main.mjs", "import value from './pkg/index.js';");
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build typed Node graph");
        let node = graph.module(&package).expect("CommonJS package node");
        assert_eq!(node.source, ModuleSource::CommonJs);
        assert_eq!(node.program.dependencies, ["./dep.js"]);
        assert_eq!(node.program.imports.len(), 5);
        assert_eq!(node.dependencies.len(), 1);
        assert_eq!(node.dependencies[0].specifier, "./dep.js");
        assert_eq!(
            node.dependencies[0].target,
            fs::canonicalize(dependency).unwrap()
        );
    }

    #[test]
    fn node_executes_commonjs_require_and_esm_default_interop() {
        let project = TempProject::new();
        project.write("package.json", r#"{"type":"module"}"#);
        fs::create_dir_all(project.root.join("pkg")).unwrap();
        project.write("pkg/package.json", r#"{"type":"commonjs"}"#);
        project.write("pkg/dep.js", "module.exports = 41;");
        project.write(
            "pkg/index.js",
            "const answer = require('./dep.js'); module.exports = answer + 1;",
        );
        let entry = project.write(
            "main.mjs",
            "import answer from './pkg/index.js'; console.log(answer);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build mixed graph");
        let mut host = kiln_runtime::CaptureHost::new();
        let execution = graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute mixed graph");
        assert_eq!(host.into_stdout(), "42\n");
        assert_eq!(execution.value, Value::Undefined);
    }

    #[test]
    fn node_executes_top_level_await_before_finishing_module() {
        let project = TempProject::new();
        let entry = project.write(
            "main.mjs",
            "await Promise.resolve(41); console.log('module-ready');",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build top-level await graph");

        let mut host = kiln_runtime::CaptureHost::new();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute top-level await graph");
        assert_eq!(host.into_stdout(), "module-ready\n");
    }

    #[test]
    fn node_dependency_top_level_await_finishes_before_importer() {
        let project = TempProject::new();
        project.write(
            "dep.mjs",
            "console.log('dep-before'); await Promise.resolve(); console.log('dep-after'); export const value = 42;",
        );
        let entry = project.write(
            "main.mjs",
            "import { value } from './dep.mjs'; console.log(value);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build dependency top-level await graph");

        let mut host = kiln_runtime::CaptureHost::new();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute dependency top-level await graph");
        assert_eq!(host.into_stdout(), "dep-before\ndep-after\n42\n");
    }

    #[test]
    fn node_dependency_top_level_await_rejection_stops_importer() {
        let project = TempProject::new();
        project.write(
            "dep.mjs",
            "console.log('dep-before'); await Promise.reject('dep failed'); console.log('dep-after'); export const value = 42;",
        );
        let entry = project.write(
            "main.mjs",
            "import { value } from './dep.mjs'; console.log(value);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build rejecting dependency top-level await graph");

        let mut host = kiln_runtime::CaptureHost::new();
        let error = graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect_err("rejected dependency top-level await must stop the importer");
        assert!(matches!(
            error,
            ModuleGraphError::Execution {
                error: ExecutionError::Uncaught { .. },
                ..
            }
        ));
        assert_eq!(host.into_stdout(), "dep-before\n");
    }

    #[test]
    fn node_reports_rejected_top_level_await_as_module_failure() {
        let project = TempProject::new();
        let entry = project.write("main.mjs", "await Promise.reject('module failed');");
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build rejecting top-level await graph");

        let mut host = kiln_runtime::CaptureHost::new();
        let error = graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect_err("rejected top-level await must fail module evaluation");
        assert!(matches!(
            error,
            ModuleGraphError::Execution {
                error: ExecutionError::Uncaught { .. },
                ..
            }
        ));
    }

    #[test]
    fn node_executes_top_level_for_await() {
        let project = TempProject::new();
        let entry = project.write(
            "main.mjs",
            "for await (const value of [Promise.resolve(7)]) console.log(value);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build top-level for-await graph");

        let mut host = kiln_runtime::CaptureHost::new();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute top-level for-await graph");
        assert_eq!(host.into_stdout(), "7\n");
    }

    #[test]
    fn node_commonjs_builtin_projection_preserves_namespace_named_exports() {
        let project = TempProject::new();
        project.write(
            "bridge.cjs",
            concat!(
                "const hooks = require('node:async_hooks');",
                "const resource = new hooks.AsyncResource('probe');",
                "module.exports = typeof resource.bind === 'function';",
            ),
        );
        let entry = project.write(
            "main.mjs",
            "import supported from './bridge.cjs'; console.log(supported);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build builtin CommonJS graph");

        let mut host = kiln_runtime::CaptureHost::new();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute builtin CommonJS graph");
        assert_eq!(host.into_stdout(), "true\n");
    }

    #[test]
    fn node_url_and_querystring_legacy_parsers_work_in_esm_and_commonjs() {
        let project = TempProject::new();
        project.write(
            "bridge.cjs",
            concat!(
                "const url = require('url');",
                "const qs = require('querystring');",
                "const parsed = url.parse('/hello?hello=world&foo=b%3D%3Dar');",
                "const query = qs.parse(parsed.query);",
                "module.exports = parsed.pathname + ' ' + query.hello + ' ' + query.foo;",
            ),
        );
        let entry = project.write(
            "main.mjs",
            concat!(
                "import { parse as parseUrl } from 'node:url';",
                "import { parse as parseQuery } from 'node:querystring';",
                "import legacy from './bridge.cjs';",
                "const parsed = parseUrl('/route?a=1&a=2#hash');",
                "const query = parseQuery(parsed.query);",
                "console.log(legacy);",
                "console.log(parsed.pathname, parsed.search, parsed.hash, query.a.join(','));",
            ),
        );
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(&project.root).expect("Node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build legacy URL/querystring graph");

        let mut host = kiln_runtime::CaptureHost::new();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute legacy URL/querystring graph");
        assert_eq!(
            host.into_stdout(),
            "/hello world b==ar\n/route ?a=1&a=2 #hash 1,2\n"
        );
    }

    #[test]
    fn node_reads_accessor_backed_module_exports_through_commonjs_semantics() {
        let project = TempProject::new();
        project.write(
            "facade.cjs",
            concat!(
                "const prototype = { answer: 42 };",
                "Object.defineProperty(module, 'exports', {",
                "  get() { return Object.create(prototype); }",
                "});",
            ),
        );
        let entry = project.write(
            "main.mjs",
            "import facade from './facade.cjs'; console.log(facade.answer);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build accessor-backed CommonJS graph");

        let mut host = kiln_runtime::CaptureHost::new();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute accessor-backed CommonJS graph");
        assert_eq!(host.into_stdout(), "42\n");
    }

    #[test]
    fn node_executes_relative_cjs_default_import_and_publishes_output() {
        let project = TempProject::new();
        let dependency = project.write("dep.cjs", "module.exports = 42;");
        let entry = project.write(
            "main.mjs",
            "import value from './dep.cjs'; console.log(value);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build relative CommonJS graph");

        let dependency = fs::canonicalize(dependency).expect("canonical dependency");
        assert_eq!(
            graph.module(&dependency).expect("dependency node").source,
            ModuleSource::CommonJs
        );
        assert_eq!(
            graph.resolved_export(&dependency, "default"),
            Some(&ResolvedExport::Local {
                module: dependency.clone(),
                local_slot: 0,
            })
        );

        let mut host = TestHost::default();
        let execution = graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute relative CommonJS graph");
        assert_eq!(host.output, ["42"]);
        assert_eq!(execution.value, Value::Undefined);
        assert!(execution.steps > 0);
    }

    #[test]
    fn node_discovers_nested_conditional_commonjs_require() {
        let project = TempProject::new();
        let dependency = project.write("dep.cjs", "module.exports = 41;");
        let bridge = project.write(
            "bridge.cjs",
            "if (true) { const value = require('./dep.cjs'); module.exports = value + 1; }",
        );
        let entry = project.write(
            "main.mjs",
            "import value from './bridge.cjs'; console.log(value);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build nested CommonJS graph");

        let dependency = fs::canonicalize(dependency).expect("canonical dependency");
        let bridge_node = graph.module(&bridge).expect("bridge node");
        assert_eq!(bridge_node.program.dependencies, ["./dep.cjs"]);
        assert_eq!(bridge_node.dependencies[0].target, dependency);

        let mut host = TestHost::default();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute nested CommonJS graph");
        assert_eq!(host.output, ["42"]);
    }

    #[test]
    fn node_commonjs_require_is_lazy_in_dead_branches_and_unused_functions() {
        let project = TempProject::new();
        let dependency = project.write("dep.cjs", "console.log('executed'); module.exports = 42;");
        let bridge = project.write(
            "bridge.cjs",
            "if (false) { require('./dep.cjs'); } function load() { return require('./dep.cjs'); } module.exports = 1;",
        );
        let entry = project.write(
            "main.mjs",
            "import value from './bridge.cjs'; console.log(value);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build lazy CommonJS graph");
        let bridge_node = graph.module(&bridge).expect("bridge node");
        assert_eq!(bridge_node.program.dependencies, ["./dep.cjs"]);
        assert_eq!(
            bridge_node.dependencies[0].target,
            fs::canonicalize(dependency).expect("canonical dependency")
        );

        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute lazy CommonJS graph");
        assert_eq!(host.output, ["1"]);
    }

    #[test]
    fn node_commonjs_missing_require_throws_at_execution_and_can_be_caught() {
        let project = TempProject::new();
        let bridge = project.write(
            "bridge.cjs",
            "try { require('not-installed'); } catch { module.exports = 42; }",
        );
        let entry = project.write(
            "main.mjs",
            "import value from './bridge.cjs'; console.log(value);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("an unresolved CommonJS require is runtime work");
        assert!(
            graph
                .module(&bridge)
                .expect("bridge node")
                .dependencies
                .is_empty()
        );

        let mut host = TestHost::default();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("JavaScript catches the missing require");
        assert_eq!(host.output, ["42"]);
    }

    #[test]
    fn node_commonjs_require_executes_a_target_once_and_reuses_its_identity() {
        let project = TempProject::new();
        project.write("dep.cjs", "exports.count = (exports.count || 0) + 1;");
        project.write(
            "bridge.cjs",
            "const first = require('./dep.cjs'); const second = require('./dep.cjs'); module.exports = first === second && first.count;",
        );
        let entry = project.write(
            "main.mjs",
            "import value from './bridge.cjs'; console.log(value);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build cached CommonJS graph");

        let mut host = kiln_runtime::CaptureHost::new();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute cached CommonJS graph");
        assert_eq!(host.into_stdout(), "1\n");
    }

    #[test]
    fn node_commonjs_require_remains_lazy_when_a_captured_loader_is_called_later() {
        let project = TempProject::new();
        project.write("dep.cjs", "console.log('executed'); module.exports = 42;");
        project.write("bridge.cjs", "module.exports = () => require('./dep.cjs');");
        let entry = project.write(
            "main.mjs",
            "import load from './bridge.cjs'; console.log(load());",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build captured CommonJS loader graph");

        let mut host = kiln_runtime::CaptureHost::new();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute captured CommonJS loader graph");
        assert_eq!(host.into_stdout(), "executed\n42\n");
    }

    #[test]
    fn node_commonjs_require_reads_esm_export_values() {
        let project = TempProject::new();
        project.write("dep.mjs", "export let answer = 41;");
        project.write(
            "bridge.cjs",
            "const dep = require('./dep.mjs'); module.exports = dep.answer + 1;",
        );
        let entry = project.write(
            "main.mjs",
            "import value from './bridge.cjs'; console.log(value);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build mixed ESM/CommonJS graph");

        let mut host = TestHost::default();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute mixed ESM/CommonJS graph");
        assert_eq!(host.output, ["42"]);
    }

    #[test]
    fn node_commonjs_require_cycles_share_partial_cache_and_execute_once() {
        let project = TempProject::new();
        project.write(
            "a.cjs",
            "exports.count = (exports.count || 0) + 1; const b = require('./b.cjs'); exports.seenB = b.count;",
        );
        project.write(
            "b.cjs",
            "exports.count = (exports.count || 0) + 1; const a = require('./a.cjs'); exports.seenA = a.count;",
        );
        let entry = project.write(
            "main.mjs",
            "import a from './a.cjs'; import b from './b.cjs'; console.log(a.count, b.count, b.seenA, a.seenB);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build CommonJS cycle");
        assert_eq!(graph.module_count(), 3);
        assert_eq!(
            graph
                .module_order()
                .iter()
                .filter(|path| {
                    path.extension().and_then(|extension| extension.to_str()) == Some("cjs")
                })
                .count(),
            2
        );

        let mut host = kiln_runtime::CaptureHost::new();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute CommonJS cycle");
        assert_eq!(host.into_stdout(), "1 1 1 1\n");
    }

    #[test]
    fn node_commonjs_esm_cycles_remain_structured_errors() {
        let project = TempProject::new();
        project.write("a.cjs", "require('./b.mjs');");
        project.write("b.mjs", "import './a.cjs';");
        let entry = project.write("main.mjs", "import './a.cjs';");
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let error = ModuleGraph::build_node(
            &entry,
            resolver,
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect_err("mixed ESM/CommonJS cycle should be rejected");
        assert!(matches!(error, ModuleGraphError::Cycle { paths } if paths.len() == 3));
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[derive(Default)]
    struct TestHost {
        output: Vec<String>,
    }

    impl HostOps for TestHost {
        fn call(
            &mut self,
            operation: HostOp,
            arguments: &[HostValue],
        ) -> Result<HostValue, HostError> {
            assert_eq!(operation, HostOp::WriteStdout);
            self.output.push(
                arguments
                    .iter()
                    .map(HostValue::to_js_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            Ok(HostValue::Undefined)
        }
    }

    struct ServerHostForPort {
        sender: Option<std::sync::mpsc::Sender<u16>>,
        output: Vec<String>,
    }

    fn http_header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
        response
            .split_once("\r\n\r\n")
            .expect("HTTP response has a header terminator")
            .0
            .lines()
            .skip(1)
            .find_map(|line| {
                let (header_name, value) = line.split_once(':')?;
                header_name
                    .eq_ignore_ascii_case(name)
                    .then_some(value.trim())
            })
    }

    fn http_status(response: &str) -> &str {
        response
            .split_once("\r\n")
            .expect("HTTP response has a status line")
            .0
    }

    fn http_body(response: &str) -> Vec<u8> {
        let (head, body) = response
            .split_once("\r\n\r\n")
            .expect("HTTP response has a header terminator");
        let body = body.as_bytes();
        if http_header(response, "transfer-encoding").is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("chunked"))
        }) {
            let mut decoded = Vec::new();
            let mut cursor = 0;
            loop {
                let line_end = body[cursor..]
                    .windows(2)
                    .position(|window| window == b"\r\n")
                    .map(|offset| cursor + offset)
                    .expect("chunk has a size line");
                let size_text = std::str::from_utf8(&body[cursor..line_end])
                    .expect("chunk size is UTF-8")
                    .split(';')
                    .next()
                    .expect("chunk size is present");
                let size =
                    usize::from_str_radix(size_text.trim(), 16).expect("chunk size is hexadecimal");
                cursor = line_end + 2;
                if size == 0 {
                    assert!(
                        body[cursor..].starts_with(b"\r\n"),
                        "chunked body is terminated"
                    );
                    break;
                }
                assert!(
                    body.len() >= cursor + size + 2,
                    "chunk has its full payload"
                );
                decoded.extend_from_slice(&body[cursor..cursor + size]);
                assert_eq!(&body[cursor + size..cursor + size + 2], b"\r\n");
                cursor += size + 2;
            }
            return decoded;
        }

        let length = http_header(response, "content-length")
            .expect("non-chunked response has Content-Length")
            .parse::<usize>()
            .expect("Content-Length is numeric");
        assert_eq!(
            head.len() + 4 + length,
            response.len(),
            "response body has its declared length"
        );
        body[..length].to_vec()
    }

    impl HostOps for ServerHostForPort {
        fn call(
            &mut self,
            operation: HostOp,
            arguments: &[HostValue],
        ) -> Result<HostValue, HostError> {
            assert_eq!(operation, HostOp::WriteStdout);
            let text = arguments
                .first()
                .map_or_else(String::new, HostValue::to_js_string);
            if let Ok(port) = text.parse::<u16>()
                && let Some(sender) = self.sender.take()
            {
                sender.send(port).expect("publish server port");
            }
            self.output.push(text);
            Ok(HostValue::Undefined)
        }
    }

    #[test]
    fn named_import_alias_links_and_executes() {
        let project = TempProject::new();
        let dependency = project.write("dep.js", "export const value = 42;");
        let entry = project.write(
            "main.js",
            "import { value as answer } from './dep.js'; console.log(answer);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let node = graph.module(&entry).expect("entry node");
        assert_eq!(
            node.dependencies[0].target,
            fs::canonicalize(dependency).unwrap()
        );

        let mut host = TestHost::default();
        let result = graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["42"]);
        assert!(result.steps > 0);
    }

    #[test]
    fn package_policy_links_a_named_package_import() {
        let project = TempProject::new();
        let package = project.root.join("node_modules/pkg");
        fs::create_dir_all(&package).expect("create package");
        fs::write(package.join("package.json"), r#"{"exports":"./index.js"}"#)
            .expect("write package manifest");
        fs::write(package.join("index.js"), "export const value = 42;")
            .expect("write package entry");
        let entry = project.write(
            "main.js",
            "import { value } from 'pkg'; console.log(value);",
        );
        let resolver = NodePackageResolver::new(&project.root).expect("package resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build package graph");

        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute package graph");
        assert_eq!(host.output, ["42"]);
    }

    #[test]
    fn node_builtin_static_imports_are_typed_virtual_modules() {
        let project = TempProject::new();
        let entry = project.write(
            "main.js",
            concat!(
                "import { EventEmitter } from 'node:events';",
                "import * as fs from 'node:fs/promises';",
                "console.log(typeof EventEmitter, typeof fs.readFile, Object.keys(fs).join(','));",
            ),
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let mut loaded = Vec::new();
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            loaded.push(path.to_path_buf());
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build graph with builtins");

        // Only the real entry passed through the source provider. Builtins are typed graph nodes,
        // not fake files or generated JavaScript.
        assert_eq!(loaded, [fs::canonicalize(&entry).unwrap()]);
        assert_eq!(graph.module_count(), 3);
        assert_eq!(
            graph
                .modules()
                .filter(|node| matches!(node.source, ModuleSource::NodeBuiltin(_)))
                .count(),
            2
        );

        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute builtin imports");
        assert_eq!(host.output, ["function function readFile,stat,writeFile"]);
    }

    #[test]
    fn node_net_connect_is_a_typed_virtual_module_export() {
        let project = TempProject::new();
        let entry = project.write(
            "main.js",
            "import { connect } from 'node:net'; console.log(connect);",
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("implemented builtin export links");
        let mut host = TestHost::default();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("execute typed node:net module");
        assert_eq!(host.output, ["[Function: nodeNetConnect]"]);
    }

    #[test]
    fn node_graph_execution_is_an_explicit_host_capability() {
        let project = TempProject::new();
        let payload = project.write("payload.txt", "from tokio");
        let payload = payload
            .to_str()
            .unwrap()
            .replace('\\', "\\\\")
            .replace('\'', "\\'");
        let entry = project.write(
            "main.js",
            &format!(
                "import {{ readFile }} from 'node:fs/promises'; readFile('{payload}', 'utf8').then(value => console.log(value));",
            ),
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build Node graph");

        let mut deterministic_host = TestHost::default();
        let error = graph
            .execute(&mut deterministic_host, ExecutionLimits::default())
            .expect_err("deterministic execution must not gain host I/O from an import");
        assert!(matches!(
            error,
            ModuleGraphError::Execution {
                error: ExecutionError::Host { message, .. },
                ..
            } if message.contains("requires the Node execution Adapter")
        ));

        let mut node_host = TestHost::default();
        graph
            .execute_node(&mut node_host, ExecutionLimits::default())
            .expect("explicit Node graph execution");
        assert_eq!(node_host.output, ["from tokio"]);
    }

    #[test]
    fn node_http_static_module_serves_one_live_request_on_tokio() {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpStream};
        use std::sync::mpsc;
        use std::time::Duration;

        let project = TempProject::new();
        let entry = project.write(
            "main.js",
            concat!(
                "import { createServer } from 'node:http';",
                "let server;",
                "server = createServer((request, response) => {",
                "  response.setHeader('Content-Type', 'text/plain');",
                "  response.end(request.method + ' ' + request.url);",
                "  server.close();",
                "});",
                "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
            ),
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build HTTP graph");

        let (port_sender, port_receiver) = mpsc::channel();
        struct ServerHost {
            port_sender: Option<mpsc::Sender<u16>>,
        }
        impl HostOps for ServerHost {
            fn call(
                &mut self,
                operation: HostOp,
                arguments: &[HostValue],
            ) -> Result<HostValue, HostError> {
                assert_eq!(operation, HostOp::WriteStdout);
                let port = arguments
                    .first()
                    .and_then(|value| value.to_js_string().parse::<u16>().ok())
                    .expect("listening callback prints port");
                if let Some(sender) = self.port_sender.take() {
                    sender.send(port).expect("send server port");
                }
                Ok(HostValue::Undefined)
            }
        }
        let client = std::thread::spawn(move || {
            let port = port_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("server announces port");
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect client");
            stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .expect("write request");
            stream
                .shutdown(Shutdown::Write)
                .expect("finish request bytes");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            let mut response = Vec::new();
            stream.read_to_end(&mut response).expect("read response");
            String::from_utf8(response).expect("UTF-8 response")
        });

        let mut host = ServerHost {
            port_sender: Some(port_sender),
        };
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("serve live request");
        let response = client.join().expect("client thread");
        assert_eq!(http_status(&response), "HTTP/1.1 200 OK");
        assert_eq!(http_header(&response, "content-type"), Some("text/plain"));
        assert_eq!(http_body(&response), b"GET /health");
    }

    #[test]
    fn node_http_create_server_accepts_options_before_the_listener() {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpStream};
        use std::sync::mpsc;
        use std::time::Duration;

        let project = TempProject::new();
        let entry = project.write(
            "main.js",
            concat!(
                "import { createServer } from 'node:http';",
                "let server;",
                "server = createServer({ keepAlive: false }, (request, response) => {",
                "  response.end('options ' + request.url);",
                "  server.close();",
                "});",
                "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
            ),
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build HTTP graph");

        let (port_sender, port_receiver) = mpsc::channel();
        struct ServerHost {
            port_sender: Option<mpsc::Sender<u16>>,
        }
        impl HostOps for ServerHost {
            fn call(
                &mut self,
                operation: HostOp,
                arguments: &[HostValue],
            ) -> Result<HostValue, HostError> {
                assert_eq!(operation, HostOp::WriteStdout);
                let port = arguments
                    .first()
                    .and_then(|value| value.to_js_string().parse::<u16>().ok())
                    .expect("listening callback prints port");
                if let Some(sender) = self.port_sender.take() {
                    sender.send(port).expect("send server port");
                }
                Ok(HostValue::Undefined)
            }
        }
        let client = std::thread::spawn(move || {
            let port = port_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("server announces port");
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect client");
            stream
                .write_all(
                    b"GET /configured HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .expect("write request");
            stream.shutdown(Shutdown::Write).expect("finish request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read response");
            response
        });

        let mut host = ServerHost {
            port_sender: Some(port_sender),
        };
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("serve options-form request");
        let response = client.join().expect("client thread");
        assert_eq!(http_status(&response), "HTTP/1.1 200 OK");
        assert_eq!(http_body(&response), b"options /configured");
    }

    #[test]
    fn node_http_request_and_response_share_the_accepted_socket_identity() {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpStream};
        use std::sync::mpsc;
        use std::time::Duration;

        let project = TempProject::new();
        let entry = project.write(
            "main.js",
            concat!(
                "import { createServer } from 'node:http';",
                "let server;",
                "server = createServer((request, response) => {",
                "  if (!request.socket || request.connection !== request.socket) throw new Error('request socket');",
                "  if (response.req !== request) throw new Error('response request');",
                "  if (response.socket !== request.socket || response.connection !== request.socket) throw new Error('response socket');",
                "  response.end('identity');",
                "  server.close();",
                "});",
                "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
            ),
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build HTTP identity graph");

        let (port_sender, port_receiver) = mpsc::channel();
        let client = std::thread::spawn(move || {
            let port = port_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("server publishes its port");
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .expect("write request");
            stream.shutdown(Shutdown::Write).expect("finish request");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read response");
            response
        });
        let mut host = ServerHostForPort {
            sender: Some(port_sender),
            output: Vec::new(),
        };
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("serve identity request");
        let response = client.join().expect("client completes");
        assert_eq!(http_status(&response), "HTTP/1.1 200 OK");
        assert_eq!(http_body(&response), b"identity");
    }

    #[test]
    fn node_http_request_body_is_a_readable_event_stream() {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpStream};
        use std::sync::mpsc;
        use std::time::Duration;

        let project = TempProject::new();
        let entry = project.write(
            "main.js",
            concat!(
                "import { createServer } from 'node:http';",
                "let server;",
                "server = createServer((request, response) => {",
                "  let bytes = 0;",
                "  request.on('data', chunk => { bytes += chunk.length; });",
                "  request.on('end', () => {",
                "    response.writeHead(201, { 'Content-Type': 'text/plain' });",
                "    response.end(request.method + ' ' + bytes);",
                "    server.close();",
                "  });",
                "});",
                "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
            ),
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build HTTP graph");

        let (port_sender, port_receiver) = mpsc::channel();
        let client = std::thread::spawn(move || {
            let port = port_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("server publishes its port");
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
            stream
                .write_all(
                    b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 12\r\nConnection: close\r\n\r\nhello ",
                )
                .expect("write request head and first body chunk");
            stream
                .write_all(b"tokio!")
                .expect("write second body chunk");
            stream.shutdown(Shutdown::Write).expect("finish request");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read response");
            response
        });

        struct ServerHost {
            port_sender: Option<mpsc::Sender<u16>>,
        }
        impl HostOps for ServerHost {
            fn call(
                &mut self,
                operation: HostOp,
                arguments: &[HostValue],
            ) -> Result<HostValue, HostError> {
                assert_eq!(operation, HostOp::WriteStdout);
                let port = arguments
                    .first()
                    .and_then(|value| value.to_js_string().parse::<u16>().ok())
                    .expect("server logs numeric port");
                if let Some(sender) = self.port_sender.take() {
                    sender.send(port).expect("publish server port");
                }
                Ok(HostValue::Undefined)
            }
        }
        let mut host = ServerHost {
            port_sender: Some(port_sender),
        };
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("serve request body");
        let response = client.join().expect("client completes");
        assert_eq!(http_status(&response), "HTTP/1.1 201 Created");
        assert_eq!(http_header(&response, "content-type"), Some("text/plain"));
        assert_eq!(http_body(&response), b"POST 12");
    }

    #[test]
    fn node_http_request_pause_and_resume_control_tokio_reads() {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpStream};
        use std::sync::mpsc;
        use std::time::Duration;

        let project = TempProject::new();
        let entry = project.write(
            "main.js",
            concat!(
                "import { createServer } from 'node:http';",
                "let server;",
                "server = createServer((request, response) => {",
                "  request.pause();",
                "  let chunks = 0;",
                "  request.on('data', chunk => { chunks += 1; });",
                "  request.on('end', () => { response.end(String(chunks)); server.close(); });",
                "  setTimeout(() => request.resume(), 1);",
                "});",
                "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
            ),
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build paused HTTP graph");

        let (port_sender, port_receiver) = mpsc::channel();
        let client = std::thread::spawn(move || {
            let port = port_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("server publishes its port");
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
            stream
                .write_all(
                    b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                )
                .expect("write request");
            stream.shutdown(Shutdown::Write).expect("finish request");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read response");
            response
        });
        let mut host = ServerHostForPort {
            sender: Some(port_sender),
            output: Vec::new(),
        };
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("serve paused request");
        let response = client.join().expect("client completes");
        assert_eq!(http_status(&response), "HTTP/1.1 200 OK");
        assert_eq!(http_body(&response), b"1");
    }

    #[test]
    fn node_http_response_write_streams_chunked_body_and_reports_pressure() {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpStream};
        use std::sync::mpsc;
        use std::time::Duration;

        let project = TempProject::new();
        let entry = project.write(
            "main.js",
            concat!(
                "import { createServer } from 'node:http';",
                "let server;",
                "server = createServer((request, response) => {",
                "  const ready = response.write('hello ');",
                "  response.end('tokio');",
                "  console.log(ready);",
                "  server.close();",
                "});",
                "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
            ),
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build streaming HTTP graph");

        let (port_sender, port_receiver) = mpsc::channel();
        let client = std::thread::spawn(move || {
            let port = port_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("server publishes its port");
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .expect("write request");
            stream.shutdown(Shutdown::Write).expect("finish request");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read response");
            response
        });
        let mut host = ServerHostForPort {
            sender: Some(port_sender),
            output: Vec::new(),
        };
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("serve streaming response");
        let response = client.join().expect("client completes");
        assert_eq!(http_status(&response), "HTTP/1.1 200 OK");
        assert_eq!(http_header(&response, "transfer-encoding"), Some("chunked"));
        assert_eq!(http_body(&response), b"hello tokio");
        assert_eq!(host.output.last().map(String::as_str), Some("true"));
    }

    #[test]
    fn node_net_server_echoes_over_the_shared_tokio_stream_driver() {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpStream};
        use std::sync::mpsc;
        use std::time::Duration;

        let project = TempProject::new();
        let entry = project.write(
            "main.js",
            concat!(
                "import { createServer } from 'node:net';",
                "let server;",
                "server = createServer(socket => {",
                "  socket.on('data', chunk => socket.end(chunk));",
                "  socket.on('finish', () => server.close());",
                "});",
                "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
            ),
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build net graph");

        let (port_sender, port_receiver) = mpsc::channel();
        struct ServerHost(Option<mpsc::Sender<u16>>);
        impl HostOps for ServerHost {
            fn call(
                &mut self,
                operation: HostOp,
                arguments: &[HostValue],
            ) -> Result<HostValue, HostError> {
                assert_eq!(operation, HostOp::WriteStdout);
                let port = arguments[0]
                    .to_js_string()
                    .parse::<u16>()
                    .expect("numeric port");
                if let Some(sender) = self.0.take() {
                    sender.send(port).expect("publish port");
                }
                Ok(HostValue::Undefined)
            }
        }
        let client = std::thread::spawn(move || {
            let port = port_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("server announces port");
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
            stream.write_all(b"tokio net").expect("write payload");
            stream.shutdown(Shutdown::Write).expect("finish request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut echoed = Vec::new();
            stream.read_to_end(&mut echoed).expect("read echo");
            echoed
        });
        let mut host = ServerHost(Some(port_sender));
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("run net server");
        assert_eq!(client.join().expect("client completes"), b"tokio net");
    }

    #[test]
    fn node_net_client_connects_over_the_shared_tokio_stream_driver() {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpListener};
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind peer");
        let port = listener.local_addr().unwrap().port();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept kiln client");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4];
            stream
                .read_exact(&mut request)
                .expect("read client payload");
            stream.write_all(b"pong!").expect("write reply");
            stream.shutdown(Shutdown::Write).expect("finish reply");
            request
        });

        let project = TempProject::new();
        let entry = project.write(
            "main.js",
            &format!(
                concat!(
                    "import {{ connect }} from 'node:net';",
                    "const socket = connect({}, '127.0.0.1', () => socket.write('ping'));",
                    "let bytes = 0;",
                    "socket.on('data', chunk => bytes += chunk.length);",
                    "socket.on('end', () => {{ console.log(bytes); socket.end(); }});",
                ),
                port
            ),
        );
        let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
        let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
            let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
            compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            })
        })
        .expect("build net client graph");
        let mut host = TestHost::default();
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("run net client");
        assert_eq!(peer.join().expect("peer completes"), *b"ping");
        assert_eq!(host.output, ["5"]);
    }

    /// A function value survives the module boundary it was exported across.
    ///
    /// A closure indexes its own program's function table, and every module compiles to its own
    /// program -- so calling an imported function used to reach into the *importer's* table and
    /// find someone else's body or nothing at all. The closure carries its program now, which is
    /// what makes `export function` mean anything.
    #[test]
    fn exported_functions_and_classes_are_callable_by_the_importer() {
        let project = TempProject::new();
        project.write(
            "lib.js",
            "export function greet(name) { return 'hi ' + name; }\n\
             export class Box { constructor(v) { this.v = v; } get() { return this.v; } }\n\
             export const twice = (n) => n * 2;\n\
             export default function main() { return 'default'; }",
        );
        let entry = project.write(
            "main.js",
            "import main, { greet, Box, twice } from './lib.js';\n\
             console.log(greet('x'));\n\
             console.log(new Box(7).get());\n\
             console.log(twice(21));\n\
             console.log(main());",
        );
        let graph = project.graph(&entry).expect("build graph");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["hi x", "7", "42", "default"]);
    }

    /// An imported function still sees the module it was written in, not the one calling it.
    ///
    /// The captured binding and the constant both live in the exporter's program; reading either
    /// through the importer's tables would give the wrong answer rather than no answer, which is
    /// the failure worth a test of its own.
    #[test]
    fn an_imported_function_closes_over_its_own_module() {
        let project = TempProject::new();
        project.write(
            "lib.js",
            "const secret = 'lib';\n\
             export function reveal() { return secret + '/' + 'constant'; }",
        );
        let entry = project.write(
            "main.js",
            "import { reveal } from './lib.js';\n\
             const secret = 'main';\n\
             console.log(reveal(), secret);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["lib/constant main"]);
    }

    /// `import * as ns` binds one object holding every export, with keys in sorted order.
    ///
    /// Sorted because the specification orders a namespace object's keys and because the linker
    /// holds exports in a hash map: without sorting, `Object.keys(ns)` would differ run to run.
    #[test]
    fn a_namespace_import_binds_every_export_as_one_object() {
        let project = TempProject::new();
        project.write(
            "lib.js",
            "export const one = 1;\n\
             export function two() { return 2; }\n\
             export default 'zero';",
        );
        let entry = project.write(
            "main.js",
            "import * as lib from './lib.js';\n\
             console.log(Object.keys(lib).join(','));\n\
             console.log(lib.one, lib.two(), lib.default);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["default,one,two", "1 2 zero"]);
    }

    /// A named re-export forwards a provider's binding without the module ever naming it.
    #[test]
    fn named_re_exports_forward_without_a_local_binding() {
        let project = TempProject::new();
        project.write(
            "base.js",
            "export const alpha = 'a';\n\
             export function beta() { return 'b'; }\n\
             export default 'delta';",
        );
        project.write(
            "middle.js",
            "export { alpha, beta as gamma } from './base.js';\n\
             export { default as delta } from './base.js';\n\
             export * as everything from './base.js';",
        );
        let entry = project.write(
            "main.js",
            "import { alpha, gamma, delta, everything } from './middle.js';\n\
             console.log(alpha, gamma(), delta, Object.keys(everything).join(','));",
        );
        let graph = project.graph(&entry).expect("build graph");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["a b delta alpha,beta,default"]);
    }

    /// `export * from` forwards the names the provider chooses, and never its default.
    ///
    /// A name the forwarding module exports itself wins, which is what a barrel file overriding
    /// one entry depends on. The link check has to see through the star too, or importing a
    /// forwarded name would be rejected before anything ran.
    #[test]
    fn a_star_re_export_forwards_names_but_not_the_default() {
        let project = TempProject::new();
        project.write(
            "base.js",
            "export const alpha = 'a';\n\
             export const shared = 'base';\n\
             export default 'delta';",
        );
        let wide = project.write(
            "wide.js",
            "export * from './base.js';\nexport const shared = 'wide';",
        );
        let entry = project.write(
            "main.js",
            "import * as wide from './wide.js';\n\
             import { alpha, shared } from './wide.js';\n\
             console.log(alpha, shared);\n\
             console.log(Object.keys(wide).join(','));",
        );
        let graph = project.graph(&entry).expect("build graph");
        assert_eq!(
            graph.resolved_export_names(project.root.join(".").join("wide.js")),
            Some(vec!["alpha".to_owned(), "shared".to_owned()])
        );
        assert_eq!(
            graph.resolved_export_names(&wide),
            Some(vec!["alpha".to_owned(), "shared".to_owned()])
        );
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["a wide", "alpha,shared"]);
    }

    #[test]
    fn a_named_import_rejects_an_ambiguous_star_export() {
        let project = TempProject::new();
        project.write("a.js", "export const shared = 'a';");
        project.write("b.js", "export const shared = 'b';");
        let barrel = project.write(
            "barrel.js",
            "export * from './a.js';\nexport * from './b.js';",
        );
        let entry = project.write(
            "main.js",
            "import { shared } from './barrel.js'; console.log(shared);",
        );

        assert!(matches!(
            project.graph(&entry),
            Err(ModuleGraphError::AmbiguousExport {
                importer,
                provider,
                name,
            }) if importer == fs::canonicalize(entry).unwrap()
                && provider == fs::canonicalize(barrel).unwrap()
                && name == "shared"
        ));
    }

    #[test]
    fn a_namespace_omits_ambiguous_star_exports() {
        let project = TempProject::new();
        project.write("a.js", "export const shared = 'a'; export const onlyA = 1;");
        project.write("b.js", "export const shared = 'b'; export const onlyB = 2;");
        let barrel = project.write(
            "barrel.js",
            "export * from './a.js';\nexport * from './b.js';",
        );
        let entry = project.write(
            "main.js",
            "import * as barrel from './barrel.js';\n\
             console.log(Object.keys(barrel).join(','));\n\
             console.log(barrel.onlyA, barrel.onlyB, barrel.shared);",
        );

        let graph = project
            .graph(&entry)
            .expect("ambiguous names do not poison a namespace");
        assert_eq!(
            graph.resolved_export_names(&barrel),
            Some(vec!["onlyA".to_owned(), "onlyB".to_owned()])
        );
        assert_eq!(
            graph.resolved_export_names(project.root.join("does-not-exist.js")),
            None
        );
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute namespace import");
        assert_eq!(host.output, ["onlyA,onlyB", "1 2 undefined"]);
    }

    #[test]
    fn two_star_paths_to_the_same_binding_are_not_ambiguous() {
        let project = TempProject::new();
        project.write("origin.js", "export const shared = 42;");
        project.write("left.js", "export * from './origin.js';");
        project.write("right.js", "export * from './origin.js';");
        project.write(
            "barrel.js",
            "export * from './left.js';\nexport * from './right.js';",
        );
        let entry = project.write(
            "main.js",
            "import { shared } from './barrel.js'; console.log(shared);",
        );

        let graph = project
            .graph(&entry)
            .expect("converging paths resolve to one binding");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute converging star graph");
        assert_eq!(host.output, ["42"]);
    }

    #[test]
    fn a_deep_star_chain_has_no_arbitrary_link_depth_limit() {
        const BARRELS: usize = 80;
        let project = TempProject::new();
        project.write("leaf.js", "export const answer = 42;");
        for index in (0..BARRELS).rev() {
            let provider = if index + 1 == BARRELS {
                "leaf.js".to_owned()
            } else {
                format!("barrel{}.js", index + 1)
            };
            project.write(
                &format!("barrel{index}.js"),
                &format!("export * from './{provider}';"),
            );
        }
        let entry = project.write(
            "main.js",
            "import { answer } from './barrel0.js'; console.log(answer);",
        );

        let graph = project.graph(&entry).expect("deep star chain links");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("deep star chain executes");
        assert_eq!(host.output, ["42"]);
    }

    /// An import is a live view of the exporter's binding, not a copy of the value it had when
    /// the exporter finished.
    ///
    /// `export let count` that an exported function increments is the case: the importer reads
    /// the *new* number. Only a binding that can change is boxed -- a `const` cannot, and boxing
    /// it would cost the native backend a module it can otherwise compile.
    #[test]
    fn a_named_import_is_a_live_view_of_the_exported_binding() {
        let project = TempProject::new();
        project.write(
            "counter.js",
            "export let count = 0;\n\
             export const limit = 10;\n\
             export function bump() { count += 1; }",
        );
        let entry = project.write(
            "main.js",
            "import { count, limit, bump } from './counter.js';\n\
             console.log(count, limit);\n\
             bump();\n\
             bump();\n\
             console.log(count, limit);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["0 10", "2 10"]);
    }

    /// A namespace object is a snapshot, and says so.
    ///
    /// Node's namespace objects are live; this one reads the exported cells once, when the
    /// provider finishes. Recorded as a test so the difference is a decision rather than a
    /// surprise.
    #[test]
    fn a_namespace_object_is_a_snapshot_of_the_exports() {
        let project = TempProject::new();
        project.write(
            "counter.js",
            "export let count = 0;\nexport function bump() { count += 1; }",
        );
        let entry = project.write(
            "main.js",
            "import * as counter from './counter.js';\n\
             counter.bump();\n\
             console.log(counter.count);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["0"]);
    }

    #[test]
    fn a_module_sees_its_own_exports_through_a_namespace_it_imported_from_itself() {
        let project = TempProject::new();
        // Every read is written *after* the binding it names, which is the whole point: a
        // namespace built when the linker ran would answer `undefined` to all four.
        let entry = project.write(
            "self.js",
            "import * as ns from './self.js';\n\
             export var mutable = 23;\n\
             var renamed = 45;\n\
             export { renamed as other };\n\
             export default 444;\n\
             console.log(ns.mutable, ns.other, ns.default);\n\
             mutable = 24;\n\
             console.log(ns.mutable);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["23 45 444", "24"]);
    }

    #[test]
    fn a_self_imported_namespace_refuses_a_binding_that_has_not_run() {
        let project = TempProject::new();
        let entry = project.write(
            "self.js",
            "import * as ns from './self.js';\n\
             try { ns.later } catch (error) { console.log(error.constructor.name); }\n\
             export const later = 1;\n\
             console.log(ns.later);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        // `undefined` here would be the one answer worse than the refusal this replaced.
        assert_eq!(host.output, ["ReferenceError", "1"]);
    }

    #[test]
    fn a_named_import_from_the_same_module_is_a_live_view_of_the_export() {
        let project = TempProject::new();
        let entry = project.write(
            "self.js",
            "import { counter as seen } from './self.js';\n\
             export let counter = 1;\n\
             console.log(seen);\n\
             counter = 2;\n\
             console.log(seen);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        // A snapshot would print `1` twice. node prints this.
        assert_eq!(host.output, ["1", "2"]);
    }

    #[test]
    fn default_scalar_import_links_to_default_export() {
        let project = TempProject::new();
        project.write("dep.js", "const value = 42; export { value as default };");
        let entry = project.write(
            "main.js",
            "import answer from './dep.js'; console.log(answer);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["42"]);
    }

    #[test]
    fn side_effect_only_modules_run_in_source_order() {
        let project = TempProject::new();
        let dep = project.write("dep.js", "console.log('dependency');");
        let entry = project.write("main.js", "import './dep.js'; console.log('entry');");
        let graph = project.graph(&entry).expect("build graph");
        assert_eq!(
            graph.module_order(),
            &[
                fs::canonicalize(dep).unwrap(),
                fs::canonicalize(&entry).unwrap()
            ]
        );
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["dependency", "entry"]);
    }

    #[test]
    fn body_patch_preserves_linkage_and_rejects_interface_changes() {
        let project = TempProject::new();
        let dependency = project.write(
            "dependency.js",
            "console.log('old'); export const answer = 42;",
        );
        let entry = project.write(
            "entry.js",
            "import { answer } from './dependency.js'; console.log(answer);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let dependency = fs::canonicalize(dependency).expect("canonical dependency");
        let order = graph.module_order().to_vec();
        let resolved = graph
            .resolved_export(&dependency, "answer")
            .cloned()
            .expect("answer is linked");
        let program = compile_module(&frontend_oxc::parse(
            dependency.to_str().expect("utf8 dependency"),
            "console.log('new'); export const answer = 7;",
        ))
        .expect("compile body edit");

        let patched = graph
            .patch_module(&dependency, program)
            .expect("body-only edit can reuse graph topology");
        assert!(Arc::ptr_eq(&graph.modules, &patched.modules));
        assert!(Arc::ptr_eq(
            &graph.export_resolutions,
            &patched.export_resolutions
        ));
        assert_eq!(
            graph.module(&entry).unwrap() as *const ModuleNode,
            patched.module(&entry).unwrap() as *const ModuleNode
        );
        assert_ne!(
            graph.module(&dependency).unwrap() as *const ModuleNode,
            patched.module(&dependency).unwrap() as *const ModuleNode
        );
        assert_eq!(patched.module_order(), order);
        assert_eq!(
            patched.resolved_export(&dependency, "answer"),
            Some(&resolved)
        );
        assert_eq!(
            patched.module(&dependency).unwrap().dependencies,
            graph.module(&dependency).unwrap().dependencies
        );
        let mut host = TestHost::default();
        patched
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute patched graph");
        assert_eq!(host.output, ["new", "7"]);

        let changed_export = compile_module(&frontend_oxc::parse(
            dependency.to_str().expect("utf8 dependency"),
            "console.log('new'); export const other = 7;",
        ))
        .expect("compile interface edit");
        assert!(graph.patch_module(&dependency, changed_export).is_none());

        let changed_import = compile_module(&frontend_oxc::parse(
            dependency.to_str().expect("utf8 dependency"),
            "import './other.js'; export const answer = 7;",
        ))
        .expect("compile import edit");
        assert!(graph.patch_module(&dependency, changed_import).is_none());
    }

    #[test]
    fn sequential_body_patches_retain_each_previous_overlay() {
        let project = TempProject::new();
        let first = project.write(
            "first.js",
            "console.log('first old'); export const first = 1;",
        );
        let second = project.write(
            "second.js",
            "console.log('second old'); export const second = 2;",
        );
        let entry = project.write(
            "entry.js",
            "import { first } from './first.js'; import { second } from './second.js'; console.log(first + second);",
        );
        let graph = project.graph(&entry).expect("build graph");
        let first = fs::canonicalize(first).expect("canonical first");
        let second = fs::canonicalize(second).expect("canonical second");
        let first_program = compile_module(&frontend_oxc::parse(
            first.to_str().expect("utf8 first"),
            "console.log('first new'); export const first = 10;",
        ))
        .expect("compile first body edit");
        let second_program = compile_module(&frontend_oxc::parse(
            second.to_str().expect("utf8 second"),
            "console.log('second new'); export const second = 20;",
        ))
        .expect("compile second body edit");

        let patched_first = graph
            .patch_module(&first, first_program)
            .expect("first body edit can be patched");
        let patched_both = patched_first
            .patch_module(&second, second_program)
            .expect("second body edit can be patched");
        let mut host = TestHost::default();
        patched_both
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute both patched modules");
        assert_eq!(host.output, ["first new", "second new", "30"]);

        let mut original_host = TestHost::default();
        graph
            .execute(&mut original_host, ExecutionLimits::default())
            .expect("execute original graph");
        assert_eq!(original_host.output, ["first old", "second old", "3"]);
    }

    #[test]
    fn missing_export_is_reported_at_link_time() {
        let project = TempProject::new();
        project.write("dep.js", "export const value = 42;");
        let entry = project.write(
            "main.js",
            "import { missing } from './dep.js'; console.log(missing);",
        );
        assert!(matches!(
            project.graph(&entry),
            Err(ModuleGraphError::MissingExport { name, .. }) if name == "missing"
        ));
    }

    #[test]
    fn direct_and_indirect_cycles_are_structured_errors() {
        let project = TempProject::new();
        let a = project.write("a.js", "import './b.js';");
        project.write("b.js", "import './c.js';");
        project.write("c.js", "import './a.js';");
        let error = project.graph(&a).expect_err("cycle should be rejected");
        assert!(matches!(error, ModuleGraphError::Cycle { paths } if paths.len() == 4));
    }

    #[test]
    fn order_is_deterministic_and_dependencies_first() {
        let project = TempProject::new();
        let c = project.write("c.js", "console.log('c');");
        let b = project.write("b.js", "import './c.js'; console.log('b');");
        let a = project.write("a.js", "console.log('a');");
        let entry = project.write(
            "main.js",
            "import './b.js'; import './a.js'; console.log('main');",
        );
        let graph = project.graph(&entry).expect("build graph");
        assert_eq!(
            graph.module_order(),
            &[
                fs::canonicalize(c).unwrap(),
                fs::canonicalize(b).unwrap(),
                fs::canonicalize(a).unwrap(),
                fs::canonicalize(entry).unwrap(),
            ]
        );
    }

    #[test]
    fn deep_linear_graph_is_stack_safe() {
        const MODULES: usize = 4_096;
        let project = TempProject::new();
        for index in 0..MODULES {
            project.write(&format!("m{index:04}.js"), "");
        }
        let entry = project.root.join("m0000.js");
        let graph = ModuleGraph::build_with_modules(&entry, &project.root, move |path| {
            let index = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.strip_prefix('m'))
                .and_then(|index| index.parse::<usize>().ok())
                .expect("chain fixture path");
            let dependencies = (index + 1 < MODULES)
                .then(|| format!("./m{:04}.js", index + 1))
                .into_iter()
                .collect();
            ModuleProgram {
                program: Program {
                    version: BYTECODE_VERSION,
                    constants: Vec::new(),
                    code: vec![Instruction::Return],
                    local_count: 0,
                    functions: Vec::new(),
                },
                dependencies,
                imports: Vec::new(),
                exports: Vec::new(),
            }
        })
        .expect("deep graph builds without recursion");
        assert_eq!(graph.module_count(), MODULES);
        assert_eq!(
            graph.module_order().first(),
            Some(&project.root.join("m4095.js").canonicalize().unwrap())
        );
        assert_eq!(
            graph.module_order().last(),
            Some(&entry.canonicalize().unwrap())
        );
    }

    #[test]
    fn step_limit_diagnostic_reports_total_graph_steps() {
        let project = TempProject::new();
        project.write("dep.js", "");
        let entry = project.write("main.js", "");
        let graph = ModuleGraph::build_with_modules(&entry, &project.root, |path| {
            let is_entry = path.file_name().and_then(|name| name.to_str()) == Some("main.js");
            ModuleProgram {
                program: Program {
                    version: BYTECODE_VERSION,
                    constants: Vec::new(),
                    code: vec![Instruction::Return],
                    local_count: 0,
                    functions: Vec::new(),
                },
                dependencies: is_entry
                    .then(|| "./dep.js".to_owned())
                    .into_iter()
                    .collect(),
                imports: Vec::new(),
                exports: Vec::new(),
            }
        })
        .expect("build graph");
        let mut host = TestHost::default();
        assert!(matches!(
            graph.execute(
                &mut host,
                ExecutionLimits {
                    max_steps: 1,
                    ..ExecutionLimits::default()
                }
            ),
            Err(ModuleGraphError::Execution {
                error: ExecutionError::StepLimitExceeded { steps: 1, limit: 1 },
                ..
            })
        ));
    }
}
