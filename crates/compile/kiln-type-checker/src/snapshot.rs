use crate::{
    engine::{
        atom::Interner,
        check::Program,
        hir,
        json::Json,
        program::{FileId, Files},
        resolve::{Host, Options, ScriptKind},
        session::{Arena, Session},
    },
    lower::Lower,
};
use kiln_hir::{Module, Span};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, OnceLock},
};

/// Immutable source inputs. Libraries and package metadata must be supplied explicitly.
#[derive(Clone, Default)]
pub struct SourceSnapshot {
    pub modules: BTreeMap<String, Arc<Module>>,
    pub auxiliary_files: BTreeMap<String, Arc<[u8]>>,
    /// Declaration libraries parsed only when a check loads them. A library path also present in
    /// `modules` is read from `modules`.
    pub libraries: Option<Arc<Libraries>>,
    /// Symbolic links among the snapshot's directories: the absolute path of a link and the
    /// absolute, link-free path it names. Package managers link packages into `node_modules`
    /// (`node_modules/effect` -> `node_modules/.bun/effect@4/node_modules/effect`), and module
    /// resolution looks them up through the link, as it would on the file system.
    pub links: BTreeMap<String, String>,
}

/// The pinned declaration libraries under the checker's library directory, each parsed by the
/// frontend the composition root supplies the first time a check loads it, then shared by every
/// later check. A program reads only the libraries its `lib` setting references, and the
/// frontend's result for a library depends on nothing but its text.
pub struct Libraries {
    entries: BTreeMap<String, Library>,
    parse: fn(&str, &str) -> Module,
}

struct Library {
    source: &'static str,
    parsed: OnceLock<Arc<Module>>,
}

impl Libraries {
    /// [`crate::STANDARD_LIBRARIES`] (TypeScript's own), [`crate::KILN_LIBRARIES`] and
    /// [`crate::NODE_LIBRARIES`] under `/__kiln_typescript_lib`. Only the first are `lib` names;
    /// the others are ambient declarations a caller adds as roots (see [`Libraries::path`]).
    pub fn standard(parse: fn(&str, &str) -> Module) -> Self {
        let entries = crate::STANDARD_LIBRARIES
            .iter()
            .chain(crate::KILN_LIBRARIES)
            .chain(crate::NODE_LIBRARIES)
            .map(|&(name, source)| {
                let library = Library {
                    source,
                    parsed: OnceLock::new(),
                };
                (format!("{LIBRARY_DIRECTORY}/{name}"), library)
            })
            .collect();
        Self { entries, parse }
    }

    /// The absolute path of a library file, `name` as in the tables above, for use as a check root.
    pub fn path(name: &str) -> String {
        format!("{LIBRARY_DIRECTORY}/{name}")
    }

    /// Parse `roots`, named as in a `lib` setting (`"esnext"`), and every library they reference,
    /// on up to `threads` threads. A check that loads one of them while this runs waits for that
    /// library's parse instead of starting another.
    pub fn prepare(&self, roots: &[&str], threads: usize) {
        let mut pending = roots
            .iter()
            .map(|name| format!("{LIBRARY_DIRECTORY}/lib.{name}.d.ts"))
            .collect::<Vec<_>>();
        let mut closure = BTreeSet::new();
        while let Some(path) = pending.pop() {
            let Some(library) = self.entries.get(&path) else {
                continue;
            };
            if !closure.insert(path) {
                continue;
            }
            // The pinned libraries spell every reference this way, below their license header.
            const REFERENCE: &str = "/// <reference lib=\"";
            for (at, _) in library.source.match_indices(REFERENCE) {
                let rest = &library.source[at + REFERENCE.len()..];
                if let Some((name, _)) = rest.split_once('"') {
                    pending.push(format!("{LIBRARY_DIRECTORY}/lib.{name}.d.ts"));
                }
            }
        }
        // The largest first: `lib.es5.d.ts` alone is the longest parse of the closure.
        let mut closure = closure.into_iter().collect::<Vec<_>>();
        closure.sort_by_key(|path| std::cmp::Reverse(self.entries[path].source.len()));
        let next = std::sync::atomic::AtomicUsize::new(0);
        let work = || {
            while let Some(path) =
                closure.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
            {
                self.module(path);
            }
        };
        worker_pool().in_place_scope(|scope| {
            for _ in 1..threads.max(1).min(closure.len()) {
                scope.spawn(|_| work());
            }
            work();
        });
    }

    fn module(&self, path: &str) -> Option<&Arc<Module>> {
        let library = self.entries.get(path)?;
        Some(
            library
                .parsed
                .get_or_init(|| Arc::new((self.parse)(path, library.source))),
        )
    }
}

