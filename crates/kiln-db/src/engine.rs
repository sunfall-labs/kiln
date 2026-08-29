//! `MemoEngine`: the memoizing query engine behind [`QueryEngine`].
//!
//! The algorithm is the standard red-green / verified-at scheme (design.md §2, docs/adr/0003):
//!
//! * every input records the epoch at which it last *changed*;
//! * every memo records `changed_at` (when its value last actually changed), `verified_at`
//!   (when we last confirmed it current) and the dependencies it read;
//! * a fetch first tries to *validate*: recursively bring each dependency up to date and, if
//!   none of them changed since `verified_at`, bump `verified_at` and hand back the cached
//!   value without running the query;
//! * only if validation fails do we recompute, and a recompute that produces an equal value
//!   leaves `changed_at` alone.
//!
//! That last line is early cutoff, and it is the entire reason this crate exists. It is what
//! makes "the interface did not change" stop propagating, without anyone maintaining a second
//! hash by hand and without the compiler having to know it is happening.

use std::collections::HashMap;

#[cfg(test)]
use crate::hash::content_hash;
use crate::hash::{Fnv1a, hash_names};
use crate::{
    Compiled, CompiledModule, FileId, Frontend, Interface, Parsed, Project, QueryEngine, Revision,
    Stats,
};
#[cfg(test)]
use kiln_hir::{Export, ExportName, ExportOrigin, Item, ItemKind, Module, SourceKind, Span};

/// The engine's internal validity clock.
///
/// Distinct from [`Revision`] on purpose. `Revision` is the user-visible input clock and counts
/// exactly one tick per `set_source`, which is what callers assert on. The epoch additionally
/// ticks when the *set of files* changes, because that set is an input too: `project()` reads it,
/// and interning a new file has to be able to invalidate a memo that was already verified at the
/// current revision. Keeping the two apart means growing the project does not silently inflate a
/// number other crates report as "edits".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
struct Epoch(u64);

impl Epoch {
    fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// A node in the query graph. Inputs and derived queries share one key space so that dependency
/// lists, cycle detection and validation are uniform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryKey {
    /// Input: the text of one file.
    Source(FileId),
    /// Input: which files exist at all. Only `project()` reads it.
    FileSet,
    Parse(FileId),
    Ir(FileId),
    ModuleIr(FileId),
    Interface(FileId),
    Project,
    /// Test-only query whose dependency is the other `Cyclic`. The v0 graph is acyclic by
    /// construction, so this is the only way to exercise cycle detection until the real frontend
    /// introduces import edges that can genuinely close a loop.
    #[cfg(test)]
    Cyclic(usize),
}

/// Bookkeeping shared by every memo, kept separate from the value so the red-green algorithm
/// can be written once instead of once per query.
#[derive(Debug)]
struct MemoMeta {
    /// Epoch at which this value last actually changed. Dependents compare against this.
    changed_at: Epoch,
    /// Epoch at which we last confirmed the value is current.
    verified_at: Epoch,
    /// What the last computation read. Replaced wholesale on every recompute - never appended
    /// to, or a dependency dropped by an edit would haunt the memo forever.
    deps: Vec<QueryKey>,
}

#[derive(Debug)]
struct Memo<T> {
    value: T,
    meta: MemoMeta,
}

#[derive(Debug)]
struct FileState {
    path: String,
    source: String,
    source_set: bool,
    source_changed_at: Epoch,
    parse: Option<Memo<Parsed>>,
    ir: Option<Memo<Compiled>>,
    module_ir: Option<Memo<CompiledModule>>,
    interface: Option<Memo<Interface>>,
}

/// An in-memory, single-threaded [`QueryEngine`].
///
/// Deliberately free of third-party dependencies: this crate is on the rebuild path of
/// everything else, and a slow `cargo check` here is a tax on every other workstream
/// (docs/adr/0004, docs/adr/0006).
#[derive(Debug)]
pub struct MemoEngine {
    frontend: Frontend,
    files: Vec<FileState>,
    by_path: HashMap<String, FileId>,
    project: Option<Memo<Project>>,
    /// Ticks when any file is added; read by `project()`.
    file_set_changed_at: Epoch,
    epoch: Epoch,
    revision: Revision,
    stats: Stats,
    /// Derived queries currently being fetched, innermost last. Empty between top-level calls.
    active: Vec<QueryKey>,
    #[cfg(test)]
    cyclic: [Option<Memo<u64>>; 2],
}

