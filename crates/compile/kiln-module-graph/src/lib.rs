//! Entry-rooted module graph construction and linking.
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
use kiln_ir::{
    BYTECODE_VERSION, CompileDiagnostic, ExportBinding, Instruction, ModuleProgram, Program,
    cjs_static_module_syntax_diagnostics, compile_commonjs, compile_module,
};
use kiln_module_resolver::{
    ModuleResolver, NodeBuiltin, NodeModuleFormat, NodeRequestKind, PathRole, ResolutionKind,
    ResolutionPolicy, ResolvedModule, SUPPORTED_EXTENSIONS, is_asset_extension,
    node_format_is_ambiguous,
};

pub const COMMONJS_WRAPPER_SOURCE: &str = "*commonjs-wrapper*";

/// A cache-stable identity for private names in one project graph. Files retain their absolute
/// paths everywhere else (resolution, diagnostics, and `import.meta`); only the compiler identity
/// uses the path relative to the canonical project root.
pub fn graph_module_identity(project_root: &Path, path: &Path) -> String {
    let logical_root =
        fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    path.strip_prefix(&logical_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
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
    pub source: ModuleSource,
    /// A `require.resolve` request retains a resolver answer but does not evaluate the target.
    pub resolve_only: bool,
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

/// An entry-rooted module graph. ESM and CommonJS cycles are retained for their respective
/// link-and-instantiate and partially initialized `require` semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleGraph {
    entry: PathBuf,
    project_root: PathBuf,
    /// Modules named by the project's closed-world include declaration. They are compiled and
    /// retained as possible runtime entries, but are never evaluated at program startup.
    declared_includes: Arc<BTreeSet<PathBuf>>,
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
    /// Resolver-verified public package specifiers for targets already compiled into this graph.
    /// These let runtime-created modules import a linked target without pretending to be one of
    /// the source modules whose static edges were available during graph construction.
    runtime_package_aliases: Arc<Vec<RuntimePackageAlias>>,
}

/// A package-visible spelling for a linked module, scoped to the package that owns the runtime
/// importer. This is resolution data only; the target still has to be present in this graph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuntimePackageAlias {
    pub package_root: PathBuf,
    pub specifier: String,
    pub target: PathBuf,
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

/// User-facing failures produced while constructing or linking a graph.
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
    pub fn module_map(&self) -> &BTreeMap<PathBuf, ModuleNode> {
        &self.modules
    }

    /// Return a graph that retains a builtin for a lazy loader created outside user modules.
    ///
    /// The builtin and its static builtin dependencies become available to the runtime loader,
    /// but remain lazy: the synthetic edge is a `RuntimeDependency`, never a static import. This
    /// is for trusted bootstraps such as global `fetch`, whose public facade needs to be loadable
    /// by `process.getBuiltinModule()` even when the user's source contains no `node:` import.
    pub fn with_lazy_node_builtin(
        &self,
        owner: impl AsRef<Path>,
        importer: impl AsRef<Path>,
        builtin: NodeBuiltin,
    ) -> Result<Self, ModuleGraphError> {
        let owner = owner.as_ref();
        let owner = if self.modules.contains_key(owner) {
            owner.to_path_buf()
        } else {
            fs::canonicalize(owner).map_err(|_| ModuleGraphError::InternalMissingModule {
                path: owner.to_path_buf(),
            })?
        };
        let importer = importer.as_ref().to_path_buf();
        let target = builtin.virtual_path(&self.project_root);
        if self.module_node(&target).is_some() {
            let owner_node = self.module_node(&owner).ok_or_else(|| {
                ModuleGraphError::InternalMissingModule {
                    path: owner.clone(),
                }
            })?;
            if owner_node.runtime_dependencies.iter().any(|dependency| {
                dependency.importer == importer
                    && dependency.specifier == builtin.specifier()
                    && dependency.target == target
            }) {
                return Ok(self.clone());
            }

            // The linked builtin closure is already validated in this graph. A lazy runtime
            // edge cannot change static export resolution, so update only the owner overlay
            // instead of cloning and relinking the whole module table.
            let mut extended = self.clone();
            let mut patched_modules = (*self.patched_modules).clone();
            let mut owner_node = owner_node.clone();
            owner_node.runtime_dependencies.push(RuntimeDependency {
                importer,
                specifier: builtin.specifier().to_owned(),
                target,
                source: ModuleSource::NodeBuiltin(builtin),
                resolve_only: false,
            });
            patched_modules.insert(owner, owner_node);
            extended.patched_modules = Arc::new(patched_modules);
            return Ok(extended);
        }
        let mut modules: BTreeMap<PathBuf, ModuleNode> = self
            .order
            .iter()
            .filter_map(|path| {
                self.module_node(path)
                    .cloned()
                    .map(|node| (path.clone(), node))
            })
            .collect();
        let mut order = (*self.order).clone();
        let mut visiting = HashSet::new();

        fn add_builtin(
            project_root: &Path,
            builtin: NodeBuiltin,
            modules: &mut BTreeMap<PathBuf, ModuleNode>,
            order: &mut Vec<PathBuf>,
            visiting: &mut HashSet<PathBuf>,
        ) -> Result<PathBuf, ModuleGraphError> {
            let path = builtin.virtual_path(project_root);
            if modules.contains_key(&path) || !visiting.insert(path.clone()) {
                return Ok(path);
            }

            let source = kiln_builtins::builtin_source(builtin);
            let logical_path = kiln_builtins::builtin_logical_path(builtin);
            let mut hir = kiln_frontend_oxc::parse(logical_path, source);
            hir.module_id = Arc::from(graph_module_identity(project_root, &path));
            let program =
                compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
                    path: path.clone(),
                    diagnostics,
                })?;
            validate_exports(&path, &program)?;

            let mut dependencies = Vec::with_capacity(program.dependencies.len());
            for specifier in &program.dependencies {
                let dependency_builtin = NodeBuiltin::parse(specifier).ok_or_else(|| {
                    ModuleGraphError::Source {
                        path: path.clone(),
                        message: format!(
                            "builtin `{}` has a dependency outside the Node builtin registry: `{specifier}`",
                            builtin.specifier()
                        ),
                    }
                })?;
                let target =
                    add_builtin(project_root, dependency_builtin, modules, order, visiting)?;
                dependencies.push(Dependency {
                    specifier: specifier.clone(),
                    target,
                });
            }

            let mut runtime_dependencies = Vec::new();
            for request in &program.link_facts.node_runtime_requests {
                // These requests are themselves lazy, but their targets must be present in the
                // graph so the bootstrap's createRequire can resolve them when called. Unknown
                // names retain the builtin's existing catchable runtime-failure behavior.
                let Some(dependency_builtin) = NodeBuiltin::parse(&request.specifier) else {
                    continue;
                };
                let target =
                    add_builtin(project_root, dependency_builtin, modules, order, visiting)?;
                runtime_dependencies.push(RuntimeDependency {
                    importer: PathBuf::from(&request.importer),
                    specifier: request.specifier.clone(),
                    target,
                    source: ModuleSource::NodeBuiltin(dependency_builtin),
                    resolve_only: request.resolve_only,
                });
            }

            let node = ModuleNode {
                path: path.clone(),
                source: ModuleSource::NodeBuiltin(builtin),
                program,
                data: None,
                dependencies,
                runtime_dependencies,
            };
            modules.insert(path.clone(), node);
            visiting.remove(&path);
            order.push(path.clone());
            Ok(path)
        }

        add_builtin(
            &self.project_root,
            builtin,
            &mut modules,
            &mut order,
            &mut visiting,
        )?;
        let owner_node =
            modules
                .get_mut(&owner)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule {
                    path: owner.clone(),
                })?;
        if !owner_node.runtime_dependencies.iter().any(|dependency| {
            dependency.importer == importer
                && dependency.specifier == builtin.specifier()
                && dependency.target == target
        }) {
            owner_node.runtime_dependencies.push(RuntimeDependency {
                importer,
                specifier: builtin.specifier().to_owned(),
                target,
                source: ModuleSource::NodeBuiltin(builtin),
                resolve_only: false,
            });
        }

        let export_resolutions = resolve_exports(&modules, &order)?;
        validate_links(&modules, &order, &export_resolutions)?;
        Ok(Self {
            entry: self.entry.clone(),
            project_root: self.project_root.clone(),
            declared_includes: Arc::clone(&self.declared_includes),
            modules: Arc::new(modules),
            order: Arc::new(order),
            export_resolutions: Arc::new(export_resolutions),
            patched_modules: Arc::new(BTreeMap::new()),
            dynamic_dependencies: Arc::clone(&self.dynamic_dependencies),
            dynamic_failures: Arc::clone(&self.dynamic_failures),
            runtime_failures: Arc::clone(&self.runtime_failures),
            runtime_package_aliases: Arc::clone(&self.runtime_package_aliases),
        })
    }

    pub fn export_resolutions(&self) -> &HashMap<PathBuf, BTreeMap<String, ResolvedExport>> {
        &self.export_resolutions
    }

    /// Build a graph rooted at a particular node, retaining the immutable graph snapshot.
    /// Dynamic imports use this instead of accidentally returning the original entry plan.
    pub fn rooted_at(&self, entry: impl AsRef<Path>) -> Result<Self, ModuleGraphError> {
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
        Ok(rooted)
    }

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
        let logical_root = fs::canonicalize(project_root.as_ref())
            .unwrap_or_else(|_| project_root.as_ref().to_path_buf());
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
            let mut hir = frontend.parse(path_string, &source);
            hir.module_id = Arc::from(graph_module_identity(&logical_root, path));
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
        let logical_root = fs::canonicalize(project_root.as_ref())
            .unwrap_or_else(|_| project_root.as_ref().to_path_buf());
        let mut provider = move |path: &Path| -> Result<ModuleProgram, ModuleGraphError> {
            let mut hir = provider(path).map_err(|error| ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
            hir.module_id = Arc::from(graph_module_identity(&logical_root, path));
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

    /// Build with extra lazy roots resolved by the caller's Node package policy. The caller
    /// supplies canonical paths and source kinds so the same pinned resolution snapshot can be
    /// used by the CLI and daemon. An include is linked like a module request, but execution
    /// still begins only at `entry`.
    pub fn build_with_typed_policy_includes<P, R>(
        entry: impl AsRef<Path>,
        resolver: R,
        provider: P,
        includes: Vec<(PathBuf, ModuleSource)>,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path, ModuleSource) -> Result<ModulePayload, ModuleGraphError>,
        R: ResolutionPolicy,
    {
        Self::build_with_policy_provider_entry(entry, resolver, provider, true, false, includes)
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
        loader: L,
        frontend: F,
    ) -> Result<Self, ModuleGraphError>
    where
        L: SourceLoader,
        F: Frontend,
        R: ResolutionPolicy,
    {
        let logical_root = resolver.project_root().to_path_buf();
        Self::build_with_typed_policy(
            entry,
            resolver,
            Self::node_provider(loader, frontend, logical_root),
        )
    }

    /// Node-format graph with already-resolved declared modules that may be loaded at runtime.
    pub fn build_node_with_includes<L, F, R>(
        entry: impl AsRef<Path>,
        resolver: R,
        loader: L,
        frontend: F,
        includes: Vec<(PathBuf, ModuleSource)>,
    ) -> Result<Self, ModuleGraphError>
    where
        L: SourceLoader,
        F: Frontend,
        R: ResolutionPolicy,
    {
        let logical_root = resolver.project_root().to_path_buf();
        Self::build_with_typed_policy_includes(
            entry,
            resolver,
            Self::node_provider(loader, frontend, logical_root),
            includes,
        )
    }

    /// [`Self::build_node`] for an entry that is not a file: `source` is its text, and `entry`
    /// the path it stands at (its directory is where its relative requests resolve, its
    /// extension its module format). What a `node:worker_threads` worker made from source
    /// text runs (`{ eval: true }`); every other module is loaded as usual.
    pub fn build_node_virtual<L, F, R>(
        entry: impl AsRef<Path>,
        source: String,
        resolver: R,
        mut loader: L,
        frontend: F,
    ) -> Result<Self, ModuleGraphError>
    where
        L: SourceLoader,
        F: Frontend,
        R: ResolutionPolicy,
    {
        let entry = entry.as_ref().to_path_buf();
        let virtual_path = entry.clone();
        let loader = move |path: &Path| -> Result<String, String> {
            if path == virtual_path {
                Ok(source.clone())
            } else {
                loader.load(path).map_err(|error| error.to_string())
            }
        };
        let logical_root = resolver.project_root().to_path_buf();
        Self::build_with_policy_provider_entry(
            entry,
            resolver,
            Self::node_provider(loader, frontend, logical_root),
            true,
            true,
            Vec::new(),
        )
    }

    fn node_provider<L, F>(
        mut loader: L,
        mut frontend: F,
        logical_root: PathBuf,
    ) -> impl FnMut(&Path, ModuleSource) -> Result<ModulePayload, ModuleGraphError>
    where
        L: SourceLoader,
        F: Frontend,
    {
        move |path: &Path, source_kind: ModuleSource| {
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
            let mut commonjs = (source_kind == ModuleSource::CommonJs)
                .then(|| frontend.parse_commonjs(path_string, &source));
            let mut hir = commonjs.as_ref().map_or_else(
                || frontend.parse(path_string, &source),
                |parsed| parsed.module.clone(),
            );
            let module_id = Arc::<str>::from(graph_module_identity(&logical_root, path));
            hir.module_id = module_id.clone();
            if let Some(parsed) = &mut commonjs {
                parsed.module.module_id = module_id;
            }
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
        }
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
        Self::build_with_policy_provider_entry(
            entry,
            resolver,
            provider,
            typed_sources,
            false,
            Vec::new(),
        )
    }

    fn build_with_policy_provider_entry<P, R>(
        entry: impl AsRef<Path>,
        resolver: R,
        provider: P,
        typed_sources: bool,
        virtual_entry: bool,
        includes: Vec<(PathBuf, ModuleSource)>,
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
        let entry_path = if virtual_entry {
            entry.as_ref().to_path_buf()
        } else {
            fs::canonicalize(entry.as_ref()).map_err(|error| ModuleGraphError::Entry {
                path: entry.as_ref().to_path_buf(),
                kind: error.kind(),
            })?
        };
        let root = resolver.project_root().to_path_buf();
        if virtual_entry {
            // Nothing on disk to check: the text was handed over with the path.
        } else if !entry_path.starts_with(&root) {
            return Err(ModuleGraphError::Resolve {
                importer: entry_path,
                specifier: String::new(),
                error: ResolveError::OutsideProject {
                    path: entry.as_ref().to_path_buf(),
                    project_root: root,
                },
            });
        }
        if !virtual_entry && !entry_path.is_file() {
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
        let mut declared_includes = BTreeSet::new();
        for (path, _) in &includes {
            if !path.is_absolute() || !path.starts_with(&root) {
                return Err(ModuleGraphError::Resolve {
                    importer: entry_path.clone(),
                    specifier: path.display().to_string(),
                    error: ResolveError::OutsideProject {
                        path: path.clone(),
                        project_root: root.clone(),
                    },
                });
            }
            declared_includes.insert(path.clone());
        }
        builder.visit_with_includes(entry_path.clone(), includes)?;
        let runtime_package_aliases = builder
            .resolver
            .runtime_package_aliases_for_targets(
                &builder.modules.keys().cloned().collect::<Vec<_>>(),
            )
            .into_iter()
            .map(|(package_root, specifier, target)| RuntimePackageAlias {
                package_root,
                specifier,
                target,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let export_resolutions = resolve_exports(&builder.modules, &builder.order)?;
        validate_links(&builder.modules, &builder.order, &export_resolutions)?;
        Ok(Self {
            entry: entry_path,
            project_root: builder.resolver.project_root().to_path_buf(),
            declared_includes: Arc::new(declared_includes),
            modules: Arc::new(builder.modules),
            order: Arc::new(builder.order),
            export_resolutions: Arc::new(export_resolutions),
            patched_modules: Arc::new(BTreeMap::new()),
            dynamic_dependencies: Arc::new(builder.dynamic_dependencies),
            dynamic_failures: Arc::new(builder.dynamic_failures.into_iter().collect()),
            runtime_failures: Arc::new(builder.runtime_failures),
            runtime_package_aliases: Arc::new(runtime_package_aliases),
        })
    }

    pub fn entry(&self) -> &Path {
        &self.entry
    }

    /// This graph with one of its modules as the entry: what a `node:worker_threads` worker
    /// running that module evaluates. `None` when `path` is not a module of the graph.
    pub fn with_entry(&self, path: impl AsRef<Path>) -> Option<Self> {
        let path = path.as_ref();
        let entry = if self.module_node(path).is_some() {
            path.to_path_buf()
        } else {
            let canonical = fs::canonicalize(path).ok()?;
            self.module_node(&canonical)?;
            canonical
        };
        let mut graph = self.clone();
        graph.entry = entry;
        Some(graph)
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Package aliases verified by the resolver for modules already present in this graph.
    pub fn runtime_package_aliases(&self) -> &[RuntimePackageAlias] {
        &self.runtime_package_aliases
    }

    pub fn declared_includes(&self) -> &BTreeSet<PathBuf> {
        &self.declared_includes
    }

    /// Resolve only an absolute path or file URL that names a declared module. This is the
    /// runtime half of `kiln.include`: a computed loader cannot gain access to an arbitrary
    /// source merely because it happens to be present elsewhere in the graph.
    pub fn declared_target_for_runtime_specifier(&self, specifier: &str) -> Option<PathBuf> {
        let path = if specifier.starts_with("file:") {
            url::Url::parse(specifier).ok()?.to_file_path().ok()?
        } else {
            PathBuf::from(specifier)
        };
        (path.is_absolute() && self.declared_includes.contains(&path)).then_some(path)
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
            declared_includes: Arc::clone(&self.declared_includes),
            modules: Arc::new(modules),
            order: Arc::clone(&self.order),
            export_resolutions: Arc::new(export_resolutions),
            patched_modules: Arc::new(BTreeMap::new()),
            dynamic_dependencies: Arc::clone(&self.dynamic_dependencies),
            dynamic_failures: Arc::clone(&self.dynamic_failures),
            runtime_failures: Arc::clone(&self.runtime_failures),
            runtime_package_aliases: Arc::clone(&self.runtime_package_aliases),
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
            let original_program_dependencies = std::mem::take(&mut node.program.dependencies);
            let original_dependencies = std::mem::take(&mut node.dependencies);
            let mut rewritten_requests = BTreeMap::<String, Vec<String>>::new();
            let mut rewritten_dependencies = BTreeMap::<String, Vec<Dependency>>::new();
            node.runtime_dependencies = node
                .runtime_dependencies
                .iter()
                // A resolve-only request needs its canonical path in the reduced graph's
                // resolver metadata, but its target is deliberately not a live module.
                .filter(|dependency| {
                    dependency.resolve_only || demand.live.contains(&dependency.target)
                })
                .cloned()
                .collect();
            let mut imports = Vec::with_capacity(original_imports.len());
            for (index, original_import) in original_imports.iter().enumerate() {
                let live = demand.live_imports.contains(&(node.path.clone(), index));
                // An indirect re-export is an import into a synthetic slot
                // plus an export of that slot. The module body does not read
                // it, but a demanded export still needs the binding metadata.
                let links_export = node
                    .program
                    .exports
                    .iter()
                    .any(|export| export.local_slot == original_import.local_slot);
                if !live && original_import.source != COMMONJS_WRAPPER_SOURCE {
                    let undemanded_reexport_is_dropped = if links_export {
                        let dependency = original_dependencies
                            .iter()
                            .find(|dependency| dependency.specifier == original_import.source)
                            .ok_or_else(|| ModuleGraphError::UnresolvedImport {
                                importer: node.path.clone(),
                                source: original_import.source.clone(),
                            })?;
                        !demand.live.contains(&dependency.target)
                    } else {
                        true
                    };
                    if undemanded_reexport_is_dropped {
                        // A re-export's import entry is retained only to support a demanded
                        // export. Keeping its export row after its provider module was shaken
                        // leaves a dangling dependency that fails relinking of the reduced graph.
                        node.program
                            .exports
                            .retain(|export| export.local_slot != original_import.local_slot);
                        continue;
                    }
                }
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
                                            // CommonJS's default is module.exports, represented
                                            // virtually by slot zero rather than an ESM export row.
                                            if origin.source == ModuleSource::CommonJs
                                                && *local_slot == 0
                                            {
                                                return Some("default".to_owned());
                                            }
                                            origin
                                                .program
                                                .exports
                                                .iter()
                                                .find(|export| {
                                                    export.local_slot == *local_slot
                                                        && !export.exported.starts_with(
                                                            kiln_ir::STAR_REEXPORT_PREFIX,
                                                        )
                                                })
                                                .map(|export| export.exported.clone())
                                        })
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
                        rewritten_requests
                            .entry(original_import.source.clone())
                            .or_default()
                            .push(specifier.clone());
                        rewritten_dependencies
                            .entry(original_import.source.clone())
                            .or_default()
                            .push(Dependency { specifier, target });
                    }
                }

                let next_index = imports.len();
                if live {
                    remapped_imports.insert((node.path.clone(), next_index));
                }
                imports.push(import);
            }
            node.program.imports = imports;
            if node.source == ModuleSource::CommonJs {
                // CommonJS's dependency list also retains unresolved require literals so the
                // runtime loader can throw at the original call site. Keep its source request
                // inventory untouched; only ESM import requests are retargeted below.
                node.dependencies = original_dependencies
                    .iter()
                    .filter(|dependency| demand.live.contains(&dependency.target))
                    .cloned()
                    .collect();
                node.program.dependencies = original_program_dependencies;
            } else {
                // Retain dependency evaluation order from the original request list, replacing a
                // dropped barrel in place with its live export providers. Multiple bindings from a
                // barrel expand in import order.
                for dependency in &original_dependencies {
                    if demand.live.contains(&dependency.target) {
                        node.dependencies.push(dependency.clone());
                    } else if let Some(rewritten) =
                        rewritten_dependencies.remove(&dependency.specifier)
                    {
                        node.dependencies.extend(rewritten);
                    }
                }
                for remaining in rewritten_dependencies.into_values() {
                    node.dependencies.extend(remaining);
                }
                // Preserve source request order while retargeting requests whose barrel was
                // removed. Native async evaluation uses this list to await static dependencies;
                // leaving the original specifier here makes it disagree with the synthetic edge
                // in `node.dependencies`, so the provider can be read before it has finished.
                let retained_requests = node
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.specifier.as_str())
                    .collect::<BTreeSet<_>>();
                for request in original_program_dependencies {
                    if let Some(rewritten) = rewritten_requests.remove(&request) {
                        node.program.dependencies.extend(rewritten);
                    } else if retained_requests.contains(request.as_str()) {
                        node.program.dependencies.push(request);
                    }
                }
                // Include any edge without an original source request while retaining the ordered
                // source-derived prefix above.
                for dependency in &node.dependencies {
                    if !node.program.dependencies.contains(&dependency.specifier) {
                        node.program.dependencies.push(dependency.specifier.clone());
                    }
                }
            }
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
        let runtime_package_aliases = self
            .runtime_package_aliases
            .iter()
            .filter(|alias| demand.live.contains(&alias.target))
            .cloned()
            .collect();
        Ok((
            Self {
                entry: self.entry.clone(),
                project_root: self.project_root.clone(),
                declared_includes: Arc::new(
                    self.declared_includes
                        .iter()
                        .filter(|path| demand.live.contains(*path))
                        .cloned()
                        .collect(),
                ),
                modules: Arc::new(modules),
                order: Arc::new(order),
                export_resolutions: Arc::new(export_resolutions),
                patched_modules: Arc::new(patched_modules),
                dynamic_dependencies: Arc::new(dynamic_dependencies),
                dynamic_failures: Arc::new(dynamic_failures),
                runtime_failures: Arc::new(runtime_failures),
                runtime_package_aliases: Arc::new(runtime_package_aliases),
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
    fn visit_with_includes(
        &mut self,
        path: PathBuf,
        includes: Vec<(PathBuf, ModuleSource)>,
    ) -> Result<(), ModuleGraphError> {
        self.visit_static(path, None)?;
        for (path, source) in includes {
            self.visit_static(path, Some(source))?;
        }
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
                                let hir = kiln_frontend_oxc::parse(path_str, source);
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
                            // `getBuiltinModule` names a builtin explicitly: resolve it as one
                            // even when the resolver's `node_builtins` gate is off (the daemon
                            // only enables Node builtins when package metadata says so, but an
                            // explicit `getBuiltinModule("os")` *is* a Node program).
                            // An engine-internal module (`kiln:actors/core`) is never named by a
                            // program: it goes through the resolver, which refuses it for any
                            // importer that is not an engine builtin.
                            let resolved = if let Some(builtin) =
                                NodeBuiltin::parse(&request.specifier)
                                    .filter(|builtin| !builtin.is_engine_internal())
                            {
                                ResolvedModule::new(
                                    builtin.virtual_path(self.resolver.project_root()),
                                    ResolutionKind::Builtin,
                                )
                            } else {
                                match self.resolver.resolve_for_request(
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
                                source: runtime_source,
                                resolve_only: request.resolve_only,
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
                            if dependency.resolve_only {
                                continue;
                            }
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
            source_origin: None,
            static_source_identity: None,
            constants: vec![],
            code: vec![Instruction::Return],
            local_count: 1,
            functions: vec![],
            template_sites: vec![],
            debug_call_sites: vec![],
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
            module_instantiation_end: 0,
        },
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
