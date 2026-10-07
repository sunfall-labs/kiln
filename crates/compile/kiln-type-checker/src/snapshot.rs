use crate::{
    engine::{
        atom::Interner,
        check::Program,
        hir,
        json::Json,
        program::Files,
        resolve::{Host, Options, ScriptKind},
        session::{Arena, Session},
    },
    lower::Lower,
};
use kiln_hir::{Module, Span};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

/// Immutable source inputs. Libraries and package metadata must be supplied explicitly.
#[derive(Clone, Default)]
pub struct SourceSnapshot {
    pub modules: BTreeMap<String, Arc<Module>>,
    pub auxiliary_files: BTreeMap<String, Arc<[u8]>>,
}
/// Select owned facts without changing checking or diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FactSelection {
    #[default]
    All,
    FunctionContracts,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckOptions {
    pub strict: bool,
    pub no_unchecked_indexed_access: bool,
    pub libraries: Vec<String>,
    pub current_directory: String,
    pub facts: FactSelection,
    /// Optional tsconfig path in the immutable snapshot. Its compiler options replace
    /// the standalone strict preset; explicit roots remain caller-owned.
    pub config_path: Option<String>,
}
impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            strict: true,
            no_unchecked_indexed_access: true,
            libraries: Vec::new(),
            current_directory: "/".into(),
            facts: FactSelection::All,
            config_path: None,
        }
    }
}
pub use kiln_hir::source_types::{CheckResult, SourceDiagnostic, SourceType, TypeClass, TypeRole};
struct SnapshotHost<'a> {
    snapshot: &'a SourceSnapshot,
    unsupported: Mutex<Vec<SourceDiagnostic>>,
    directories: BTreeMap<String, BTreeSet<Vec<u8>>>,
}
impl<'a> SnapshotHost<'a> {
    fn new(snapshot: &'a SourceSnapshot) -> Self {
        let mut directories: BTreeMap<String, BTreeSet<Vec<u8>>> = BTreeMap::new();
        for path in snapshot
            .modules
            .keys()
            .chain(snapshot.auxiliary_files.keys())
        {
            let mut remaining = path.as_str();
            while let Some((parent, name)) = remaining.rsplit_once('/') {
                let directory = if parent.is_empty() { "/" } else { parent };
                directories
                    .entry(directory.into())
                    .or_default()
                    .insert(name.as_bytes().to_vec());
                if parent.is_empty() {
                    break;
                }
                remaining = parent;
            }
        }
        Self {
            snapshot,
            unsupported: Mutex::new(Vec::new()),
            directories,
        }
    }
}
impl Host for SnapshotHost<'_> {
    fn read(&self, path: &[u8]) -> Option<Cow<'static, [u8]>> {
        let path = std::str::from_utf8(path).ok()?;
        self.snapshot
            .modules
            .get(path)
            .map(|module| {
                let source = module
                    .type_syntax
                    .original_module
                    .as_deref()
                    .unwrap_or(module);
                Cow::Owned(source.source.as_bytes().to_vec())
            })
            .or_else(|| {
                self.snapshot
                    .auxiliary_files
                    .get(path)
                    .map(|t| Cow::Owned(t.to_vec()))
            })
    }
    fn is_file(&self, path: &[u8]) -> bool {
        std::str::from_utf8(path).is_ok_and(|p| {
            self.snapshot.modules.contains_key(p) || self.snapshot.auxiliary_files.contains_key(p)
        })
    }
    fn is_dir(&self, path: &[u8]) -> bool {
        let Ok(path) = std::str::from_utf8(path) else {
            return false;
        };
        let directory = if path == "/" {
            path
        } else {
            path.trim_end_matches('/')
        };
        self.directories.contains_key(directory)
    }

    fn realpath(&self, path: &[u8]) -> Vec<u8> {
        path.to_vec()
    }
    fn list_dir(&self, path: &[u8]) -> Vec<Vec<u8>> {
        let Ok(path) = std::str::from_utf8(path) else {
            return Vec::new();
        };
        let directory = if path == "/" {
            path
        } else {
            path.trim_end_matches('/')
        };
        self.directories
            .get(directory)
            .map_or_else(Vec::new, |children| children.iter().cloned().collect())
    }

    fn script_kind(&self, _: &[u8]) -> Option<ScriptKind> {
        None
    }
    fn extra_file_extensions(&self) -> &[(Vec<u8>, ScriptKind)] {
        &[]
    }
    fn scripts_of_page(&self, _: &[u8]) -> Vec<Vec<u8>> {
        Vec::new()
    }
    fn parse<'s>(
        &self,
        arena: &'s Arena,
        path: &[u8],
        text: &[u8],
        atoms: &Interner<'s>,
        _: &Options,
    ) -> hir::File<'s> {
        let name = String::from_utf8_lossy(path).into_owned();
        if name.ends_with(".json") || name.ends_with(".jsonc") {
            return crate::support::json::parse_source_file(arena, atoms.session(), atoms, text);
        }
        if let Some(module) = self.snapshot.modules.get(&name) {
            let source_module = module
                .type_syntax
                .original_module
                .as_deref()
                .unwrap_or(module);
            // Owned parser diagnostics remain authoritative. Checking a recovered AST
            // must neither erase them nor publish specialization facts as complete.
            if source_module.panicked || !source_module.diagnostics.is_empty() {
                let mut errors = self.unsupported.lock().unwrap();
                errors.extend(source_module.diagnostics.iter().map(|diagnostic| {
                    SourceDiagnostic {
                        path: Some(name.clone()),
                        span: diagnostic.span,
                        code: None,
                        message: diagnostic.message.clone(),
                    }
                }));
                if source_module.panicked && source_module.diagnostics.is_empty() {
                    errors.push(SourceDiagnostic {
                        path: Some(name.clone()),
                        span: None,
                        code: None,
                        message: "source parser could not recover a complete module".into(),
                    });
                }
            }
            // The loader's arena belongs to this session. Its lifetime is narrowed to the parse call.
            let (mut file, unsupported) =
                Lower::new(source_module, atoms).finish(arena, atoms.session());
            if name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts") {
                file.kind = hir::FileKind::Declaration;
            }
            self.unsupported
                .lock()
                .unwrap()
                .extend(
                    unsupported
                        .into_iter()
                        .map(|(span, message)| SourceDiagnostic {
                            path: Some(name.clone()),
                            span: Some(span),
                            code: None,
                            message: format!("checker adapter does not cover {message}"),
                        }),
                );
            file
        } else {
            self.unsupported.lock().unwrap().push(SourceDiagnostic {
                path: Some(name),
                span: None,
                code: None,
                message: "checker source is missing from the owned snapshot".into(),
            });
            // No source facts are accepted from an incomplete result.
            let mut builder = hir::FileBuilder::default();
            builder.has_errors = true;
            builder.into_arena(arena, atoms.session()).0
        }
    }
    fn parse_package_json(&self, _: &Arena, text: &[u8]) -> Option<Json> {
        crate::support::json::parse(text)
    }
    fn parallel(&self, count: usize, work: &(dyn Fn(usize) + Sync)) {
        for i in 0..count {
            work(i)
        }
    }
}
/// Check one immutable project. No filesystem reads or parser/checker identities escape.
pub fn check(snapshot: &SourceSnapshot, roots: &[String], options: &CheckOptions) -> CheckResult {
    if let Some(path) = snapshot
        .modules
        .keys()
        .find(|path| snapshot.auxiliary_files.contains_key(*path))
    {
        return CheckResult {
            checker_identity: crate::CHECKER_VERSION.into(),
            complete: false,
            diagnostics: vec![SourceDiagnostic {
                path: Some(path.clone()),
                span: None,
                code: None,
                message: "source snapshot contains colliding module and auxiliary paths".into(),
            }],
            ..Default::default()
        };
    }
    // Resolver ancestor walks require absolute lexical paths. Virtual callers may
    // supply relative names; resolve them against the explicit snapshot directory,
    // then translate results back without consulting the process's filesystem.
    let needs_normalization = |path: &str| {
        !path.starts_with('/') || path.split('/').any(|part| matches!(part, "." | ".."))
    };
    if options
        .config_path
        .as_ref()
        .is_some_and(|path| needs_normalization(path))
        || needs_normalization(&options.current_directory)
        || snapshot
            .modules
            .keys()
            .chain(snapshot.auxiliary_files.keys())
            .any(|p| needs_normalization(p))
        || roots.iter().any(|p| needs_normalization(p))
    {
        let directory = lexical_path(&options.current_directory, "/");
        let mut normalized = SourceSnapshot::default();
        let mut original_paths = BTreeMap::new();
        for (path, module) in &snapshot.modules {
            let normalized_path = lexical_path(path, &directory);
            if normalized
                .modules
                .insert(normalized_path.clone(), module.clone())
                .is_some()
            {
                return CheckResult {
                    checker_identity: crate::CHECKER_VERSION.into(),
                    complete: false,
                    diagnostics: vec![SourceDiagnostic {
                        path: Some(path.clone()),
                        span: None,
                        code: None,
                        message: "source snapshot contains colliding normalized paths".into(),
                    }],
                    ..Default::default()
                };
            }
            original_paths.insert(normalized_path, path.clone());
        }
        for (path, bytes) in &snapshot.auxiliary_files {
            let normalized_path = lexical_path(path, &directory);
            if normalized
                .auxiliary_files
                .insert(normalized_path.clone(), bytes.clone())
                .is_some()
                || normalized.modules.contains_key(&normalized_path)
            {
                return CheckResult {
                    checker_identity: crate::CHECKER_VERSION.into(),
                    complete: false,
                    diagnostics: vec![SourceDiagnostic {
                        path: Some(path.clone()),
                        span: None,
                        code: None,
                        message: "source snapshot contains colliding normalized paths".into(),
                    }],
                    ..Default::default()
                };
            }
            original_paths.insert(normalized_path, path.clone());
        }
        let roots = roots
            .iter()
            .map(|p| lexical_path(p, &directory))
            .collect::<Vec<_>>();
        let mut options = options.clone();
        options.config_path = options
            .config_path
            .as_ref()
            .map(|path| lexical_path(path, &directory));
        options.current_directory = directory;
        let mut checked = check_with_stack(&normalized, &roots, &options);
        for ty in &mut checked.types {
            if let Some(path) = original_paths.get(&ty.path) {
                ty.path.clone_from(path);
            }
        }
        for diagnostic in &mut checked.diagnostics {
            if let Some(path) = diagnostic.path.as_mut()
                && let Some(original) = original_paths.get(path)
            {
                path.clone_from(original);
            }
        }
        return checked;
    }
    check_with_stack(snapshot, roots, options)
}