const LIBRARY_DIRECTORY: &str = "/__kiln_typescript_lib";
/// Select owned facts without changing checking or diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FactSelection {
    #[default]
    All,
    FunctionContracts,
    /// No facts: diagnostics only, for a caller that compiles nothing it keeps.
    None,
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
pub use kiln_hir::source_types::{
    CheckResult, NativeFieldType, RecordField, RecordLayout, RecordLayoutId, SourceDiagnostic,
    SourceType, TypeClass, TypeRole,
};
struct SnapshotHost<'a> {
    snapshot: &'a SourceSnapshot,
    unsupported: Mutex<Vec<SourceDiagnostic>>,
    /// Early errors in the files parsed so far, for [`uncovered_early`].
    early: Mutex<Vec<SourceDiagnostic>>,
    directories: BTreeMap<String, BTreeSet<Vec<u8>>>,
    threads: usize,
}

/// The threads of one check: the available CPUs, bounded by any positive `KILN_JOBS` or
/// `RAYON_NUM_THREADS`. Loading, binding and resolution are the donor's parallel phases; the
/// engine publishes their results under its own task protocol, whatever the thread count.
fn check_threads() -> usize {
    kiln_jobs::budget(usize::MAX)
}

/// The stack of a checker worker thread, as for the calling thread in [`check_with_stack`].
const WORKER_STACK: usize = 8 << 20;

/// The checker's worker threads, shared by every check in the process, each with the stack the
/// checker needs. Their mimalloc heaps stay warm from one check to the next.
fn worker_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = check_threads().saturating_sub(1).max(1);
        kiln_jobs::pool("kiln-source-check", threads, Some(WORKER_STACK))
            .expect("source checker thread pool")
    })
}
impl<'a> SnapshotHost<'a> {
    fn new(snapshot: &'a SourceSnapshot) -> Self {
        let mut directories: BTreeMap<String, BTreeSet<Vec<u8>>> = BTreeMap::new();
        let libraries = snapshot.libraries.iter().flat_map(|l| l.entries.keys());
        for path in snapshot
            .modules
            .keys()
            .chain(snapshot.auxiliary_files.keys())
            .chain(libraries)
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
        for link in snapshot.links.keys() {
            if let Some((parent, name)) = link.rsplit_once('/') {
                let directory = if parent.is_empty() { "/" } else { parent };
                directories
                    .entry(directory.into())
                    .or_default()
                    .insert(name.as_bytes().to_vec());
            }
        }
        Self {
            snapshot,
            unsupported: Mutex::new(Vec::new()),
            early: Mutex::new(Vec::new()),
            directories,
            threads: check_threads(),
        }
    }
}
impl SnapshotHost<'_> {
    /// `path` with every link among its directories replaced by what it names: its `realpath`.
    fn follow<'p>(&self, path: &'p str) -> Cow<'p, str> {
        let links = &self.snapshot.links;
        if links.is_empty() {
            return Cow::Borrowed(path);
        }
        let mut path = Cow::Borrowed(path);
        // A link's target may itself contain links; each round replaces the first one.
        for _ in 0..40 {
            let mut replaced = None;
            for (at, _) in path.match_indices('/').skip(1).chain([(path.len(), "")]) {
                if let Some(target) = links.get(&path[..at]) {
                    replaced = Some(format!("{target}{}", &path[at..]));
                    break;
                }
            }
            match replaced {
                Some(next) => path = Cow::Owned(next),
                None => break,
            }
        }
        path
    }
}

