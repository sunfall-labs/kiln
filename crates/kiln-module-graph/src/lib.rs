//! Entry-rooted module graph construction and scalar module execution.
//!
//! This crate deliberately owns no filesystem or frontend policy. Callers provide a source
//! loader and a HIR frontend; the resolver is used only to apply the explicit, canonical
//! relative/absolute filesystem policy shared by the rest of Kiln.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use kiln_hir::{Module, commonjs_literal_requires};
use kiln_host_abi::HostOps;
use kiln_interpreter::{
    Cell, CommonJsRequire, ExecutionError, ExecutionLimits, ExecutionResult, ModuleRealmSession,
    Value,
    execute_module_in_runtime, execute_node_module_in_session, node_builtin_export_names,
    node_builtin_exports, ordinary_object, own_data_property, read_through_cell,
};
use kiln_ir::{
    BYTECODE_VERSION, CompileDiagnostic, ExportBinding, Instruction, ModuleProgram, Program,
    compile_commonjs, compile_module,
};
use kiln_module_resolver::{
    ModuleResolver, NodeBuiltin, NodeModuleFormat, PathRole, ResolutionKind, ResolutionPolicy,
    ResolvedModule, SUPPORTED_EXTENSIONS,
};
use kiln_runtime::RealmRuntime;

const COMMONJS_WRAPPER_SOURCE: &str = "*commonjs-wrapper*";

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

    /// Parse a CommonJS script. Frontends without a distinct script goal may use the module
    /// parser; Node-aware production frontends override this method.
    fn parse_commonjs(&mut self, path: &str, source: &str) -> Module {
        self.parse(path, source)
    }
}

impl<F> Frontend for F
where
    F: FnMut(&str, &str) -> Module,
{
    fn parse(&mut self, path: &str, source: &str) -> Module {
        self(path, source)
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
    pub dependencies: Vec<Dependency>,
}

/// The provider responsible for one graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleSource {
    Source,
    CommonJs,
    NodeBuiltin(NodeBuiltin),
}