impl MemoEngine {
    pub fn new(frontend: Frontend) -> Self {
        Self {
            frontend,
            files: Vec::new(),
            by_path: HashMap::new(),
            project: None,
            file_set_changed_at: Epoch::default(),
            epoch: Epoch::default(),
            revision: Revision::default(),
            stats: Stats::default(),
            active: Vec::new(),
            #[cfg(test)]
            cyclic: [None, None],
        }
    }

    fn file(&self, file: FileId) -> &FileState {
        self.files.get(file.0 as usize).expect(UNINTERNED)
    }

    fn file_mut(&mut self, file: FileId) -> &mut FileState {
        self.files.get_mut(file.0 as usize).expect(UNINTERNED)
    }

    /// Bring `key` up to date and report the epoch at which its value last changed.
    ///
    /// Every dependency edge in the engine goes through here, which is what makes validation
    /// recursive: a dependency that is itself stale is validated (or recomputed) before its
    /// `changed_at` is compared.
    fn force(&mut self, key: QueryKey) -> Epoch {
        match key {
            QueryKey::Source(file) => self.file(file).source_changed_at,
            QueryKey::FileSet => self.file_set_changed_at,
            _ => self.force_derived(key),
        }
    }

    /// Guards the whole fetch, not just the recompute. Validation recurses into dependencies
    /// exactly like computation does, so a cycle reached while validating would spin between two
    /// stale memos forever without either of them ever being recomputed.
    fn force_derived(&mut self, key: QueryKey) -> Epoch {
        self.enter(key);
        let changed_at = self.validate_or_recompute(key);
        self.exit(key);
        changed_at
    }

    /// The red-green core: validate if we can, recompute if we must.
    ///
    /// `stats.validated` counts every fetch this serves from cache without running the query,
    /// including the ones a validation walk makes on its way down. That is deliberately a count
    /// of *memos reused*, not of calls the compiler made.
    fn validate_or_recompute(&mut self, key: QueryKey) -> Epoch {
        let Some(meta) = self.meta(key) else {
            return self.recompute(key);
        };
        let (changed_at, verified_at) = (meta.changed_at, meta.verified_at);

        if verified_at == self.epoch {
            self.stats.validated += 1;
            return changed_at;
        }

        let deps = meta.deps.clone();
        for dep in deps {
            if self.force(dep) > verified_at {
                return self.recompute(key);
            }
        }

        // Nothing this query read has changed since we last verified it, so the cached value is
        // still correct and - crucially - still has its old `changed_at`, so our own dependents
        // can take the same shortcut.
        self.meta_mut(key)
            .expect("memo present, checked above")
            .verified_at = self.epoch;
        self.stats.validated += 1;
        changed_at
    }

    fn recompute(&mut self, key: QueryKey) -> Epoch {
        let now = self.epoch;
        match key {
            QueryKey::Source(_) | QueryKey::FileSet => unreachable!("inputs are never computed"),
            QueryKey::Parse(file) => {
                let (value, deps) = self.compute_parse(file);
                self.stats.parse_computed += 1;
                let index = file.0 as usize;
                store(
                    &mut self.files[index].parse,
                    value,
                    deps,
                    now,
                    &mut self.stats,
                )
            }
            QueryKey::Ir(file) => {
                let (value, deps) = self.compute_ir(file);
                self.stats.ir_computed += 1;
                let index = file.0 as usize;
                store(&mut self.files[index].ir, value, deps, now, &mut self.stats)
            }
            QueryKey::ModuleIr(file) => {
                let (value, deps) = self.compute_module_ir(file);
                self.stats.ir_computed += 1;
                let index = file.0 as usize;
                store(
                    &mut self.files[index].module_ir,
                    value,
                    deps,
                    now,
                    &mut self.stats,
                )
            }
            QueryKey::Interface(file) => {
                let (value, deps) = self.compute_interface(file);
                self.stats.interface_computed += 1;
                let index = file.0 as usize;
                store(
                    &mut self.files[index].interface,
                    value,
                    deps,
                    now,
                    &mut self.stats,
                )
            }
            QueryKey::Project => {
                let (value, deps) = self.compute_project();
                self.stats.project_computed += 1;
                store(&mut self.project, value, deps, now, &mut self.stats)
            }
            #[cfg(test)]
            QueryKey::Cyclic(slot) => {
                let dep = QueryKey::Cyclic(slot ^ 1);
                let value = self.force(dep).0;
                store(
                    &mut self.cyclic[slot],
                    value,
                    vec![dep],
                    now,
                    &mut self.stats,
                )
            }
        }
    }