impl Host for SnapshotHost<'_> {
    fn read(&self, path: &[u8]) -> Option<Cow<'static, [u8]>> {
        let path = std::str::from_utf8(path).ok()?;
        let path = &*self.follow(path);
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
            .or_else(|| {
                let library = self.snapshot.libraries.as_ref()?.entries.get(path)?;
                Some(Cow::Borrowed(library.source.as_bytes()))
            })
    }
    fn is_file(&self, path: &[u8]) -> bool {
        std::str::from_utf8(path).is_ok_and(|p| {
            let p = &*self.follow(p);
            self.snapshot.modules.contains_key(p)
                || self.snapshot.auxiliary_files.contains_key(p)
                || self
                    .snapshot
                    .libraries
                    .as_ref()
                    .is_some_and(|libraries| libraries.entries.contains_key(p))
        })
    }
    fn is_dir(&self, path: &[u8]) -> bool {
        let Ok(path) = std::str::from_utf8(path) else {
            return false;
        };
        let path = &*self.follow(path);
        let directory = if path == "/" {
            path
        } else {
            path.trim_end_matches('/')
        };
        self.directories.contains_key(directory)
    }

    fn realpath(&self, path: &[u8]) -> Vec<u8> {
        match std::str::from_utf8(path) {
            Ok(text) => self.follow(text).as_bytes().to_vec(),
            Err(_) => path.to_vec(),
        }
    }
    fn list_dir(&self, path: &[u8]) -> Vec<Vec<u8>> {
        let Ok(path) = std::str::from_utf8(path) else {
            return Vec::new();
        };
        let path = &*self.follow(path);
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
        options: &Options,
    ) -> hir::File<'s> {
        let name = String::from_utf8_lossy(path).into_owned();
        if name.ends_with(".json") || name.ends_with(".jsonc") {
            return crate::support::json::parse_source_file(arena, atoms.session(), atoms, text);
        }
        let library = || self.snapshot.libraries.as_ref()?.module(&name).cloned();
        // A source that the caller supplied as text, unparsed: parsed now that the program
        // loads it. JavaScript a TypeScript program never loads needs no parse at all.
        let unparsed = || {
            let source = self.snapshot.auxiliary_files.get(&name)?;
            let parse = self.snapshot.libraries.as_ref()?.parse;
            let text = std::str::from_utf8(source).ok()?;
            Some(Arc::new(parse(&name, text)))
        };
        let module = self
            .snapshot
            .modules
            .get(&name)
            .cloned()
            .or_else(library)
            .or_else(unparsed);
        if let Some(module) = module.as_ref() {
            let lowered = lower_module(module, &name, arena, atoms, options);
            self.unsupported.lock().unwrap().extend(lowered.unsupported);
            self.early.lock().unwrap().extend(lowered.early);
            let file = lowered.file;
            file
        } else {
            self.unsupported.lock().unwrap().push(SourceDiagnostic {
                related: Vec::new(),
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
        let threads = self.threads.min(count);
        if threads <= 1 {
            for i in 0..count {
                work(i)
            }
            return;
        }
        let next = std::sync::atomic::AtomicUsize::new(0);
        let run = || {
            loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= count {
                    break;
                }
                work(i);
            }
        };
        // The engine calls this at every barrier of every step. Spawning its threads each time
        // cost more than the work of most barriers: hundreds of thread creations per check.
        worker_pool().in_place_scope(|scope| {
            for _ in 1..threads {
                scope.spawn(|_| run());
            }
            run();
        });
    }
    fn threads(&self) -> usize {
        self.threads
    }
}
/// Check one immutable project. No filesystem reads or parser/checker identities escape.
/// The root files of the project that the configuration at `config_path` describes, in
/// TypeScript's order: its `files`, then the matches of `include` among the snapshot's paths.
/// Empty if the configuration cannot be read.
pub fn project_roots(snapshot: &SourceSnapshot, config_path: &str) -> Vec<String> {
    let session = Session::new();
    let host = SnapshotHost::new(snapshot);
    crate::engine::config::load_overriding(&host, &session, config_path.as_bytes(), &|_| Vec::new())
        .map(|project| {
            project
                .files
                .into_iter()
                .map(|path| String::from_utf8_lossy(&path).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

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
                related: Vec::new(),
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
        let mut normalized = SourceSnapshot {
            libraries: snapshot.libraries.clone(),
            links: snapshot
                .links
                .iter()
                .map(|(link, target)| {
                    (
                        lexical_path(link, &directory),
                        lexical_path(target, &directory),
                    )
                })
                .collect(),
            ..SourceSnapshot::default()
        };
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
                        related: Vec::new(),
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
                        related: Vec::new(),
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
        for (file, _) in &mut checked.files {
            if let Some(path) = original_paths.get(file) {
                file.clone_from(path);
            }
        }
        checked.files.sort();
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
                .stack_size(WORKER_STACK)
                .spawn_scoped(scope, || check_inner(snapshot, roots, options))
                .expect("source checker thread creation failed")
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        });
    }
    check_inner(snapshot, roots, options)
}
/// Each source file of the program with whether its own facts are complete: no file it reaches
/// through its references and imports has an adapter gap or ran out of stack. A gap with no file,
/// or in a file that is not a module or that augments the global scope, can change any file's
/// types, as can an invalid configuration; then no file is complete.
fn complete_files(
    files: &Files<'_>,
    sources: &[(FileId, String)],
    unsupported: &[SourceDiagnostic],
    ran_out_in: &[FileId],
    configuration_valid: bool,
) -> Vec<(String, bool)> {
    let count = files.modules.len();
    let by_path: std::collections::HashMap<&[u8], FileId> = (0..count)
        .flat_map(|index| {
            let id = FileId(index as u32);
            let module = files.module(id);
            [(module.path.text, id), (module.file_name(), id)]
        })
        .collect();
    let mut global = !configuration_valid;
    let mut gaps: Vec<FileId> = ran_out_in.to_vec();
    for diagnostic in unsupported {
        match diagnostic
            .path
            .as_deref()
            .and_then(|path| by_path.get(path.as_bytes()))
        {
            Some(&id) => gaps.push(id),
            None => global = true,
        }
    }
    for &id in &gaps {
        let module = files.module(id);
        if !module.is_module() || !module.bound.global_augmentations.is_empty() {
            global = true;
        }
    }
    // Everything that reaches a gap, by walking the edges backwards from the gaps.
    let mut importers: Vec<Vec<FileId>> = vec![Vec::new(); count];
    for index in 0..count {
        for &edge in files.module(FileId(index as u32)).edges {
            importers[edge.idx()].push(FileId(index as u32));
        }
    }
    let mut tainted = vec![false; count];
    while let Some(id) = gaps.pop() {
        if !std::mem::replace(&mut tainted[id.idx()], true) {
            gaps.extend(importers[id.idx()].iter().copied());
        }
    }
    let mut complete: Vec<(String, bool)> = sources
        .iter()
        .map(|(id, path)| (path.clone(), !global && !tainted[id.idx()]))
        .collect();
    complete.sort();
    complete
}