/// An acyclic, entry-rooted module graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleGraph {
    entry: PathBuf,
    project_root: PathBuf,
    modules: BTreeMap<PathBuf, ModuleNode>,
    /// Dependencies-first, deterministic (source-occurrence ordered) module order.
    order: Vec<PathBuf>,
    /// Link-time export identity for every visible, non-missing name.
    export_resolutions: HashMap<PathBuf, BTreeMap<String, ResolvedExport>>,
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
        P: FnMut(&Path, ModuleSource) -> Result<ModuleProgram, ModuleGraphError>,
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
            let hir = if source_kind == ModuleSource::CommonJs {
                frontend.parse_commonjs(path_string, &source)
            } else {
                frontend.parse(path_string, &source)
            };
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
                program.dependencies = commonjs_literal_requires(&hir);
            }
            Ok(program)
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
            move |path, _source| provider(path),
            false,
        )
    }

    fn build_with_typed_policy_provider<P, R>(
        entry: impl AsRef<Path>,
        resolver: R,
        provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path, ModuleSource) -> Result<ModuleProgram, ModuleGraphError>,
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
        P: FnMut(&Path, ModuleSource) -> Result<ModuleProgram, ModuleGraphError>,
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
            modules: builder.modules,
            order: builder.order,
            export_resolutions,
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

    pub fn module(&self, path: impl AsRef<Path>) -> Option<&ModuleNode> {
        self.modules.get(path.as_ref()).or_else(|| {
            fs::canonicalize(path.as_ref())
                .ok()
                .and_then(|canonical| self.modules.get(&canonical))
        })
    }

    pub fn modules(&self) -> impl Iterator<Item = &ModuleNode> {
        self.order.iter().filter_map(|path| self.modules.get(path))
    }

    pub fn module_order(&self) -> &[PathBuf] {
        &self.order
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

    /// Execute every module once, dependencies first, and return the entry's value.
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

        for path in &self.order {
            let module_node = self
                .modules
                .get(path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
            if let ModuleSource::NodeBuiltin(builtin) = module_node.source {
                let exports = node_builtin_exports(builtin.specifier())
                    .expect("only materializable builtins have linkable exports");
                values.insert(path.clone(), exports.into_iter().collect());
                continue;
            }
            if module_node.source == ModuleSource::CommonJs {
                let exports_cell = commonjs_cache.get(path).ok_or_else(|| {
                    ModuleGraphError::InternalMissingModule { path: path.clone() }
                })?;
                let initial_exports = exports_cell.borrow().clone();
                let mut targets = Vec::with_capacity(module_node.dependencies.len());
                for dependency in &module_node.dependencies {
                    let value = if let Some(cell) = commonjs_cache.get(&dependency.target) {
                        std::rc::Rc::clone(cell)
                    } else if let Some(provider) = values.get(&dependency.target) {
                        let mut entries: Vec<_> = provider
                            .iter()
                            .map(|(name, value)| (name.clone(), read_through_cell(value)))
                            .collect();
                        entries.sort_by(|left, right| left.0.cmp(&right.0));
                        std::rc::Rc::new(std::cell::RefCell::new(ordinary_object(entries)))
                    } else {
                        return Err(ModuleGraphError::InternalMissingModule {
                            path: dependency.target.clone(),
                        });
                    };
                    targets.push((dependency.specifier.clone(), value));
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
                    execute_node_module_in_session(
                        &module_node.program,
                        &imports,
                        host,
                        remaining_limits,
                        &mut runtime,
                        node_session.as_mut().expect("Node mode owns a session"),
                        path == &self.entry,
                    )
                } else {
                    execute_module_in_runtime(
                        &module_node.program,
                        &imports,
                        host,
                        remaining_limits,
                        &mut runtime,
                    )
                }
                .map_err(|error| ModuleGraphError::Execution {
                    module: path.clone(),
                    error,
                })?;
                total_steps = total_steps
                    .checked_add(result.execution.steps)
                    .ok_or(ModuleGraphError::StepOverflow)?;
                let final_exports = own_data_property(&module_object, "exports")
                    .unwrap_or(Value::Undefined);
                *exports_cell.borrow_mut() = final_exports.clone();
                values.insert(
                    path.clone(),
                    HashMap::from([("default".to_owned(), final_exports)]),
                );
                if path == &self.entry {
                    entry_result = Some(result.execution);
                }
                continue;
            }
            let mut imports = Vec::with_capacity(module_node.program.imports.len());
            for import in &module_node.program.imports {
                let dependency = module_node
                    .dependencies
                    .iter()
                    .find(|dependency| dependency.specifier == import.source)
                    .ok_or_else(|| ModuleGraphError::UnresolvedImport {
                        importer: path.clone(),
                        source: import.source.clone(),
                    })?;
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
                execute_node_module_in_session(
                    &module_node.program,
                    &imports,
                    host,
                    remaining_limits,
                    &mut runtime,
                    node_session.as_mut().expect("Node mode owns a session"),
                    path == &self.entry,
                )
            } else {
                execute_module_in_runtime(
                    &module_node.program,
                    &imports,
                    host,
                    remaining_limits,
                    &mut runtime,
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
    Exit {
        path: PathBuf,
        source: ModuleSource,
        program: ModuleProgram,
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
    P: FnMut(&Path, ModuleSource) -> Result<ModuleProgram, ModuleGraphError>,
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
                        let mut cycle = self.stack[start..].to_vec();
                        cycle.push(path);
                        return Err(ModuleGraphError::Cycle { paths: cycle });
                    }
                    None => {
                        self.states.insert(path.clone(), VisitState::Visiting);
                        self.stack.push(path.clone());

                        let source = match source {
                            ModuleSource::Source if !self.typed_sources => ModuleSource::Source,
                            ModuleSource::Source => {
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
                            source => source,
                        };
                        let program = match source {
                            ModuleSource::Source | ModuleSource::CommonJs => {
                                (self.provider)(&path, source)?
                            }
                            ModuleSource::NodeBuiltin(builtin) => builtin_module_program(builtin),
                        };
                        validate_exports(&path, &program)?;
                        let mut dependencies = Vec::with_capacity(program.dependencies.len());
                        let mut dependency_sources = Vec::with_capacity(program.dependencies.len());
                        for specifier in &program.dependencies {
                            let resolved =
                                self.resolver.resolve(specifier, &path).map_err(|error| {
                                    ModuleGraphError::Resolve {
                                        importer: path.clone(),
                                        specifier: specifier.clone(),
                                        error,
                                    }
                                })?;
                            let source = if let Some(builtin) = resolved.builtin() {
                                ModuleSource::NodeBuiltin(builtin)
                            } else if !self.typed_sources {
                                ModuleSource::Source
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
                    dependencies,
                } => {
                    self.stack.pop();
                    self.states.insert(path.clone(), VisitState::Done);
                    self.order.push(path.clone());
                    self.modules.insert(
                        path.clone(),
                        ModuleNode {
                            path,
                            source,
                            program,
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
            if importer.source == ModuleSource::CommonJs
                && import.source == COMMONJS_WRAPPER_SOURCE
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
/// The graph builder has already rejected cycles, so every star provider has a completed table by
/// the time its importer is visited. This avoids both recursive stack growth and an arbitrary
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
                    match tables
                        .get(&dependency.target)
                        .and_then(|provider| provider.get(&import.imported))
                    {
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
                ModuleSource::CommonJs => frontend_oxc::parse_commonjs(path.to_str().unwrap(), &source),
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
            Ok(program)
        })
        .expect("build package graph");
        assert_eq!(
            graph.module(&package).unwrap().source,
            ModuleSource::CommonJs
        );
        assert_eq!(graph.module(&entry).unwrap().source, ModuleSource::Source);
    }

    struct OxcFrontend;

    impl Frontend for OxcFrontend {
        fn parse(&mut self, path: &str, source: &str) -> Module {
            frontend_oxc::parse(path, source)
        }

        fn parse_commonjs(&mut self, path: &str, source: &str) -> Module {
            frontend_oxc::parse_commonjs(path, source)
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
            let mut chunk = [0_u8; 1024];
            loop {
                let length = stream.read(&mut chunk).expect("read response");
                if length == 0 {
                    break;
                }
                response.extend_from_slice(&chunk[..length]);
                let Some(head_end) = response.windows(4).position(|window| window == b"\r\n\r\n")
                else {
                    continue;
                };
                let head = String::from_utf8_lossy(&response[..head_end]);
                let content_length = head
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("Content-Length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .expect("response has Content-Length");
                if response.len() >= head_end + 4 + content_length {
                    break;
                }
            }
            String::from_utf8(response).expect("UTF-8 response")
        });

        let mut host = ServerHost {
            port_sender: Some(port_sender),
        };
        graph
            .execute_node(&mut host, ExecutionLimits::default())
            .expect("serve live request");
        let response = client.join().expect("client thread");
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response:?}");
        assert!(
            response.contains("Content-Type: text/plain\r\n"),
            "{response:?}"
        );
        assert!(response.ends_with("GET /health"), "{response:?}");
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
        assert!(response.starts_with("HTTP/1.1 201 Created\r\n"));
        assert!(response.contains("Content-Type: text/plain\r\n"));
        assert!(response.ends_with("POST 12"));
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
        assert!(response.ends_with('1'), "{response:?}");
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
        assert!(
            response.contains("Transfer-Encoding: chunked\r\n"),
            "{response:?}"
        );
        assert!(
            response.ends_with("6\r\nhello \r\n5\r\ntokio\r\n0\r\n\r\n"),
            "{response:?}"
        );
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
