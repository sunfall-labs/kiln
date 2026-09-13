//! The linker `import()` reaches for while a program is already running.
//!
//! The interpreter owns no parser and no filesystem (ADR 0007), so it asks for both instead --
//! the same shape ADR 0030 gives `eval`, one function pointer over owned data. This crate is
//! such a host: it resolves the specifier, builds the module graph the specifier names with the
//! same `frontend_oxc` -> `kiln_ir` path every other run in the suite takes, and hands back the
//! plan. Nothing here evaluates anything; the machine that asked is still running, and a module
//! and its importer have to share one realm.

use std::path::{Path, PathBuf};

use frontend_oxc::parse_with_goal;
use kiln_hir::SourceGoal;
use kiln_interpreter::{
    ImportAttributes, ModuleLoadError, ModuleLoadRequest, ModuleLoaderService, ModulePlan,
};
use kiln_module_graph::{ModuleGraph, ModuleGraphError};

pub(crate) struct ConformanceModuleLoader {
    base: Option<PathBuf>,
}

impl ConformanceModuleLoader {
    pub(crate) fn new(base: Option<PathBuf>) -> Self {
        Self { base }
    }
}

impl ModuleLoaderService for ConformanceModuleLoader {
    fn load(&self, request: ModuleLoadRequest) -> Result<ModulePlan, ModuleLoadError> {
        let base = request
            .importer
            .as_deref()
            .map(Path::new)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.base.clone())
            .ok_or_else(|| {
                ModuleLoadError::NotFound("dynamic import request has no importer".to_owned())
            })?;
        load_from(&base, &request.specifier, request.attributes.as_ref())
    }
}

fn load_from(
    base: &Path,
    specifier: &str,
    attributes: Option<&ImportAttributes>,
) -> Result<ModulePlan, ModuleLoadError> {
    let entry = base.join(specifier).canonicalize().map_err(|error| {
        ModuleLoadError::NotFound(format!("could not resolve module `{specifier}`: {error}"))
    })?;
    let graph = ModuleGraph::build_with_module_provider(&entry, base, compile_module_at)
        .map_err(classify)?;
    graph
        .import_plan_for_request(&entry, attributes)
        .map_err(classify)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn loader_instances_keep_explicit_bases_and_importers_isolated() {
        let root = std::env::temp_dir().join(format!(
            "kiln-conformance-loader-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(first.join("nested")).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("dep.mjs"), "export default 'first';").unwrap();
        fs::write(first.join("nested/dep.mjs"), "export default 'nested';").unwrap();
        fs::write(second.join("dep.mjs"), "export default 'second';").unwrap();

        let first_loader = ConformanceModuleLoader::new(Some(first.clone()));
        let second_loader = ConformanceModuleLoader::new(Some(second.clone()));
        let first_plan = first_loader
            .load(ModuleLoadRequest {
                importer: None,
                specifier: "./dep.mjs".into(),
                attributes: None,
            })
            .unwrap_or_else(|_| panic!("first loader request failed"));
        let second_plan = second_loader
            .load(ModuleLoadRequest {
                importer: None,
                specifier: "./dep.mjs".into(),
                attributes: None,
            })
            .unwrap_or_else(|_| panic!("second loader request failed"));
        assert!(first_plan.key.ends_with("/first/dep.mjs"));
        assert!(second_plan.key.ends_with("/second/dep.mjs"));

        let nested_importer = first.join("nested/main.mjs");
        let nested_plan = first_loader
            .load(ModuleLoadRequest {
                importer: Some(nested_importer.to_string_lossy().into_owned()),
                specifier: "./dep.mjs".into(),
                attributes: None,
            })
            .unwrap_or_else(|_| panic!("nested loader request failed"));
        assert!(nested_plan.key.ends_with("/first/nested/dep.mjs"));

        let source_error = match first_loader.load(ModuleLoadRequest {
            importer: None,
            specifier: "./dep.mjs".into(),
            attributes: Some(ImportAttributes {
                type_: Some("file".into()),
            }),
        }) {
            Err(error) => error,
            Ok(_) => panic!("source modules must reject file attributes"),
        };
        assert!(
            matches!(source_error, ModuleLoadError::Unsupported(message) if message.contains("not a file asset"))
        );
        fs::remove_dir_all(root).unwrap();
    }
}
