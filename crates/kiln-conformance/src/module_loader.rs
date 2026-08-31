//! The linker `import()` reaches for while a program is already running.
//!
//! The interpreter owns no parser and no filesystem (ADR 0007), so it asks for both instead --
//! the same shape ADR 0030 gives `eval`, one function pointer over owned data. This crate is
//! such a host: it resolves the specifier, builds the module graph the specifier names with the
//! same `frontend_oxc` -> `kiln_ir` path every other run in the suite takes, and hands back the
//! plan. Nothing here evaluates anything; the machine that asked is still running, and a module
//! and its importer have to share one realm.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use frontend_oxc::parse_with_goal;
use kiln_hir::SourceGoal;
use kiln_interpreter::{ModuleLoadError, ModulePlan, install_module_loader};
use kiln_module_graph::{ModuleGraph, ModuleGraphError, ResolveError};

thread_local! {
    /// The directory a bare relative specifier is resolved against.
    ///
    /// A compiled program does not carry the path it came from, so the referrer cannot travel
    /// with the request. Every Test262 fixture is a sibling of the test that imports it -- the
    /// corpus never reaches above its own directory -- so the test's directory is the referrer
    /// for the whole graph, and recording it here is exact rather than approximate *for this
    /// corpus*. A host whose modules import across directories needs the referrer itself.
    static BASE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Give this thread's VMs a module loader rooted at `base`.
///
/// Called at the top of every run rather than once at startup, for the same reason the source
/// compiler is: a conformance run creates a VM per case, and the base directory is a property of
/// the case rather than of the process.
pub(crate) fn install(base: &Path) {
    BASE.with(|slot| *slot.borrow_mut() = Some(base.to_path_buf()));
    install_module_loader(load);
}

fn load(specifier: &str) -> Result<ModulePlan, ModuleLoadError> {
    let Some(base) = BASE.with(|slot| slot.borrow().clone()) else {
        return Err(ModuleLoadError::Unsupported(
            "no directory was recorded to resolve `import()` against".to_owned(),
        ));
    };
    // A specifier this runner cannot host is not a specifier the test got wrong: a bare name is
    // a package, and packages are the resolver's business rather than this corpus's. Rejecting
    // is right either way -- there is no such module here -- and the message says which it was.
    let entry = base.join(specifier);
    let entry = entry.canonicalize().map_err(|error| {
        ModuleLoadError::NotFound(format!("could not resolve module `{specifier}`: {error}"))
    })?;

    let graph = ModuleGraph::build_with_module_provider(&entry, &base, compile_module_at)
        .map_err(classify)?;
    graph.import_plan().map_err(classify)
}

/// Compile one module straight from disk, with no harness and no strictness directive.
///
/// A dynamically imported module is not a test: it carries no frontmatter, and it is always a
/// module, because only a module can be imported.
fn compile_module_at(path: &Path) -> Result<kiln_ir::ModuleProgram, ModuleGraphError> {
    let source = std::fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
        path: path.to_path_buf(),
        message: format!("could not read module {}: {error}", path.display()),
    })?;
    let Some(name) = path.to_str() else {
        return Err(ModuleGraphError::FrontendPath {
            path: path.to_path_buf(),
        });
    };
    let hir = parse_with_goal(name, &source, SourceGoal::Module);
    if hir.panicked || !hir.diagnostics.is_empty() {
        let detail = hir.diagnostics.first().map_or(
            "frontend parser could not recover a program",
            |diagnostic| diagnostic.message.as_str(),
        );
        return Err(ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: format!("{} is not a valid module: {detail}", path.display()),
        });
    }
    kiln_ir::compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
        path: path.to_path_buf(),
        diagnostics,
    })
}

/// Read one graph refusal as something to tell the importing program, or not to.
///
/// The distinction is the whole point of the three error kinds: a specifier that names nothing
/// and a module that is not a module are answers, and the program is entitled to catch them. A
/// module Kiln cannot compile is not an answer -- a rejection would tell the program the module
/// is broken, and a program that believes that takes the branch it wrote for that instead.
fn classify(error: ModuleGraphError) -> ModuleLoadError {
    let message = error.to_string();
    match &error {
        ModuleGraphError::Resolve { .. }
        | ModuleGraphError::Entry { .. }
        | ModuleGraphError::NonAbsoluteEntry { .. } => ModuleLoadError::NotFound(message),
        // A missing or ambiguous export, a duplicated one, a specifier naming no module: the
        // specification calls each of these a SyntaxError raised while linking.
        ModuleGraphError::MissingExport { .. }
        | ModuleGraphError::AmbiguousExport { .. }
        | ModuleGraphError::DuplicateExport { .. }
        | ModuleGraphError::UnresolvedImport { .. } => ModuleLoadError::Syntax(message),
        ModuleGraphError::Source { message, .. } if message.contains("is not a valid module") => {
            ModuleLoadError::Syntax(message.clone())
        }
        ModuleGraphError::Compile { diagnostics, .. }
            if diagnostics.iter().all(|diagnostic| {
                crate::test262::is_intentionally_unsupported(&diagnostic.message)
            }) =>
        {
            ModuleLoadError::Unsupported(message)
        }
        // A cycle is legal JavaScript this linker does not accept, a JSON or CommonJS module is
        // a shape `import()` cannot yet materialise, and a compiler rejection this runner cannot
        // attribute to the specification could be a bug of its own. None of the three says
        // anything about the test, so none of them becomes a rejection.
        _ => ModuleLoadError::Unsupported(message),
    }
}

/// Kept for the resolver's error type, which [`classify`] reads through the graph's.
const _: Option<ResolveError> = None;