    fn compute_parse(&mut self, file: FileId) -> (Parsed, Vec<QueryKey>) {
        let dep = QueryKey::Source(file);
        self.force(dep);
        let state = self.file(file);
        let module = (self.frontend)(&state.path, &state.source);
        (Parsed::new(&state.source, module), vec![dep])
    }

    fn compute_ir(&mut self, file: FileId) -> (Compiled, Vec<QueryKey>) {
        let dep = QueryKey::Parse(file);
        self.force(dep);
        let parsed = self.file(file).parse.as_ref().expect(FORCED);
        (kiln_ir::compile(&parsed.value.module), vec![dep])
    }

    fn compute_module_ir(&mut self, file: FileId) -> (CompiledModule, Vec<QueryKey>) {
        let dep = QueryKey::Parse(file);
        self.force(dep);
        let parsed = self.file(file).parse.as_ref().expect(FORCED);
        (kiln_ir::compile_module(&parsed.value.module), vec![dep])
    }

    fn compute_interface(&mut self, file: FileId) -> (Interface, Vec<QueryKey>) {
        let dep = QueryKey::Parse(file);
        self.force(dep);
        let parsed = self.file(file).parse.as_ref().expect(FORCED);

        // Normalized, because equality of this value is what gates every dependent (adr/0003):
        // a module's interface is a *set* of names, so reordering or duplicating declarations
        // must not invalidate anything.
        let mut exports = parsed.value.exports.clone();
        exports.sort();
        exports.dedup();

        let interface = Interface {
            interface_hash: hash_names(&exports),
            exports,
        };
        (interface, vec![dep])
    }

    fn compute_project(&mut self) -> (Project, Vec<QueryKey>) {
        let mut deps = Vec::with_capacity(self.files.len() + 1);
        deps.push(QueryKey::FileSet);
        self.force(QueryKey::FileSet);

        // Forcing an interface cannot intern a file, so the length is stable across the loop.
        for index in 0..self.files.len() {
            let dep = QueryKey::Interface(FileId(index as u32));
            self.force(dep);
            deps.push(dep);
        }

        let mut hasher = Fnv1a::new();
        let mut exports = 0;
        for state in &self.files {
            let interface = &state.interface.as_ref().expect(FORCED).value;
            hasher.write_u64(interface.interface_hash);
            exports += interface.exports.len();
        }

        let project = Project {
            modules: self.files.len(),
            exports,
            interface_hash: hasher.finish(),
        };
        (project, deps)
    }

    fn meta(&self, key: QueryKey) -> Option<&MemoMeta> {
        match key {
            QueryKey::Source(_) | QueryKey::FileSet => None,
            QueryKey::Parse(file) => self.file(file).parse.as_ref().map(|memo| &memo.meta),
            QueryKey::Ir(file) => self.file(file).ir.as_ref().map(|memo| &memo.meta),
            QueryKey::ModuleIr(file) => self.file(file).module_ir.as_ref().map(|memo| &memo.meta),
            QueryKey::Interface(file) => self.file(file).interface.as_ref().map(|memo| &memo.meta),
            QueryKey::Project => self.project.as_ref().map(|memo| &memo.meta),
            #[cfg(test)]
            QueryKey::Cyclic(slot) => self.cyclic[slot].as_ref().map(|memo| &memo.meta),
        }
    }

