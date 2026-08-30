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

use kiln_hir::Module;
use kiln_host_abi::HostOps;
use kiln_interpreter::{ExecutionError, ExecutionLimits, ExecutionResult, Value, execute_module};
use kiln_ir::{CompileDiagnostic, ModuleProgram, compile_module};
use kiln_module_resolver::{ModuleResolver, PathRole, ResolveError, SUPPORTED_EXTENSIONS};

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
    pub program: ModuleProgram,
    pub dependencies: Vec<Dependency>,
}

/// An acyclic, entry-rooted module graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleGraph {
    entry: PathBuf,
    project_root: PathBuf,
    modules: BTreeMap<PathBuf, ModuleNode>,
    /// Dependencies-first, deterministic (source-occurrence ordered) module order.
    order: Vec<PathBuf>,
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

    fn build_with_provider<P>(
        entry: impl AsRef<Path>,
        project_root: impl AsRef<Path>,
        provider: P,
    ) -> Result<Self, ModuleGraphError>
    where
        P: FnMut(&Path) -> Result<ModuleProgram, ModuleGraphError>,
    {
        if !entry.as_ref().is_absolute() {
            return Err(ModuleGraphError::NonAbsoluteEntry {
                path: entry.as_ref().to_path_buf(),
            });
        }
        let resolver =
            ModuleResolver::new(project_root).map_err(|error| ModuleGraphError::Resolve {
                importer: PathBuf::new(),
                specifier: String::new(),
                error,
            })?;
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
            modules: BTreeMap::new(),
            states: HashMap::new(),
            stack: Vec::new(),
            order: Vec::new(),
        };
        builder.visit(entry_path.clone())?;
        let graph = Self {
            entry: entry_path,
            project_root: builder.resolver.project_root().to_path_buf(),
            modules: builder.modules,
            order: builder.order,
        };
        validate_links(&graph.modules, &graph.order)?;
        Ok(graph)
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

    /// Execute every module once, dependencies first, and return the entry's value.
    ///
    /// The returned `steps` is the total across the graph; its `value` is the entry module's
    /// result. Side-effect-only dependencies are present in `order` and are therefore evaluated.
    pub fn execute<H: HostOps + ?Sized>(
        &self,
        host: &mut H,
        limits: ExecutionLimits,
    ) -> Result<ExecutionResult, ModuleGraphError> {
        let mut values: HashMap<PathBuf, HashMap<String, Value>> = HashMap::new();
        let mut total_steps = 0_u64;
        let mut entry_result = None;

        for path in &self.order {
            let node = self
                .modules
                .get(path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule { path: path.clone() })?;
            let mut imports = Vec::with_capacity(node.program.imports.len());
            for import in &node.program.imports {
                let dependency = node
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
            let result = execute_module(&node.program, &imports, host, remaining_limits).map_err(
                |error| {
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
                },
            )?;
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
                for (name, value) in kiln_interpreter::namespace_entries(&star.value) {
                    // `export *` never forwards a default: a module's default is its own.
                    if name != "default" {
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

struct Builder<P> {
    resolver: ModuleResolver,
    provider: P,
    modules: BTreeMap<PathBuf, ModuleNode>,
    states: HashMap<PathBuf, VisitState>,
    stack: Vec<PathBuf>,
    order: Vec<PathBuf>,
}

enum WorkItem {
    Enter(PathBuf),
    Exit {
        path: PathBuf,
        program: ModuleProgram,
        dependencies: Vec<Dependency>,
    },
}

#[derive(Clone, Copy)]
enum VisitState {
    Visiting,
    Done,
}

impl<P> Builder<P>
where
    P: FnMut(&Path) -> Result<ModuleProgram, ModuleGraphError>,
{
    fn visit(&mut self, path: PathBuf) -> Result<(), ModuleGraphError> {
        let mut work = vec![WorkItem::Enter(path)];
        while let Some(item) = work.pop() {
            match item {
                WorkItem::Enter(path) => match self.states.get(&path).copied() {
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

                        let program = (self.provider)(&path)?;
                        validate_exports(&path, &program)?;
                        let mut dependencies = Vec::with_capacity(program.dependencies.len());
                        for specifier in &program.dependencies {
                            let resolved =
                                self.resolver.resolve(specifier, &path).map_err(|error| {
                                    ModuleGraphError::Resolve {
                                        importer: path.clone(),
                                        specifier: specifier.clone(),
                                        error,
                                    }
                                })?;
                            dependencies.push(Dependency {
                                specifier: specifier.clone(),
                                target: resolved.path,
                            });
                        }

                        // LIFO work preserves the original source order: the first dependency is
                        // entered first, while Exit remains below all child work.
                        work.push(WorkItem::Exit {
                            path,
                            program,
                            dependencies: dependencies.clone(),
                        });
                        for dependency in dependencies.into_iter().rev() {
                            work.push(WorkItem::Enter(dependency.target));
                        }
                    }
                },
                WorkItem::Exit {
                    path,
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
) -> Result<(), ModuleGraphError> {
    for importer_path in order {
        let importer =
            modules
                .get(importer_path)
                .ok_or_else(|| ModuleGraphError::InternalMissingModule {
                    path: importer_path.clone(),
                })?;
        for import in &importer.program.imports {
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
            if import.imported != kiln_ir::NAMESPACE_IMPORT
                && !provides_export(modules, &dependency.target, &import.imported, 0)
            {
                return Err(ModuleGraphError::MissingExport {
                    importer: importer_path.clone(),
                    provider: dependency.target.clone(),
                    name: import.imported.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Whether `module` exports `name`, following `export * from` into the modules it forwards.
///
/// A star export is one entry naming a specifier, not a list of names, so this is where the two
/// export tables meet. The graph is acyclic, but `depth` bounds the walk anyway: this runs before
/// execution, and a link check that can hang is worse than one that gives up.
fn provides_export(
    modules: &BTreeMap<PathBuf, ModuleNode>,
    module: &Path,
    name: &str,
    depth: usize,
) -> bool {
    const MAX_STAR_DEPTH: usize = 64;
    let Some(node) = modules.get(module) else {
        return false;
    };
    if node
        .program
        .exports
        .iter()
        .any(|export| export.exported == name)
    {
        return true;
    }
    if depth >= MAX_STAR_DEPTH {
        return false;
    }
    node.program
        .exports
        .iter()
        .filter_map(|export| export.exported.strip_prefix(kiln_ir::STAR_REEXPORT_PREFIX))
        .any(|source| {
            node.dependencies
                .iter()
                .find(|dependency| dependency.specifier == source)
                .is_some_and(|dependency| {
                    provides_export(modules, &dependency.target, name, depth + 1)
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_host_abi::{HostError, HostOp, HostValue};
    use kiln_ir::{BYTECODE_VERSION, Instruction, Program};
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
        project.write(
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
        let mut host = TestHost::default();
        graph
            .execute(&mut host, ExecutionLimits::default())
            .expect("execute graph");
        assert_eq!(host.output, ["a wide", "alpha,shared"]);
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