fn lexical_path(path: &str, directory: &str) -> String {
    let joined = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{directory}/{path}")
    };
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}

fn check_with_stack(
    snapshot: &SourceSnapshot,
    roots: &[String],
    options: &CheckOptions,
) -> CheckResult {
    if crate::support::core::StackCheck::init().remaining() < (4 << 20) {
        return std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("kiln-source-check".into())
                .stack_size(8 << 20)
                .spawn_scoped(scope, || check_inner(snapshot, roots, options))
                .expect("source checker thread creation failed")
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        });
    }
    check_inner(snapshot, roots, options)
}
fn check_inner(snapshot: &SourceSnapshot, roots: &[String], options: &CheckOptions) -> CheckResult {
    let session = Session::new();
    let host = SnapshotHost::new(snapshot);
    let mut config_errors = Vec::new();
    let mut opts = if let Some(path) = &options.config_path {
        let project =
            crate::engine::config::load_overriding(&host, &session, path.as_bytes(), &|_| {
                Vec::new()
            });
        config_errors = project.errors;
        project.options
    } else {
        let mut opts = Options::default();
        opts.base_dir = options.current_directory.as_bytes().to_vec();
        opts.strict_null_checks = options.strict;
        opts.strict_function_types = options.strict;
        opts.no_implicit_any = options.strict;
        opts.strict_bind_call_apply = options.strict;
        opts.strict_property_initialization = options.strict;
        opts.use_unknown_in_catch_variables = options.strict;
        opts.no_implicit_this = options.strict;
        opts.strict_builtin_iterator_return = options.strict;
        opts.no_unchecked_indexed_access = options.no_unchecked_indexed_access;
        opts.allow_importing_ts_extensions = true;
        opts.target = crate::engine::resolve::ScriptTarget::ESNext;
        opts.module = crate::engine::resolve::ModuleKind::Preserve;
        // Preserve uses bundler resolution, including package exports/imports.
        // Options::default alone does not derive the module-dependent flags.
        opts.resolve_package_json_exports = true;
        opts.resolve_package_json_imports = true;
        opts.resolve_json_module = true;
        opts.skip_lib_check = true;
        opts.skip_default_lib_check = true;
        opts.no_lib = options.libraries.is_empty();
        opts.libs = options
            .libraries
            .iter()
            .map(|path| path.as_bytes().to_vec())
            .collect();
        opts
    };
    opts.current_directory = options.current_directory.as_bytes().to_vec();
    opts.no_emit = true;
    opts.lib_dir = b"/__kiln_typescript_lib".to_vec();
    let roots: Vec<_> = roots.iter().map(|p| p.as_bytes().to_vec()).collect();
    let files = session
        .arena()
        .alloc(Files::load(&session, &host, opts, &roots));
    let program = Program::new(&session, files);
    let mut checker = program.checker();
    checker.begin_stack_budget();
    checker.begin_task(0, 0, true);
    let mut checked_files = Vec::new();
    let mut ran_out = false;
    let mut result = CheckResult {
        checker_identity: format!("{}:{options:?}", crate::CHECKER_VERSION),
        complete: true,
        ..Default::default()
    };
    let configuration_valid = config_errors.is_empty();
    for error in config_errors {
        let mut message = Vec::new();
        if let Some((_, template)) = crate::engine::messages::message(error.code) {
            crate::engine::messages::format(&mut message, template, &error.args);
        }
        let (path, span) =
            error
                .at
                .map_or((options.config_path.clone(), None), |(path, start, end)| {
                    (
                        Some(String::from_utf8_lossy(&path).into_owned()),
                        Some(Span::new(start, end)),
                    )
                });
        result.diagnostics.push(SourceDiagnostic {
            path,
            span,
            code: Some(error.code),
            message: String::from_utf8_lossy(&message).into_owned(),
        });
    }
    for &id in files.order {
        let checked = checker.check_file(id);
        let module = files.module(id);
        let path = String::from_utf8_lossy(module.file_name()).into_owned();
        checked_files.push((id, path.clone(), checked));
        ran_out |= checker.take_ran_out_of_stack();
        if path.starts_with("/__kiln_typescript_lib/") {
            continue;
        }
        if options.facts == FactSelection::All {
            for (index, e) in module.hir.exprs.iter().enumerate() {
                let ty = checker.type_of_expr(id, hir::ExprId(index as u32));
                result.types.push(export_type(
                    &mut checker,
                    &path,
                    Span::new(e.pos, e.end),
                    TypeRole::Expression,
                    ty,
                ));
            }
        }
        for (index, p) in module.hir.params.iter().enumerate() {
            let ty = checker.type_of_param(id, hir::ParamId(index as u32));
            result.types.push(export_type(
                &mut checker,
                &path,
                Span::new(p.loc.pos, p.loc.end),
                TypeRole::Parameter,
                ty,
            ));
        }
        for (index, f) in module.hir.fns.iter().enumerate() {
            let ty = checker.return_type_of_fn(id, hir::FnId(index as u32));
            result.types.push(export_type(
                &mut checker,
                &path,
                Span::new(
                    f.start,
                    if f.ret.is_some() {
                        module.hir[f.ret].end
                    } else {
                        f.start
                    },
                ),
                TypeRole::Return,
                ty,
            ));
        }
    }
    ran_out |= checker.take_ran_out_of_stack();
    let mut finished = vec![checker.end_task()];
    drop(checker);
    let serial = |count: usize, work: &(dyn Fn(usize) + Sync)| {
        for i in 0..count {
            work(i)
        }
    };
    program.link(&mut finished, &serial);
    program.publish(&mut finished, &serial, false);
    for (id, path, checked) in checked_files {
        for d in program.finish_file(id, checked) {
            result.diagnostics.push(SourceDiagnostic {
                path: Some(path.clone()),
                span: Some(Span::new(d.start, d.end)),
                code: Some(d.code),
                message: String::from_utf8_lossy(&d.text).into_owned(),
            });
        }
    }
    for (code, args) in program.global_errors() {
        let mut text = Vec::new();
        if let Some((_, template)) = crate::engine::messages::message(code) {
            crate::engine::messages::format(&mut text, template, &args);
        } else {
            text.extend_from_slice(format!("Unknown TypeScript diagnostic TS{code}").as_bytes());
        }
        result.diagnostics.push(SourceDiagnostic {
            path: None,
            span: None,
            code: Some(code),
            message: String::from_utf8_lossy(&text).into_owned(),
        });
    }
    let unsupported = host.unsupported.into_inner().unwrap();
    result.complete = configuration_valid && unsupported.is_empty() && !ran_out;
    result.diagnostics.extend(unsupported);
    if ran_out {
        result.diagnostics.push(SourceDiagnostic {
            path: None,
            span: None,
            code: None,
            message: "source checker exhausted its stack budget".into(),
        });
    }
    result
        .types
        .sort_by(|a, b| (&a.path, a.span).cmp(&(&b.path, b.span)));
    result.diagnostics.sort_by(|a, b| {
        (&a.path, a.span, a.code, &a.message).cmp(&(&b.path, b.span, b.code, &b.message))
    });
    result.diagnostics.dedup();
    result
}

fn export_type(
    checker: &mut crate::engine::check::Checker<'_, '_>,
    path: &str,
    span: Span,
    role: TypeRole,
    ty: crate::engine::types::TypeId,
) -> SourceType {
    let display = String::from_utf8_lossy(&checker.type_to_string(ty)).into_owned();
    let class = if checker.is_any(ty) {
        TypeClass::Any
    } else if checker.is_number_like(ty) {
        TypeClass::Number
    } else if checker.is_string_like(ty) {
        TypeClass::String
    } else if checker.is_boolean_like(ty) {
        TypeClass::Boolean
    } else if checker.is_bigint_like(ty) {
        TypeClass::BigInt
    } else if checker.is_symbol_like(ty) {
        TypeClass::Symbol
    } else if checker.is_nullish(ty) {
        TypeClass::Nullish
    } else if display == "unknown" {
        TypeClass::Unknown
    } else {
        TypeClass::Composite
    };
    SourceType {
        path: path.to_owned(),
        span,
        display,
        class,
        role,
    }
}