    fn meta_mut(&mut self, key: QueryKey) -> Option<&mut MemoMeta> {
        match key {
            QueryKey::Source(_) | QueryKey::FileSet => None,
            QueryKey::Parse(file) => self
                .file_mut(file)
                .parse
                .as_mut()
                .map(|memo| &mut memo.meta),
            QueryKey::Ir(file) => self.file_mut(file).ir.as_mut().map(|memo| &mut memo.meta),
            QueryKey::ModuleIr(file) => self
                .file_mut(file)
                .module_ir
                .as_mut()
                .map(|memo| &mut memo.meta),
            QueryKey::Interface(file) => self
                .file_mut(file)
                .interface
                .as_mut()
                .map(|memo| &mut memo.meta),
            QueryKey::Project => self.project.as_mut().map(|memo| &mut memo.meta),
            #[cfg(test)]
            QueryKey::Cyclic(slot) => self.cyclic[slot].as_mut().map(|memo| &mut memo.meta),
        }
    }

    /// Marks `key` as in-flight, panicking if it already is.
    ///
    /// A cycle is a bug in the query graph, not a user error, so it aborts rather than being
    /// reported: there is no correct value to return and silently returning a stale one would be
    /// far worse than a crash. The panic leaves `active` non-empty, so an engine that survives it
    /// (a daemon catching the unwind) must be discarded rather than queried again.
    fn enter(&mut self, key: QueryKey) {
        if let Some(start) = self.active.iter().position(|active| *active == key) {
            let cycle: Vec<String> = self.active[start..]
                .iter()
                .chain(std::iter::once(&key))
                .map(|key| self.describe(*key))
                .collect();
            panic!("query cycle detected: {}", cycle.join(" -> "));
        }
        self.active.push(key);
    }

    fn exit(&mut self, key: QueryKey) {
        let popped = self.active.pop();
        debug_assert_eq!(popped, Some(key), "unbalanced query stack");
    }

    fn describe(&self, key: QueryKey) -> String {
        let path = |file: FileId| {
            self.files
                .get(file.0 as usize)
                .map_or("<unknown>", |state| state.path.as_str())
        };
        match key {
            QueryKey::Source(file) => format!("source({})", path(file)),
            QueryKey::FileSet => "file_set()".to_string(),
            QueryKey::Parse(file) => format!("parse({})", path(file)),
            QueryKey::Ir(file) => format!("ir({})", path(file)),
            QueryKey::ModuleIr(file) => format!("module_ir({})", path(file)),
            QueryKey::Interface(file) => format!("interface({})", path(file)),
            QueryKey::Project => "project()".to_string(),
            #[cfg(test)]
            QueryKey::Cyclic(slot) => format!("cyclic({slot})"),
        }
    }
}

const UNINTERNED: &str = "FileId was not interned by this engine";
const FORCED: &str = "dependency was forced immediately above";

/// Installs a freshly computed value, preserving `changed_at` when the value is unchanged.
///
/// This is early cutoff. It is a free function taking the slot and the counters separately so
/// that it can borrow one field of the engine while the caller still owns the rest.
fn store<T: PartialEq>(
    slot: &mut Option<Memo<T>>,
    value: T,
    deps: Vec<QueryKey>,
    now: Epoch,
    stats: &mut Stats,
) -> Epoch {
    let changed_at = match slot {
        Some(previous) if previous.value == value => {
            stats.early_cutoff += 1;
            previous.meta.changed_at
        }
        _ => now,
    };
    *slot = Some(Memo {
        value,
        meta: MemoMeta {
            changed_at,
            verified_at: now,
            deps,
        },
    });
    changed_at
}

/// Tiny deterministic frontend used only by this crate's query-engine tests.
///
/// Production callers inject `frontend_oxc::parse`; keeping the fixture here lets the foundational
/// database test red-green invalidation without putting Oxc on its own test dependency path.
#[cfg(test)]
fn fixture_frontend(_path: &str, text: &str) -> Module {
    // `export default ...` really does export the name "default", so the bare fallthrough below
    // gets that case right without special handling.
    const DECLARATIONS: [&str; 6] = [
        "async function ",
        "function ",
        "const ",
        "let ",
        "var ",
        "class ",
    ];

    let mut exports = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("export ") else {
            continue;
        };
        let rest = rest.trim_start();
        let rest = DECLARATIONS
            .iter()
            .find_map(|kw| rest.strip_prefix(kw))
            .unwrap_or(rest);
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !name.is_empty() {
            exports.push(name);
        }
    }

    Module {
        source_kind: SourceKind::JavaScript,
        source_goal: kiln_hir::SourceGoal::Module,
        items: text
            .lines()
            .enumerate()
            .map(|(line, _)| Item {
                kind: ItemKind::Other,
                span: Span::new(line as u32, line as u32 + 1),
            })
            .collect(),
        body: Vec::new(),
        requested_modules: Vec::new(),
        imports: Vec::new(),
        exports: exports
            .into_iter()
            .map(|name| Export {
                exported: Some(ExportName::Name(name.clone())),
                origin: ExportOrigin::Local { local: name },
                type_only: false,
                span: Span::default(),
                statement_span: Span::default(),
            })
            .collect(),
        diagnostics: Vec::new(),
        panicked: false,
    }
}