/// `isDeclarationFileName`: `.d.ts`, `.d.mts`, `.d.cts` and `.d.<extension>.ts`, in any case.
fn is_declaration_file_name(name: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
    file.ends_with(".d.ts")
        || file.ends_with(".d.mts")
        || file.ends_with(".d.cts")
        || file
            .strip_suffix(".ts")
            .and_then(|stem| stem.rsplit_once(".d."))
            .is_some_and(|(_, extension)| !extension.is_empty() && !extension.contains('.'))
}

/// A configuration that cannot be read: its errors, and no check, so nothing is complete.
fn unreadable_config(
    errors: Vec<crate::engine::config::ConfigError>,
    options: &CheckOptions,
) -> CheckResult {
    let mut result = CheckResult {
        checker_identity: format!("{}:{options:?}", crate::CHECKER_VERSION),
        complete: false,
        ..Default::default()
    };
    push_config_errors(&mut result, errors, options);
    result
}

fn push_config_errors(
    result: &mut CheckResult,
    errors: Vec<crate::engine::config::ConfigError>,
    options: &CheckOptions,
) {
    for error in errors {
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
            related: Vec::new(),
            path,
            span,
            code: Some(error.code),
            message: String::from_utf8_lossy(&message).into_owned(),
        });
    }
}

/// A module lowered for the checker.
pub(crate) struct Lowered<'s> {
    pub file: hir::File<'s>,
    /// What the checker cannot be trusted with: the parser's own diagnostics, which have no
    /// TypeScript code, and whatever the adapter does not cover. Each makes a result incomplete.
    pub unsupported: Vec<SourceDiagnostic>,
    /// The frontend's early errors. The module parsed, so it is checked, and TypeScript reports
    /// these itself from its grammar checks; see [`uncovered_early`].
    pub early: Vec<SourceDiagnostic>,
}

/// The checker's file for `module`, the oxc frontend's parse of the file at `name`.
pub(crate) fn lower_module<'s>(
    module: &Module,
    name: &str,
    arena: &'s Arena,
    atoms: &Interner<'s>,
    options: &Options,
) -> Lowered<'s> {
    let source_module = module
        .type_syntax
        .original_module
        .as_deref()
        .unwrap_or(module);
    let (mut errors, mut early) = (Vec::new(), Vec::new());
    // Owned parser diagnostics remain authoritative. Checking a recovered AST must neither erase
    // them nor publish specialization facts as complete.
    if source_module.panicked || !source_module.diagnostics.is_empty() {
        for diagnostic in &source_module.diagnostics {
            let reported = SourceDiagnostic {
                related: Vec::new(),
                path: Some(name.to_owned()),
                span: diagnostic.span,
                code: None,
                message: diagnostic.message.clone(),
            };
            match diagnostic.early {
                true => early.push(reported),
                false => errors.push(reported),
            }
        }
        if source_module.panicked && source_module.diagnostics.is_empty() {
            errors.push(SourceDiagnostic {
                related: Vec::new(),
                path: Some(name.to_owned()),
                span: None,
                code: None,
                message: "source parser could not recover a complete module".into(),
            });
        }
    }
    // The loader's arena belongs to this session. Its lifetime is narrowed to the parse call.
    let (mut file, unsupported) = Lower::new(source_module, atoms).finish(arena, atoms.session());
    // `experimentalDecorators` is a parser option in TypeScript.
    file.legacy_decorators = options.experimental_decorators;
    if is_declaration_file_name(name) {
        file.kind = hir::FileKind::Declaration;
    } else if matches!(
        source_module.source_kind,
        kiln_hir::SourceKind::Tsx | kiln_hir::SourceKind::Jsx
    ) {
        file.kind = hir::FileKind::Tsx;
    }
    errors.extend(
        unsupported
            .into_iter()
            .map(|(span, message)| SourceDiagnostic {
                related: Vec::new(),
                path: Some(name.to_owned()),
                span: Some(span),
                code: None,
                message: format!("checker adapter does not cover {message}"),
            }),
    );
    Lowered {
        file,
        unsupported: errors,
        early,
    }
}