impl QueryEngine for MemoEngine {
    fn intern(&mut self, path: &str) -> FileId {
        if let Some(file) = self.by_path.get(path) {
            return *file;
        }
        let file = FileId(self.files.len() as u32);
        // The file set is an input, so adding to it has to invalidate whatever read it. Only the
        // internal epoch moves: no source was written, so the revision has not advanced.
        self.epoch = self.epoch.next();
        self.file_set_changed_at = self.epoch;
        self.files.push(FileState {
            path: path.to_string(),
            source: String::new(),
            source_set: false,
            source_changed_at: self.epoch,
            parse: None,
            ir: None,
            module_ir: None,
            interface: None,
        });
        self.by_path.insert(path.to_string(), file);
        file
    }

    fn set_source(&mut self, file: FileId, text: String) {
        // Writing byte-identical text is not an edit. Marking it as one was the deliberate
        // choice here at first - "the input really was written" - but nothing observable
        // distinguishes the two cases, and the cost was not hypothetical. A stateless client
        // that re-ingests the whole project on every command made every command reparse every
        // file; early cutoff contained the damage at the interface, so the rollup stayed
        // correct and the waste sat one layer below anything that was asserting on it.
        if self.file(file).source_set && self.file(file).source == text {
            return;
        }
        self.epoch = self.epoch.next();
        self.revision = Revision(self.revision.0 + 1);
        let epoch = self.epoch;
        let state = self.file_mut(file);
        state.source = text;
        state.source_set = true;
        state.source_changed_at = epoch;
    }

    fn source(&self, file: FileId) -> Option<&str> {
        self.files
            .get(file.0 as usize)
            .and_then(|state| state.source_set.then_some(state.source.as_str()))
    }

    fn parse(&mut self, file: FileId) -> Parsed {
        self.force(QueryKey::Parse(file));
        self.file(file).parse.as_ref().expect(FORCED).value.clone()
    }

    fn ir(&mut self, file: FileId) -> Compiled {
        self.force(QueryKey::Ir(file));
        self.file(file).ir.as_ref().expect(FORCED).value.clone()
    }

    fn module_ir(&mut self, file: FileId) -> CompiledModule {
        self.force(QueryKey::ModuleIr(file));
        self.file(file)
            .module_ir
            .as_ref()
            .expect(FORCED)
            .value
            .clone()
    }

    fn interface(&mut self, file: FileId) -> Interface {
        self.force(QueryKey::Interface(file));
        self.file(file)
            .interface
            .as_ref()
            .expect(FORCED)
            .value
            .clone()
    }

    fn project(&mut self) -> Project {
        self.force(QueryKey::Project);
        self.project.as_ref().expect(FORCED).value.clone()
    }

    fn revision(&self) -> Revision {
        self.revision
    }

    fn stats(&self) -> Stats {
        Stats {
            revision: self.revision.0,
            ..self.stats
        }
    }

    fn reset_stats(&mut self) {
        // Counters only: the revision is a property of the inputs, not of the accounting.
        self.stats = Stats::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A_V1: &str = "// alpha, first cut\nexport function alpha() {\n  return 1;\n}\n";
    /// Same exports as [`A_V1`], different everything else.
    const A_BODY_EDIT: &str = "// alpha, revised\nexport function alpha() {\n  return 2;\n}\n";
    /// Adds an export, so the interface really does change.
    const A_NEW_EXPORT: &str =
        "// alpha, revised\nexport function alpha() {\n  return 2;\n}\nexport const gamma = 3;\n";
    const B_V1: &str = "export const beta = 2;\n";

    fn engine(files: &[(&str, &str)]) -> (MemoEngine, Vec<FileId>) {
        let mut db = MemoEngine::new(fixture_frontend);
        let ids = files
            .iter()
            .map(|(path, text)| {
                let file = db.intern(path);
                db.set_source(file, (*text).to_string());
                file
            })
            .collect();
        (db, ids)
    }

    /// Two files, cold. Nothing is cached, so everything is computed - but only once each.
    fn cold_project() -> (MemoEngine, Vec<FileId>) {
        let (mut db, files) = engine(&[("a.js", A_V1), ("b.js", B_V1)]);
        db.project();
        db.reset_stats();
        (db, files)
    }

    #[test]
    fn cold_fetch_computes_each_query_exactly_once() {
        let (mut db, files) = engine(&[("a.js", A_V1), ("b.js", B_V1)]);

        let project = db.project();
        assert_eq!(project.modules, 2);
        assert_eq!(project.exports, 2);
        assert_eq!(
            db.stats(),
            Stats {
                revision: 2,
                parse_computed: 2,
                ir_computed: 0,
                interface_computed: 2,
                project_computed: 1,
                validated: 0,
                early_cutoff: 0,
            }
        );

        // Fetching the intermediate results the rollup already forced must reuse them.
        assert_eq!(db.parse(files[0]).exports, ["alpha"]);
        assert_eq!(db.interface(files[1]).exports, ["beta"]);
        assert_eq!(
            db.stats(),
            Stats {
                revision: 2,
                parse_computed: 2,
                ir_computed: 0,
                interface_computed: 2,
                project_computed: 1,
                validated: 2,
                early_cutoff: 0,
            },
            "already-verified memos should be served straight from cache"
        );
    }

    #[test]
    fn refetching_at_the_same_revision_computes_nothing() {
        let (mut db, files) = cold_project();

        let before = db.project();
        assert_eq!(db.project(), before);
        db.parse(files[0]);
        db.interface(files[0]);

        assert_eq!(
            db.stats(),
            Stats {
                revision: 2,
                parse_computed: 0,
                ir_computed: 0,
                interface_computed: 0,
                project_computed: 0,
                validated: 4,
                early_cutoff: 0,
            }
        );
    }

    #[test]
    fn bytecode_is_a_memoized_parse_dependent() {
        let (mut db, files) = engine(&[("a.js", A_V1)]);
        let first = db.ir(files[0]).expect("fixture HIR compiles");
        assert_eq!(db.stats().parse_computed, 1);
        assert_eq!(db.stats().ir_computed, 1);

        db.reset_stats();
        assert_eq!(db.ir(files[0]).expect("cached IR remains valid"), first);
        assert_eq!(db.stats().ir_computed, 0);
        assert_eq!(db.stats().validated, 1);

        db.reset_stats();
        db.set_source(files[0], A_BODY_EDIT.to_string());
        assert_eq!(db.ir(files[0]).expect("edited fixture HIR compiles"), first);
        assert_eq!(db.stats().parse_computed, 1);
        assert_eq!(db.stats().ir_computed, 1);
        assert_eq!(db.stats().early_cutoff, 1);
    }

    #[test]
    fn linkage_bytecode_is_memoized_independently() {
        let (mut db, files) = engine(&[("a.js", "const hidden = 1;\n")]);
        let first = db.module_ir(files[0]).expect("fixture module compiles");
        assert_eq!(db.stats().parse_computed, 1);
        assert_eq!(db.stats().ir_computed, 1);

        db.reset_stats();
        assert_eq!(
            db.module_ir(files[0])
                .expect("cached module IR remains valid"),
            first
        );
        assert_eq!(db.stats().ir_computed, 0);
        assert_eq!(db.stats().validated, 1);

        db.reset_stats();
        db.set_source(files[0], "const hidden = 2;\n".to_owned());
        assert_eq!(
            db.module_ir(files[0])
                .expect("edited fixture module compiles"),
            first
        );
        assert_eq!(db.stats().parse_computed, 1);
        assert_eq!(db.stats().ir_computed, 1);
        assert_eq!(db.stats().early_cutoff, 1);
    }

    /// The headline property: an edit that does not change the interface does not reach the
    /// project. Parse must recompute (the text really did change), the interface recomputes but
    /// produces an equal value and so cuts off, and the rollup is merely revalidated.
    #[test]
    fn body_edit_cuts_off_at_the_interface() {
        let (mut db, files) = cold_project();
        let project_before = db.project();
        let interface_before = db.interface(files[0]);
        db.reset_stats();

        db.set_source(files[0], A_BODY_EDIT.to_string());
        let project_after = db.project();

        assert_eq!(
            db.stats(),
            Stats {
                revision: 3,
                parse_computed: 1,
                ir_computed: 0,
                interface_computed: 1,
                project_computed: 0,
                validated: 4,
                early_cutoff: 1,
            }
        );
        assert_eq!(
            project_after, project_before,
            "the rollup value must be unchanged"
        );
        assert_eq!(db.interface(files[0]), interface_before);
        // The one thing that must have changed: what the file actually contains.
        assert_ne!(db.parse(files[0]).implementation_hash, content_hash(A_V1));
    }

    #[test]
    fn adding_an_export_propagates_to_the_project() {
        let (mut db, files) = cold_project();
        let project_before = db.project();
        db.reset_stats();

        db.set_source(files[0], A_NEW_EXPORT.to_string());
        let project_after = db.project();

        assert_eq!(project_after.exports, project_before.exports + 1);
        assert_ne!(project_after.interface_hash, project_before.interface_hash);
        assert_eq!(
            db.stats(),
            Stats {
                revision: 3,
                parse_computed: 1,
                ir_computed: 0,
                interface_computed: 1,
                project_computed: 1,
                validated: 4,
                early_cutoff: 0,
            }
        );
    }

    #[test]
    fn editing_one_file_does_not_recompute_another() {
        let (mut db, files) = cold_project();
        let (a, b) = (files[0], files[1]);
        let parse_b = db.parse(b);
        let interface_b = db.interface(b);
        db.reset_stats();

        db.set_source(a, A_NEW_EXPORT.to_string());
        db.project();

        // One parse and one interface recompute in total, and they are demonstrably a's: b's
        // values are still the ones computed before the edit, served without recomputing.
        assert_eq!(db.stats().parse_computed, 1);
        assert_eq!(db.stats().interface_computed, 1);
        db.reset_stats();
        assert_eq!(db.parse(b), parse_b);
        assert_eq!(db.interface(b), interface_b);
        assert_eq!(
            db.stats(),
            Stats {
                revision: 3,
                validated: 2,
                ..Stats::default()
            },
            "b was only ever validated"
        );
    }

    /// A write is a write even when the bytes are the same, so parse reruns - and then the
    /// equal-value check stops it there, one level earlier than a body edit does.
    #[test]
    fn rewriting_identical_text_is_not_an_edit() {
        let (mut db, files) = cold_project();

        db.set_source(files[0], A_V1.to_string());
        db.project();

        assert_eq!(
            db.stats(),
            Stats {
                revision: 2,
                parse_computed: 0,
                ir_computed: 0,
                interface_computed: 0,
                project_computed: 0,
                validated: 1,
                early_cutoff: 0,
            },
            "re-ingesting unchanged text must cost nothing, not one parse per file"
        );
    }

    /// The interface is a *set* of names, so reordering declarations must not disturb the
    /// project even though the file's text - and therefore its parse - changed.
    #[test]
    fn reordering_exports_does_not_change_the_interface() {
        let (mut db, files) = engine(&[("a.js", "export const one = 1;\nexport const two = 2;\n")]);
        let interface_before = db.interface(files[0]);
        db.project();
        db.reset_stats();

        db.set_source(
            files[0],
            "export const two = 2;\nexport const one = 1;\n".to_string(),
        );
        db.project();

        assert_eq!(db.interface(files[0]), interface_before);
        assert_eq!(db.stats().parse_computed, 1);
        assert_eq!(db.stats().project_computed, 0);
        assert_eq!(
            db.stats().early_cutoff,
            1,
            "the interface recomputed to an equal value"
        );
    }

    /// The file set is an input in its own right: `project()` reads it, so growing it has to
    /// invalidate the rollup even though no source was written and the revision did not move.
    #[test]
    fn interning_a_file_invalidates_the_project_without_bumping_the_revision() {
        let (mut db, _) = engine(&[("a.js", A_V1)]);
        assert_eq!(db.project().modules, 1);
        let revision = db.revision();

        let b = db.intern("b.js");
        assert_eq!(db.revision(), revision, "interning is not a source write");
        assert_eq!(db.project().modules, 2);
        assert_eq!(db.project().exports, 1, "b.js is empty until it is written");

        db.set_source(b, B_V1.to_string());
        assert_eq!(db.project().exports, 2);
        assert_eq!(db.revision(), Revision(revision.0 + 1));
    }

    #[test]
    fn interning_is_idempotent() {
        let mut db = MemoEngine::new(fixture_frontend);
        let first = db.intern("a.js");
        assert_eq!(db.intern("a.js"), first);
        db.set_source(first, A_V1.to_string());
        assert_eq!(db.intern("a.js"), first);
        assert_eq!(db.source(first), Some(A_V1));
        assert_eq!(db.source(FileId(7)), None);
    }

    #[test]
    fn resetting_stats_keeps_the_revision_and_the_cache() {
        let (mut db, _) = cold_project();
        assert_eq!(
            db.stats(),
            Stats {
                revision: 2,
                ..Stats::default()
            }
        );
        db.project();
        assert_eq!(
            db.stats().project_computed,
            0,
            "the cache survived reset_stats"
        );
    }

    /// The v0 graph cannot close a loop, but the engine will grow import edges that can. A cycle
    /// must report itself rather than recursing until the stack runs out.
    #[test]
    fn a_cycle_is_reported_rather_than_hanging() {
        let mut db = MemoEngine::new(fixture_frontend);

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.force(QueryKey::Cyclic(0))
        }));
        std::panic::set_hook(previous);

        let payload = result.expect_err("forcing a cyclic query must panic");
        let message = payload.downcast_ref::<String>().map_or("", String::as_str);
        assert!(
            message.contains("query cycle detected"),
            "unexpected panic: {message}"
        );
        assert!(
            message.contains("cyclic(0) -> cyclic(1) -> cyclic(0)"),
            "the panic should name the cycle, got: {message}"
        );
    }

    #[test]
    fn exports_are_extracted_from_declaration_forms() {
        const SAMPLE: &str = concat!(
            "import x from './x.js';\n",
            "export function f() {}\n",
            "export async function g() {}\n",
            "export const h = 1;\n",
            "export let i;\n",
            "  export var j;\n",
            "export class K {}\n",
            // `export default` exports the name "default", so the plain fallthrough is correct.
            "export default 3;\n",
            "let hidden = 2;\n",
        );

        let parsed = Parsed::new(SAMPLE, fixture_frontend("sample.js", SAMPLE));
        assert_eq!(parsed.exports, ["f", "g", "h", "i", "j", "K", "default"]);
        assert_eq!(parsed.lines, 9);
        assert_eq!(parsed.implementation_hash, content_hash(SAMPLE));
    }

    /// The claim the whole project rests on, at a size where an O(n) rebuild would be obvious:
    /// one body edit in a 200-module project costs exactly one parse.
    #[test]
    fn a_body_edit_in_a_large_project_costs_one_parse() {
        let sources: Vec<(String, String)> = (0..200)
            .map(|i| {
                (
                    format!("m{i}.js"),
                    format!("export const value{i} = {i};\n"),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = sources
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        let (mut db, files) = engine(&borrowed);
        db.project();
        db.reset_stats();

        db.set_source(
            files[137],
            "// a comment\nexport const value137 = 137;\n".to_string(),
        );
        db.project();

        assert_eq!(db.stats().parse_computed, 1);
        assert_eq!(db.stats().interface_computed, 1);
        assert_eq!(db.stats().project_computed, 0);
        assert_eq!(db.stats().early_cutoff, 1);
    }

    #[test]
    fn empty_project_is_well_defined() {
        let mut db = MemoEngine::new(fixture_frontend);
        assert_eq!(
            db.project(),
            Project {
                modules: 0,
                exports: 0,
                interface_hash: Fnv1a::new().finish()
            }
        );
        assert_eq!(db.revision(), Revision(0));
    }
}