/// The early errors in `early` that no checker diagnostic in `reported` overlaps. TypeScript
/// reports most early errors itself, with a code and its own wording; one it does not is still an
/// error the program has, so it is kept, and its result stays incomplete.
pub(crate) fn uncovered_early(
    early: Vec<SourceDiagnostic>,
    reported: &[crate::driver::Diagnostic],
) -> Vec<SourceDiagnostic> {
    early
        .into_iter()
        .filter(|e| {
            let (Some(path), Some(span)) = (e.path.as_deref(), e.span) else {
                return true;
            };
            !reported.iter().any(|d| {
                d.path == path.as_bytes()
                    && !d.has_undefined_range
                    && d.start <= span.end.max(span.start)
                    && span.start <= d.end.max(d.start)
            })
        })
        .collect()
}

fn check_inner(snapshot: &SourceSnapshot, roots: &[String], options: &CheckOptions) -> CheckResult {
    let session = Session::new();
    let host = SnapshotHost::new(snapshot);
    let root_files: Vec<Vec<u8>> = roots.iter().map(|p| p.as_bytes().to_vec()).collect();
    let mut project = if let Some(path) = &options.config_path {
        let project =
            crate::engine::config::load_overriding(&host, &session, path.as_bytes(), &|_| {
                Vec::new()
            });
        match project {
            Ok(project) => project,
            // `tsc`: the file cannot be read (TS5083), and nothing is checked.
            Err(errors) => return unreadable_config(errors, options),
        }
    } else {
        let mut project = crate::engine::config::without_config(
            &host,
            options.current_directory.as_bytes(),
            Json::Object(Vec::new()),
            root_files.clone(),
        );
        project.options = {
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
        project
    };
    let configuration_valid = project.errors.is_empty();
    project.options.current_directory = options.current_directory.as_bytes().to_vec();
    project.options.no_emit = true;
    project.options.lib_dir = LIBRARY_DIRECTORY.as_bytes().to_vec();
    project.files = root_files;
    // What only the checked program can answer: each source file's facts, which files are
    // complete, and where the stack ran out.
    #[derive(Default)]
    struct Read {
        types: Vec<SourceType>,
        complete_files: Vec<(String, bool)>,
        ran_out: bool,
        record_layouts: Vec<RecordLayout>,
    }
    let read: Mutex<Read> = Mutex::default();
    let checked = |program: &Program<'_>, report: &crate::driver::Report| {
        let files = program.files;
        // The contracts of each source file, read from the published state by checkers outside
        // the plan, whose own writes are dropped with them.
        let sources: Vec<(FileId, String)> = files
            .order
            .iter()
            .map(|&id| {
                (
                    id,
                    String::from_utf8_lossy(files.module(id).file_name()).into_owned(),
                )
            })
            .filter(|(_, path)| !path.starts_with(LIBRARY_DIRECTORY))
            .collect();
        let mut ran_out_in: Vec<FileId> = sources
            .iter()
            .filter(|(id, _)| {
                report
                    .incomplete
                    .iter()
                    .any(|path| path.as_slice() == files.module(*id).file_name())
            })
            .map(|&(id, _)| id)
            .collect();
        let (types, record_layouts, facts_ran_out) =
            source_facts(program, &sources, options, &host);
        ran_out_in.extend(facts_ran_out);
        let early = std::mem::take(&mut *host.early.lock().unwrap());
        let mut unsupported = host.unsupported.lock().unwrap();
        unsupported.extend(uncovered_early(early, &report.diagnostics));
        let complete = complete_files(
            files,
            &sources,
            &unsupported,
            &ran_out_in,
            configuration_valid,
        );
        *read.lock().unwrap() = Read {
            types,
            complete_files: complete,
            ran_out: !ran_out_in.is_empty(),
            record_layouts,
        };
    };
    let request = crate::driver::Request {
        // Facts are read from the published state after the plan.
        retains_everything: true,
        checked: Some(&checked),
        ..Default::default()
    };
    let report = crate::driver::check_project(&host, project, &request);
    let Read {
        types,
        complete_files,
        ran_out,
        record_layouts,
    } = read.into_inner().unwrap();
    let mut result = CheckResult {
        checker_identity: format!("{}:{options:?}", crate::CHECKER_VERSION),
        types,
        files: complete_files,
        record_layouts,
        ..Default::default()
    };
    // The embedded declarations (TypeScript's libraries, kiln's, @types/node) are pinned inputs a
    // user cannot fix, so whatever the project's `skipLibCheck`, problems inside them are not
    // reported as the project's own.
    result.diagnostics = report
        .diagnostics
        .into_iter()
        .filter(|d| !d.path.starts_with(LIBRARY_DIRECTORY.as_bytes()))
        .map(|d| {
            let of = |d: &crate::driver::Diagnostic, related| SourceDiagnostic {
                path: (!d.path.is_empty()).then(|| String::from_utf8_lossy(&d.path).into_owned()),
                span: (!d.has_undefined_range && !d.path.is_empty())
                    .then(|| Span::new(d.start, d.end)),
                code: Some(d.code),
                message: String::from_utf8_lossy(&d.text).into_owned(),
                related,
            };
            let related = d
                .related
                .iter()
                .filter(|r| !r.path.starts_with(LIBRARY_DIRECTORY.as_bytes()))
                .map(|r| of(r, Vec::new()))
                .collect();
            of(&d, related)
        })
        .collect();
    let unsupported = host.unsupported.into_inner().unwrap();
    result.complete = configuration_valid && unsupported.is_empty() && !ran_out;
    // A parser error at the place of a checker error is the same error: TypeScript reports it
    // from the checker's grammar checks, with a code. The result stays incomplete.
    let coded: std::collections::HashSet<(&str, u32)> = result
        .diagnostics
        .iter()
        .filter(|d| d.code.is_some())
        .filter_map(|d| Some((d.path.as_deref()?, d.span?.start)))
        .collect();
    let unsupported: Vec<_> = unsupported
        .into_iter()
        .filter(|d| {
            d.code.is_some()
                || !d
                    .path
                    .as_deref()
                    .zip(d.span)
                    .is_some_and(|(path, span)| coded.contains(&(path, span.start)))
        })
        .collect();
    result.diagnostics.extend(unsupported);
    if ran_out {
        result.diagnostics.push(SourceDiagnostic {
            related: Vec::new(),
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

#[derive(Default)]
pub(crate) struct RecordLayoutTable {
    pub layouts: Vec<RecordLayout>,
    pub by_layout: std::collections::HashMap<RecordLayout, RecordLayoutId>,
    pub visiting: std::collections::HashSet<crate::engine::types::TypeId>,
}

impl RecordLayoutTable {
    pub fn intern(&mut self, layout: RecordLayout) -> RecordLayoutId {
        if let Some(&id) = self.by_layout.get(&layout) {
            return id;
        }
        let id = RecordLayoutId(self.layouts.len() as u32);
        self.by_layout.insert(layout.clone(), id);
        self.layouts.push(layout);
        id
    }
}

fn resolve_record_layout(
    checker: &mut crate::engine::check::Checker<'_, '_>,
    table: &mut RecordLayoutTable,
    ty: crate::engine::types::TypeId,
) -> Option<RecordLayoutId> {
    if checker.is_any(ty)
        || checker.is_nullish(ty)
        || checker.is_array(ty)
        || checker.is_tuple(ty)
        || checker.is_number_like(ty)
        || checker.is_string_like(ty)
        || checker.is_boolean(ty)
        || checker.is_boolean_like(ty)
        || checker.is_bigint_like(ty)
        || checker.is_symbol_like(ty)
        || checker.every_type(ty, |c, m| {
            c.is_number_like(m) || c.is_string_like(m) || c.is_boolean(m) || c.is_boolean_like(m)
        })
        || matches!(
            checker.data(ty),
            crate::engine::types::TypeData::Ref { target, .. }
                if checker.files().flags(*target).contains(crate::engine::bind::SymFlags::CLASS)
        )
    {
        return None;
    }
    if !table.visiting.insert(ty) {
        return None;
    }
    let layout = extract_record_layout(checker, table, ty);
    table.visiting.remove(&ty);
    layout
}

fn extract_record_layout(
    checker: &mut crate::engine::check::Checker<'_, '_>,
    table: &mut RecordLayoutTable,
    ty: crate::engine::types::TypeId,
) -> Option<RecordLayoutId> {
    let members = checker.members(ty)?;
    let shape = members.shape();

    if shape.props.is_empty()
        || !shape.call.is_empty()
        || !shape.construct.is_empty()
        || !shape.index.is_empty()
    {
        return None;
    }

    use crate::engine::types::PropFlags;

    let mut prop_entries = Vec::with_capacity(shape.props.len());
    for prop in shape.props.iter() {
        if prop.flags.contains(PropFlags::METHOD)
            || prop.flags.contains(PropFlags::ACCESSOR)
            || prop.flags.contains(PropFlags::OPTIONAL)
        {
            return None;
        }
        let name_bytes = checker.atoms().bytes(prop.name);
        if name_bytes.starts_with(b"__") {
            return None;
        }
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        let readonly = prop.flags.contains(PropFlags::READONLY);
        let prop_ty = checker.type_of_prop(prop, members.mapper);
        prop_entries.push((name, readonly, prop_ty));
    }

    let mut fields = Vec::with_capacity(prop_entries.len());
    for (name, readonly, prop_ty) in prop_entries {
        let field_ty = if checker.is_int32_type(prop_ty) {
            NativeFieldType::Int32
        } else if checker.every_type(prop_ty, |c, m| c.is_number_like(m)) {
            NativeFieldType::Number
        } else if checker.every_type(prop_ty, |c, m| c.is_boolean(m) || c.is_boolean_like(m)) {
            NativeFieldType::Boolean
        } else if checker.every_type(prop_ty, |c, m| c.is_string_like(m)) {
            NativeFieldType::String
        } else if checker.is_array(prop_ty) {
            match resolve_array_element(checker, table, prop_ty) {
                Some(NativeFieldType::Number | NativeFieldType::Int32) => {
                    NativeFieldType::NumberArray
                }
                Some(NativeFieldType::Record(sub_id)) => NativeFieldType::RecordArray(sub_id),
                _ => return None,
            }
        } else if let Some(nested_id) = resolve_record_layout(checker, table, prop_ty) {
            NativeFieldType::Record(nested_id)
        } else {
            return None;
        };
        fields.push(RecordField {
            name,
            ty: field_ty,
            readonly,
        });
    }

    fields.sort_by(|a, b| a.name.cmp(&b.name));
    Some(table.intern(RecordLayout { fields }))
}

fn resolve_array_element(
    checker: &mut crate::engine::check::Checker<'_, '_>,
    table: &mut RecordLayoutTable,
    ty: crate::engine::types::TypeId,
) -> Option<NativeFieldType> {
    let elem_ty = checker.array_element(ty)?;
    if checker.is_int32_type(elem_ty) {
        Some(NativeFieldType::Int32)
    } else if checker.every_type(elem_ty, |c, m| c.is_number_like(m)) {
        Some(NativeFieldType::Number)
    } else if checker.every_type(elem_ty, |c, m| c.is_boolean(m) || c.is_boolean_like(m)) {
        Some(NativeFieldType::Boolean)
    } else if checker.every_type(elem_ty, |c, m| c.is_string_like(m)) {
        Some(NativeFieldType::String)
    } else if let Some(nested_id) = resolve_record_layout(checker, table, elem_ty) {
        Some(NativeFieldType::Record(nested_id))
    } else {
        None
    }
}

/// The facts of `sources` that `options` selects, the canonical record layouts, and the files
/// whose facts are incomplete because the stack ran out.
fn source_facts(
    program: &Program<'_>,
    sources: &[(FileId, String)],
    options: &CheckOptions,
    host: &SnapshotHost<'_>,
) -> (Vec<SourceType>, Vec<RecordLayout>, Vec<FileId>) {
    let files = program.files;
    // A declaration file compiles to nothing, so its facts would go unread.
    let fact_sources: Vec<usize> = if options.facts == FactSelection::None {
        Vec::new()
    } else {
        (0..sources.len())
            .filter(|&index| files.module(sources[index].0).hir.kind != hir::FileKind::Declaration)
            .collect()
    };
    let facts: Vec<Mutex<(Vec<SourceType>, Vec<RecordLayout>, bool)>> =
        fact_sources.iter().map(|_| Mutex::default()).collect();
    host.parallel(fact_sources.len(), &|index| {
        let (id, path) = (
            sources[fact_sources[index]].0,
            sources[fact_sources[index]].1.as_str(),
        );
        let module = files.module(id);
        let mut checker = program.checker();
        checker.begin_stack_budget();
        let mut types = Vec::new();
        let mut table = RecordLayoutTable::default();
        if options.facts == FactSelection::All {
            for (expr_index, e) in module.hir.exprs.iter().enumerate() {
                let ty = checker.type_of_expr(id, hir::ExprId(expr_index as u32));
                types.push(export_type(
                    &mut checker,
                    path,
                    Span::new(e.pos, e.end),
                    TypeRole::Expression,
                    ty,
                    &mut table,
                ));
            }
        }
        for (param_index, p) in module.hir.params.iter().enumerate() {
            let ty = checker.type_of_param(id, hir::ParamId(param_index as u32));
            types.push(export_type(
                &mut checker,
                path,
                Span::new(p.loc.pos, p.loc.end),
                TypeRole::Parameter,
                ty,
                &mut table,
            ));
        }
        for (fn_index, f) in module.hir.fns.iter().enumerate() {
            let ty = checker.return_type_of_fn(id, hir::FnId(fn_index as u32));
            types.push(export_type(
                &mut checker,
                path,
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
                &mut table,
            ));
        }
        for (var_index, decl) in module.hir.var_decls.iter().enumerate() {
            let ty = checker.type_of_var_decl(id, hir::VarDeclId(var_index as u32));
            let start = if decl.pat.is_some() {
                module.hir[decl.pat].pos
            } else {
                decl.loc.pos
            };
            let st = export_type(
                &mut checker,
                path,
                Span::new(start, decl.loc.end),
                TypeRole::Annotation,
                ty,
                &mut table,
            );
            if st.record_layout.is_some()
                || st.array_element.is_some()
                || options.facts == FactSelection::All
            {
                types.push(st);
            }
        }
        for (expr_index, e) in module.hir.exprs.iter().enumerate() {
            if matches!(e.kind, hir::ExprKind::Object(_) | hir::ExprKind::Array(_)) {
                let ty = checker.type_of_expr(id, hir::ExprId(expr_index as u32));
                let st = export_type(
                    &mut checker,
                    path,
                    Span::new(e.pos, e.end),
                    TypeRole::Annotation,
                    ty,
                    &mut table,
                );
                if st.record_layout.is_some() || st.array_element.is_some() {
                    types.push(st);
                }
            }
        }
        let ran_out = checker.take_ran_out_of_stack();
        *facts[index].lock().unwrap() = (types, table.layouts, ran_out);
    });
    let mut canonical_table = RecordLayoutTable::default();
    let (mut all, mut ran_out_in) = (Vec::new(), Vec::new());
    for (fact, &(id, _)) in facts
        .into_iter()
        .zip(fact_sources.iter().map(|&index| &sources[index]))
    {
        let (mut types, worker_layouts, fact_ran_out) = fact.into_inner().unwrap();
        if fact_ran_out {
            ran_out_in.push(id);
        }
        if worker_layouts.is_empty() {
            all.extend(types);
            continue;
        }
        let mut id_map: Vec<RecordLayoutId> = Vec::with_capacity(worker_layouts.len());
        for local_layout in &worker_layouts {
            let remapped_fields: Vec<RecordField> = local_layout
                .fields
                .iter()
                .map(|f| {
                    let ty = match f.ty {
                        NativeFieldType::Record(child_id) => {
                            NativeFieldType::Record(id_map[child_id.0 as usize])
                        }
                        NativeFieldType::RecordArray(child_id) => {
                            NativeFieldType::RecordArray(id_map[child_id.0 as usize])
                        }
                        other => other,
                    };
                    RecordField {
                        name: f.name.clone(),
                        ty,
                        readonly: f.readonly,
                    }
                })
                .collect();
            let canonical_id = canonical_table.intern(RecordLayout {
                fields: remapped_fields,
            });
            id_map.push(canonical_id);
        }
        for ty in &mut types {
            if let Some(local_id) = ty.record_layout {
                ty.record_layout = Some(id_map[local_id.0 as usize]);
            }
            if let Some(NativeFieldType::Record(local_id)) = ty.array_element {
                ty.array_element = Some(NativeFieldType::Record(id_map[local_id.0 as usize]));
            } else if let Some(NativeFieldType::RecordArray(local_id)) = ty.array_element {
                ty.array_element = Some(NativeFieldType::RecordArray(id_map[local_id.0 as usize]));
            }
        }
        all.extend(types);
    }
    (all, canonical_table.layouts, ran_out_in)
}

fn export_type(
    checker: &mut crate::engine::check::Checker<'_, '_>,
    path: &str,
    span: Span,
    role: TypeRole,
    ty: crate::engine::types::TypeId,
    table: &mut RecordLayoutTable,
) -> SourceType {
    let display = String::from_utf8_lossy(&checker.type_to_string(ty)).into_owned();
    let class = if checker.is_any(ty) {
        TypeClass::Any
    } else if checker.is_number_like(ty) {
        TypeClass::Number
    } else if checker.is_string_like(ty) {
        TypeClass::String
    } else if checker.is_boolean(ty) || checker.is_boolean_like(ty) {
        TypeClass::Boolean
    } else if checker.is_bigint_like(ty) {
        TypeClass::BigInt
    } else if checker.is_symbol_like(ty) {
        TypeClass::Symbol
    } else if checker.is_nullish(ty) {
        TypeClass::Nullish
    } else if checker.is_array(ty) {
        TypeClass::Array
    } else if checker.is_tuple(ty) {
        TypeClass::Tuple
    } else if display == "unknown" {
        TypeClass::Unknown
    } else {
        TypeClass::Composite
    };
    let record_layout = if class == TypeClass::Composite {
        resolve_record_layout(checker, table, ty)
    } else {
        None
    };
    let array_element = if class == TypeClass::Array {
        resolve_array_element(checker, table, ty)
    } else {
        None
    };
    SourceType {
        path: path.to_owned(),
        span,
        display,
        class,
        role,
        record_layout,
        array_element,
    }
}
