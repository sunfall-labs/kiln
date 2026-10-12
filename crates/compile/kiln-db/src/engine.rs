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

use ahash::AHashMap;
use rayon::prelude::*;

use crate::{
    Compiled, CompiledModule, FileId, Frontend, FunctionSummary, Interface, Parsed, Project,
    QueryEngine, Revision, SourceCheck, SourceCheckQuery, Stats, SummaryQuery,
};
#[cfg(test)]
use crate::{Digest, Linkage, hash::content_hash};
#[cfg(test)]
use kiln_hir::{Export, ExportName, ExportOrigin, Item, ItemKind, Module, SourceKind, Span};

/// The engine's internal validity clock.
///
/// Distinct from [`Revision`] on purpose. `Revision` is the user-visible input clock and counts
/// exactly one tick per source mutation, which is what callers assert on. The epoch is the
/// engine's internal validation clock; keeping the concepts separate avoids coupling query
/// bookkeeping to the public meaning of an edit.
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
    /// Input: which interned paths currently have source text. Only `project()` reads it.
    FileSet,
    Parse(FileId),
    Ir(FileId),
    ModuleIr(FileId),
    Summaries(FileId),
    SourceCheck(FileId),
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
    source_revision: Revision,
    parse: Option<Memo<Parsed>>,
    ir: Option<Memo<Compiled>>,
    module_ir: Option<Memo<CompiledModule>>,
    module_ir_by_identity: AHashMap<String, (Revision, Option<Epoch>, CompiledModule)>,
    summaries: Option<Memo<Vec<FunctionSummary>>>,
    source_check: Option<Memo<SourceCheck>>,
    interface: Option<Memo<Interface>>,
}

/// An in-memory, single-threaded [`QueryEngine`].
///
/// Deliberately limited to small, concrete compiler-owned dependencies: this crate is on the
/// rebuild path of everything else, and a slow `cargo check` here is a tax on every other
/// workstream (docs/adr/0004, docs/adr/0006, docs/adr/0010).
#[derive(Debug)]
pub struct MemoEngine {
    frontend: Frontend,
    summary_query: Option<SummaryQuery>,
    source_check_query: Option<SourceCheckQuery>,
    files: Vec<FileState>,
    by_path: AHashMap<String, FileId>,
    project: Option<Memo<Project>>,
    /// Ticks when a source becomes active or inactive; read by `project()`.
    file_set_changed_at: Epoch,
    epoch: Epoch,
    revision: Revision,
    stats: Stats,
    /// Derived queries currently being fetched, innermost last. Empty between top-level calls.
    active: Vec<QueryKey>,
    /// A source check running on its own thread, which forcing it in the same epoch joins.
    pending_check: Option<PendingCheck>,
    /// Whether compiling a TypeScript module reads its source check's facts.
    compile_facts: bool,
    #[cfg(test)]
    cyclic: [Option<Memo<u64>>; 2],
}

/// [`QueryEngine::begin_source_check`]'s check: its file, the epoch whose inputs it read, the
/// dependencies those inputs are, and its thread.
struct PendingCheck {
    file: FileId,
    epoch: Epoch,
    deps: Vec<QueryKey>,
    handle: std::thread::JoinHandle<SourceCheck>,
}

impl std::fmt::Debug for PendingCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingCheck")
            .field("file", &self.file)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// What a source check reads: the files TypeScript parses, the rest as text.
struct CheckInputs {
    query: SourceCheckQuery,
    path: String,
    modules: Vec<(String, std::sync::Arc<crate::HirModule>)>,
    auxiliary: Vec<(String, std::sync::Arc<[u8]>)>,
    deps: Vec<QueryKey>,
}

/// A JavaScript file, which a TypeScript program only reads if it allows JavaScript: the checker
/// receives its text and parses it if the program loads it.
fn is_javascript(path: &str) -> bool {
    [".js", ".mjs", ".cjs", ".jsx"]
        .iter()
        .any(|extension| path.ends_with(extension))
}

impl MemoEngine {
    /// Build a backend-independent engine with no function-summary provider.
    ///
    /// Function summaries remain empty and do not participate in interface invalidation.
    /// Callers that consume backend analysis should use [`Self::with_summary_query`].
    pub fn new(frontend: Frontend) -> Self {
        Self::build(frontend, None)
    }

    /// Register the summary query for the lifetime of this engine.
    ///
    /// Summaries are memoized from bytecode and included in interface digests. The query must
    /// return deterministic results for a given program; changing providers requires a new engine.
    pub fn with_summary_query(frontend: Frontend, summary_query: SummaryQuery) -> Self {
        Self::build(frontend, Some(summary_query))
    }

    /// Register source checking without depending on a concrete checker implementation.
    pub fn with_source_check_query(mut self, query: SourceCheckQuery) -> Self {
        self.source_check_query = Some(query);
        self
    }

    fn build(frontend: Frontend, summary_query: Option<SummaryQuery>) -> Self {
        Self {
            frontend,
            summary_query,
            source_check_query: None,
            files: Vec::new(),
            by_path: AHashMap::new(),
            project: None,
            file_set_changed_at: Epoch::default(),
            epoch: Epoch::default(),
            revision: Revision::default(),
            stats: Stats::default(),
            active: Vec::new(),
            pending_check: None,
            compile_facts: true,
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
    /// stale memos forever without either of them ever being recomputed. The query stack must be
    /// restored before any panic continues unwinding: the daemon catches a failed request and
    /// keeps this same engine alive for the next one.
    fn force_derived(&mut self, key: QueryKey) -> Epoch {
        self.enter(key);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.validate_or_recompute(key)
        }));
        self.exit(key);
        match result {
            Ok(changed_at) => changed_at,
            Err(payload) => std::panic::resume_unwind(payload),
        }
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
            QueryKey::Summaries(file) => {
                let (value, deps) = self.compute_summaries(file);
                let index = file.0 as usize;
                store(
                    &mut self.files[index].summaries,
                    value,
                    deps,
                    now,
                    &mut self.stats,
                )
            }
            QueryKey::SourceCheck(file) => {
                let (value, deps) = self.compute_source_check(file);
                store(
                    &mut self.files[file.0 as usize].source_check,
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

    /// Parse every stale file among `files` at once, storing each memo exactly as forcing its
    /// parse would. A parse reads only its own source and the frontend is a pure function, so the
    /// stored values are the ones a sequential walk would compute. A file whose parse panics is
    /// left stale: forcing it later reports the panic the way it always has.
    fn parse_stale_in_parallel(&mut self, files: &[FileId]) {
        let epoch = self.epoch;
        let stale = files
            .iter()
            .copied()
            .filter(|&file| {
                let state = self.file(file);
                state.parse.as_ref().is_none_or(|memo| {
                    memo.meta.verified_at != epoch
                        && state.source_changed_at > memo.meta.verified_at
                })
            })
            .collect::<Vec<_>>();
        if stale.len() < 2 {
            return;
        }
        let Some(pool) = parse_pool() else { return };
        let frontend = self.frontend;
        let files_state = &self.files;
        let parsed: Vec<Option<Parsed>> = pool.install(|| {
            stale
                .par_iter()
                .map(|&file| {
                    let state = &files_state[file.0 as usize];
                    std::panic::catch_unwind(|| {
                        Parsed::new(&state.source, frontend(&state.path, &state.source))
                    })
                    .ok()
                })
                .collect()
        });
        for (file, value) in stale.into_iter().zip(parsed) {
            let Some(value) = value else {
                continue;
            };
            self.stats.parse_computed += 1;
            let deps = vec![QueryKey::Source(file)];
            store(
                &mut self.files[file.0 as usize].parse,
                value,
                deps,
                epoch,
                &mut self.stats,
            );
        }
    }

    fn compute_source_check(&mut self, file: FileId) -> (SourceCheck, Vec<QueryKey>) {
        if let Some(pending) = self.pending_check.take()
            && pending.file == file
            && pending.epoch == self.epoch
        {
            match pending.handle.join() {
                Ok(result) => return (result, pending.deps),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        let Some(inputs) = self.source_check_inputs(file) else {
            return (SourceCheck::default(), Vec::new());
        };
        let result = (inputs.query)(&inputs.path, &inputs.modules, &inputs.auxiliary);
        (result, inputs.deps)
    }

    fn source_check_inputs(&mut self, file: FileId) -> Option<CheckInputs> {
        let query = self.source_check_query?;
        self.force(QueryKey::FileSet);
        let active = self.source_files();
        let parses = active
            .iter()
            .filter(|(_, path)| {
                !(path.ends_with(".json") || path.ends_with(".jsonc") || is_javascript(path))
            })
            .map(|&(id, _)| id)
            .collect::<Vec<_>>();
        self.parse_stale_in_parallel(&parses);
        let mut deps = vec![QueryKey::FileSet];
        let mut modules = Vec::with_capacity(parses.len());
        let mut auxiliary = Vec::new();
        for (id, path) in active {
            if path.ends_with(".json") || path.ends_with(".jsonc") || is_javascript(&path) {
                let source = QueryKey::Source(id);
                self.force(source);
                deps.push(source);
                auxiliary.push((
                    path,
                    std::sync::Arc::<[u8]>::from(self.file(id).source.as_bytes()),
                ));
                continue;
            }
            self.force(QueryKey::Parse(id));
            deps.push(QueryKey::Parse(id));
            modules.push((
                path,
                self.file(id)
                    .parse
                    .as_ref()
                    .expect(FORCED)
                    .value
                    .module
                    .clone(),
            ));
        }
        Some(CheckInputs {
            query,
            path: self.file(file).path.clone(),
            modules,
            auxiliary,
            deps,
        })
    }

    fn compile_inputs(
        &mut self,
        file: FileId,
    ) -> (Parsed, Option<FileId>, Option<Epoch>, Vec<QueryKey>) {
        let parse_dep = QueryKey::Parse(file);
        self.force(parse_dep);
        let parsed = self.file(file).parse.as_ref().expect(FORCED).value.clone();
        let mut deps = vec![parse_dep];
        let (checked, checked_epoch) = if self.source_check_query.is_some()
            && self.compile_facts
            && (parsed.module.type_syntax.original_module.is_some()
                || matches!(
                    parsed.module.source_kind,
                    kiln_hir::SourceKind::TypeScript | kiln_hir::SourceKind::Tsx
                )) {
            // A root check also resolves its imported modules. Reuse those current facts
            // for compilation, while keeping diagnostic queries rooted at their own entry.
            // Depending on the supplying check preserves its complete source dependency set.
            // A check whose program contains the module answers for it even when other files
            // of that program have gaps: whether the module's own facts are usable is decided
            // per file (`CheckResult::is_complete_for`), and a check rooted at the module itself
            // would load the same gaps through the same imports. Rooting one check per module
            // instead made a 240-file project with JSX gaps run 166 whole-program checks.
            let path = &self.file(file).path;
            let supplier = self
                .files
                .iter()
                .enumerate()
                .find_map(|(index, state)| {
                    let memo = state.source_check.as_ref()?;
                    (memo.meta.verified_at == self.epoch && memo.value.covers(path))
                        .then_some(FileId(index as u32))
                })
                .unwrap_or(file);
            self.force(QueryKey::SourceCheck(supplier));
            deps.push(QueryKey::SourceCheck(supplier));
            let memo = self.file(supplier).source_check.as_ref().expect(FORCED);
            (Some(supplier), Some(memo.meta.changed_at))
        } else {
            (None, None)
        };
        (parsed, checked, checked_epoch, deps)
    }
    /// The facts of the check that [`Self::compile_inputs`] forced, read in place: they cover
    /// every module of the program, and each module compiles against them.
    fn supplied_check(&self, supplier: Option<FileId>) -> Option<&SourceCheck> {
        supplier.map(|supplier| {
            &self
                .file(supplier)
                .source_check
                .as_ref()
                .expect(FORCED)
                .value
        })
    }

    fn compute_ir(&mut self, file: FileId) -> (Compiled, Vec<QueryKey>) {
        let (parsed, supplier, _, deps) = self.compile_inputs(file);
        let mut result = kiln_ir::compile(&parsed.module);
        if let (Ok(program), Some(checked)) = (&mut result, self.supplied_check(supplier)) {
            kiln_ir::apply_source_types(
                &self.file(file).path,
                &parsed.module,
                checked,
                &checked.checker_identity,
                program,
            );
        }
        (result, deps)
    }
    fn compute_module_ir(&mut self, file: FileId) -> (CompiledModule, Vec<QueryKey>) {
        let (parsed, supplier, _, deps) = self.compile_inputs(file);
        let mut result = kiln_ir::compile_module(&parsed.module);
        if let (Ok(program), Some(checked)) = (&mut result, self.supplied_check(supplier)) {
            kiln_ir::apply_source_types(
                &self.file(file).path,
                &parsed.module,
                checked,
                &checked.checker_identity,
                &mut program.program,
            );
        }
        (result, deps)
    }

    fn compute_summaries(&mut self, file: FileId) -> (Vec<FunctionSummary>, Vec<QueryKey>) {
        let dep = QueryKey::Ir(file);
        self.force(dep);
        let ir = self.file(file).ir.as_ref().expect(FORCED);
        let summaries = match (&ir.value, self.summary_query) {
            (Ok(program), Some(query)) => query(program),
            _ => Vec::new(),
        };
        (summaries, vec![dep])
    }

    fn compute_interface(&mut self, file: FileId) -> (Interface, Vec<QueryKey>) {
        let parse_dep = QueryKey::Parse(file);
        let summaries_dep = QueryKey::Summaries(file);
        self.force(parse_dep);
        self.force(summaries_dep);
        let parsed = self.file(file).parse.as_ref().expect(FORCED);
        let summaries = self.file(file).summaries.as_ref().expect(FORCED);

        // Normalized, because equality of this value is what gates every dependent (adr/0003):
        // declared names are a set, runtime re-export wiring is a sorted descriptor list, and
        // function summaries guard body changes.
        let interface =
            Interface::from_module_and_summaries(&parsed.value.module, summaries.value.clone());
        (interface, vec![parse_dep, summaries_dep])
    }

    fn compute_project(&mut self) -> (Project, Vec<QueryKey>) {
        let mut deps = Vec::with_capacity(self.files.len() + 1);
        deps.push(QueryKey::FileSet);
        self.force(QueryKey::FileSet);

        // FileId reflects interning order, not project identity. Fold active modules in
        // deterministic lexical path order so two clients that send the same logical paths in
        // different orders observe the same project summary. The caller owns path normalization;
        // the exact logical path remains part of the fold because moving a module changes its
        // identity even when its exported names do not.
        let mut active_files: Vec<FileId> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, state)| state.source_set && !is_checker_auxiliary(&state.path))
            .map(|(index, _)| FileId(index as u32))
            .collect();
        active_files.sort_by(|left, right| self.file(*left).path.cmp(&self.file(*right).path));

        // Forcing an interface cannot intern a file, so the length is stable across the loop.
        // Unset interned paths are deliberately absent from the project: interning reserves a
        // reusable FileId, but only a source write makes a file part of the project.
        for file in &active_files {
            let dep = QueryKey::Interface(*file);
            self.force(dep);
            deps.push(dep);
        }

        let project = Project::from_interfaces(active_files.iter().map(|file| {
            let state = self.file(*file);
            (
                state.path.as_str(),
                &state.interface.as_ref().expect(FORCED).value,
            )
        }));
        (project, deps)
    }

    fn meta(&self, key: QueryKey) -> Option<&MemoMeta> {
        match key {
            QueryKey::Source(_) | QueryKey::FileSet => None,
            QueryKey::Parse(file) => self.file(file).parse.as_ref().map(|memo| &memo.meta),
            QueryKey::Ir(file) => self.file(file).ir.as_ref().map(|memo| &memo.meta),
            QueryKey::ModuleIr(file) => self.file(file).module_ir.as_ref().map(|memo| &memo.meta),
            QueryKey::Summaries(file) => self.file(file).summaries.as_ref().map(|memo| &memo.meta),
            QueryKey::SourceCheck(file) => {
                self.file(file).source_check.as_ref().map(|memo| &memo.meta)
            }
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
            QueryKey::Summaries(file) => self
                .file_mut(file)
                .summaries
                .as_mut()
                .map(|memo| &mut memo.meta),
            QueryKey::SourceCheck(file) => self
                .file_mut(file)
                .source_check
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
    /// far worse than a crash. Each active fetch removes itself while the panic unwinds, allowing
    /// a daemon to report this request as failed without poisoning every later query.
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
            QueryKey::Summaries(file) => format!("summaries({})", path(file)),
            QueryKey::SourceCheck(file) => format!("source_check({})", path(file)),
            QueryKey::Interface(file) => format!("interface({})", path(file)),
            QueryKey::Project => "project()".to_string(),
            #[cfg(test)]
            QueryKey::Cyclic(slot) => format!("cyclic({slot})"),
        }
    }
}

fn is_checker_auxiliary(path: &str) -> bool {
    path.ends_with(".json") || path.ends_with(".jsonc")
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

/// A module's IR under a logical identity, with the facts of the check that covers it.
fn module_ir_as(
    path: &str,
    parsed: &Parsed,
    identity: &str,
    checked: Option<&SourceCheck>,
) -> CompiledModule {
    let mut module = parsed.module.as_ref().clone();
    module.module_id = identity.into();
    let mut program = kiln_ir::compile_module(&module);
    if let (Ok(program), Some(checked)) = (&mut program, checked) {
        kiln_ir::apply_source_types(
            path,
            &module,
            checked,
            &checked.checker_identity,
            &mut program.program,
        );
    }
    program
}

/// The stack of a parallel parse worker: the daemon's request-thread stack.
const PARSE_STACK_BYTES: usize = 16 * 1024 * 1024;

/// The pool that parses and compiles modules ahead of a graph walk: the available CPUs, at most
/// eight, bounded by any positive `KILN_JOBS` or `RAYON_NUM_THREADS`. `None` if its threads
/// cannot be created; the walk then computes each module when it asks for it.
fn parse_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let stack = Some(PARSE_STACK_BYTES);
        kiln_jobs::pool("kiln-parse", kiln_jobs::budget(8), stack)
    })
    .as_ref()
}

/// Tiny deterministic frontend used only by this crate's query-engine tests.
///
/// Production callers inject `kiln_frontend_oxc::parse`; keeping the fixture here lets the foundational
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
        type_syntax: kiln_hir::TypeSyntax::default(),
        module_id: "".into(),
        // No source, deliberately. This frontend synthesizes a fixed shape and never reads
        // `text`, so its spans index into nothing and its bytecode is the same whatever the file
        // says. Handing it the text would imply a relationship that does not exist -- and would
        // break the early-cutoff tests below, which edit a file expecting the *same* program
        // back. That expectation is real for this stub and false for a real frontend, where a
        // comment-only edit now does produce a different `Program`: the bytecode is unchanged but
        // the text `Function.prototype.toString` answers with is not.
        source: "".into(),
        static_source_provenance: None,
        source_origin: None,
        source_kind: SourceKind::JavaScript,
        source_goal: kiln_hir::SourceGoal::Module,
        strict: false,
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
        node_runtime_requests: Vec::new(),
    }
}

#[cfg(test)]
fn panic_injecting_frontend(path: &str, text: &str) -> Module {
    if text == "panic during parse" {
        panic!("injected frontend panic");
    }
    fixture_frontend(path, text)
}

#[cfg(test)]
fn linkage_frontend(path: &str, text: &str) -> Module {
    let mut module = fixture_frontend(path, text);
    if let Some(source) = text.lines().find_map(|line| line.strip_prefix("star ")) {
        module.exports.push(Export {
            exported: None,
            origin: ExportOrigin::Star {
                source: source.trim().to_owned(),
            },
            type_only: false,
            span: Span::default(),
            statement_span: Span::default(),
        });
    }
    module
}

impl QueryEngine for MemoEngine {
    fn intern(&mut self, path: &str) -> FileId {
        if let Some(file) = self.by_path.get(path) {
            return *file;
        }
        let file = FileId(self.files.len() as u32);
        self.files.push(FileState {
            path: path.to_string(),
            source: String::new(),
            source_set: false,
            source_changed_at: self.epoch,
            source_revision: self.revision,
            parse: None,
            ir: None,
            module_ir: None,
            module_ir_by_identity: AHashMap::new(),
            summaries: None,
            source_check: None,
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
        let was_set = self.file(file).source_set;
        self.epoch = self.epoch.next();
        self.revision = Revision(self.revision.0 + 1);
        let epoch = self.epoch;
        let revision = self.revision;
        let state = self.file_mut(file);
        state.source = text;
        state.source_set = true;
        state.source_changed_at = epoch;
        state.source_revision = revision;
        // Activating an interned-but-unset path changes the file set just as removal does. Body
        // edits leave this input alone so they can still cut off at the interface query.
        if !was_set {
            self.file_set_changed_at = epoch;
        }
    }

    fn source_files(&self) -> Vec<(FileId, String)> {
        self.files
            .iter()
            .enumerate()
            .filter(|(_, state)| state.source_set)
            .map(|(index, state)| (FileId(index as u32), state.path.clone()))
            .collect()
    }

    fn remove_source(&mut self, file: FileId) {
        let Some(state) = self.files.get(file.0 as usize) else {
            return;
        };
        if !state.source_set {
            return;
        }

        self.epoch = self.epoch.next();
        self.revision = Revision(self.revision.0 + 1);
        let epoch = self.epoch;
        let state = self.file_mut(file);
        // Interned path identity is permanent, but inactive payloads are not. A resident daemon
        // may observe unbounded file churn; retaining String capacity, owned HIR, and two bytecode
        // forms for every deleted path would make authoritative Sync deletion purely logical.
        state.source = String::new();
        state.parse = None;
        state.ir = None;
        state.module_ir = None;
        state.summaries = None;
        state.source_check = None;
        // Keep the small interface memo. Restoring an equal file can still cut off here after its
        // parse is rebuilt, without retaining the heavyweight parse value while it is inactive.
        state.source_set = false;
        state.source_changed_at = epoch;
        self.file_set_changed_at = epoch;
    }

    fn source(&self, file: FileId) -> Option<&str> {
        self.files
            .get(file.0 as usize)
            .and_then(|state| state.source_set.then_some(state.source.as_str()))
    }

    fn source_revision(&self, file: FileId) -> Option<Revision> {
        self.files
            .get(file.0 as usize)
            .and_then(|state| state.source_set.then_some(state.source_revision))
    }

    fn parse(&mut self, file: FileId) -> Parsed {
        self.force(QueryKey::Parse(file));
        self.file(file).parse.as_ref().expect(FORCED).value.clone()
    }

    fn source_check(&mut self, file: FileId) -> SourceCheck {
        self.force(QueryKey::SourceCheck(file));
        self.file(file)
            .source_check
            .as_ref()
            .expect(FORCED)
            .value
            .clone()
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

    fn module_ir_with_identity(&mut self, file: FileId, identity: &str) -> CompiledModule {
        let (parsed, supplier, checked_epoch, _) = self.compile_inputs(file);
        let revision = self.file(file).source_revision;
        if let Some((cached_revision, cached_check_epoch, program)) =
            self.file(file).module_ir_by_identity.get(identity)
            && *cached_revision == revision
            && *cached_check_epoch == checked_epoch
        {
            let program = program.clone();
            self.stats.validated += 1;
            return program;
        }
        let program = module_ir_as(
            &self.file(file).path,
            &parsed,
            identity,
            self.supplied_check(supplier),
        );
        self.stats.ir_computed += 1;
        self.file_mut(file).module_ir_by_identity.insert(
            identity.to_owned(),
            (revision, checked_epoch, program.clone()),
        );
        program
    }

    fn set_compile_facts(&mut self, on: bool) {
        self.compile_facts = on;
    }

    fn begin_source_check(&mut self, file: FileId) {
        if self.source_check_query.is_none() || self.pending_check.is_some() {
            return;
        }
        // Only a check that forcing would recompute: one with no memo, or whose file set or one
        // of whose sources changed since it was verified.
        if let Some(memo) = self.file(file).source_check.as_ref() {
            let verified_at = memo.meta.verified_at;
            if verified_at == self.epoch {
                return;
            }
            let changed = memo.meta.deps.iter().any(|dep| match *dep {
                QueryKey::FileSet => self.file_set_changed_at > verified_at,
                QueryKey::Source(id) | QueryKey::Parse(id) => {
                    self.file(id).source_changed_at > verified_at
                }
                _ => true,
            });
            if !changed {
                return;
            }
        }
        let Some(inputs) = self.source_check_inputs(file) else {
            return;
        };
        let CheckInputs {
            query,
            path,
            modules,
            auxiliary,
            deps,
        } = inputs;
        let spawned = std::thread::Builder::new()
            .name("kiln-source-check-root".into())
            .stack_size(PARSE_STACK_BYTES)
            .spawn(move || query(&path, &modules, &auxiliary));
        if let Ok(handle) = spawned {
            self.pending_check = Some(PendingCheck {
                file,
                epoch: self.epoch,
                deps,
                handle,
            });
        }
    }

    fn prepare_module_irs(&mut self, modules: &[(FileId, String)]) {
        struct Job {
            file: FileId,
            identity: String,
            parsed: Parsed,
            supplier: Option<FileId>,
            checked_epoch: Option<Epoch>,
            revision: Revision,
        }
        let files: Vec<FileId> = modules
            .iter()
            .map(|(file, _)| *file)
            .filter(|&file| self.file(file).source_set)
            .collect();
        self.parse_stale_in_parallel(&files);
        let mut jobs = Vec::new();
        for (file, identity) in modules {
            let file = *file;
            if !self.file(file).source_set {
                continue;
            }
            self.force(QueryKey::Parse(file));
            let module = &self.file(file).parse.as_ref().expect(FORCED).value.module;
            let checks_source = self.source_check_query.is_some()
                && self.compile_facts
                && (module.type_syntax.original_module.is_some()
                    || matches!(
                        module.source_kind,
                        kiln_hir::SourceKind::TypeScript | kiln_hir::SourceKind::Tsx
                    ));
            // A TypeScript module that no current check covers would start a whole-program check
            // of its own. Only a graph that actually reaches it pays for that.
            let path = &self.file(file).path;
            if checks_source
                && !self.files.iter().any(|state| {
                    state.source_check.as_ref().is_some_and(|memo| {
                        memo.meta.verified_at == self.epoch && memo.value.covers(path)
                    })
                })
            {
                continue;
            }
            let (parsed, supplier, checked_epoch, _) = self.compile_inputs(file);
            let revision = self.file(file).source_revision;
            if self
                .file(file)
                .module_ir_by_identity
                .get(identity.as_str())
                .is_some_and(|(cached_revision, cached_check_epoch, _)| {
                    *cached_revision == revision && *cached_check_epoch == checked_epoch
                })
            {
                continue;
            }
            jobs.push(Job {
                file,
                identity: identity.clone(),
                parsed,
                supplier,
                checked_epoch,
                revision,
            });
        }
        if jobs.is_empty() {
            return;
        }
        let this = &*self;
        let compile = |job: &Job| {
            // A panic is left for the graph walk's own request to report.
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                module_ir_as(
                    &this.file(job.file).path,
                    &job.parsed,
                    &job.identity,
                    this.supplied_check(job.supplier),
                )
            }))
            .ok()
        };
        let programs: Vec<Option<CompiledModule>> = match parse_pool() {
            Some(pool) => pool.install(|| jobs.par_iter().map(compile).collect()),
            None => jobs.iter().map(compile).collect(),
        };
        for (job, program) in jobs.into_iter().zip(programs) {
            let Some(program) = program else {
                continue;
            };
            self.stats.ir_computed += 1;
            self.file_mut(job.file)
                .module_ir_by_identity
                .insert(job.identity, (job.revision, job.checked_epoch, program));
        }
    }

    fn function_summaries(&mut self, file: FileId) -> Vec<FunctionSummary> {
        self.force(QueryKey::Summaries(file));
        self.file(file)
            .summaries
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

    /// The program of a fixture check is the modules its types describe.
    fn with_files(mut check: SourceCheck) -> SourceCheck {
        let mut files: Vec<(String, bool)> = check
            .types
            .iter()
            .map(|ty| (ty.path.clone(), true))
            .collect();
        files.sort();
        files.dedup();
        check.files = files;
        check
    }

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
                ir_computed: 2,
                interface_computed: 2,
                project_computed: 1,
                validated: 2,
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
                ir_computed: 2,
                interface_computed: 2,
                project_computed: 1,
                validated: 4,
                early_cutoff: 0,
            },
            "already-verified memos should be served straight from cache"
        );
    }

    #[test]
    fn project_rollup_is_independent_of_interning_order() {
        let (mut forward, _) = engine(&[("a.js", A_V1), ("b.js", B_V1)]);
        let (mut reverse, _) = engine(&[("b.js", B_V1), ("a.js", A_V1)]);

        assert_eq!(
            forward.project(),
            reverse.project(),
            "FileId assignment must not leak into the observable project summary"
        );
    }

    #[test]
    fn project_constructor_is_order_independent_even_for_duplicate_paths() {
        let alpha = Interface::from_exports(vec!["alpha".to_owned()]);
        let beta = Interface::from_exports(vec!["beta".to_owned()]);

        let forward = Project::from_interfaces([("same.js", &alpha), ("same.js", &beta)]);
        let reverse = Project::from_interfaces([("same.js", &beta), ("same.js", &alpha)]);
        assert_eq!(forward, reverse);
    }

    #[test]
    fn same_declared_exports_with_different_star_targets_change_project_identity() {
        let mut left = fixture_frontend("index.js", "export const value = 1;\n");
        left.exports.push(Export {
            exported: None,
            origin: ExportOrigin::Star {
                source: "./left.js".to_owned(),
            },
            type_only: false,
            span: Span::default(),
            statement_span: Span::default(),
        });
        let mut right = left.clone();
        right.exports.last_mut().expect("star export").origin = ExportOrigin::Star {
            source: "./right.js".to_owned(),
        };

        let left_parsed = Parsed::new("left", left);
        let right_parsed = Parsed::new("right", right);
        assert_eq!(left_parsed.exports, right_parsed.exports);
        let left_linkage = Linkage::from_module(&left_parsed.module);
        let right_linkage = Linkage::from_module(&right_parsed.module);
        assert_ne!(left_linkage, right_linkage);

        let left_interface =
            Interface::from_exports_and_linkage(left_parsed.exports.clone(), left_linkage);
        let right_interface =
            Interface::from_exports_and_linkage(right_parsed.exports.clone(), right_linkage);
        assert_ne!(
            left_interface.interface_hash,
            right_interface.interface_hash
        );
        let left_project = Project::from_interfaces([("index.js", &left_interface)]);
        let right_project = Project::from_interfaces([("index.js", &right_interface)]);
        assert_eq!(left_project.exports, right_project.exports);
        assert_ne!(left_project.interface_hash, right_project.interface_hash);
        assert_ne!(left_project.identity_digest, right_project.identity_digest);
    }

    #[test]
    fn body_edit_keeps_project_identity_when_linkage_is_unchanged() {
        let mut db = MemoEngine::new(linkage_frontend);
        let file = db.intern("index.js");
        db.set_source(file, "star ./dep.js\nexport const value = 1;\n".to_owned());
        let before = db.project();
        db.reset_stats();

        db.set_source(file, "star ./dep.js\nexport const value = 2;\n".to_owned());
        let after = db.project();

        assert_eq!(after, before);
        assert_eq!(db.stats().project_computed, 0);
        assert_eq!(db.stats().early_cutoff, 2);
    }

    #[test]
    fn changing_a_star_target_recomputes_project_without_changing_declared_exports() {
        let mut db = MemoEngine::new(linkage_frontend);
        let file = db.intern("index.js");
        db.set_source(file, "star ./left.js\nexport const value = 1;\n".to_owned());
        let before = db.project();
        db.reset_stats();

        db.set_source(
            file,
            "star ./right.js\nexport const value = 1;\n".to_owned(),
        );
        let after = db.project();

        assert_eq!(after.exports, before.exports);
        assert_eq!(after.modules, before.modules);
        assert_ne!(after.interface_hash, before.interface_hash);
        assert_ne!(after.identity_digest, before.identity_digest);
        assert_eq!(db.stats().project_computed, 1);
        assert_eq!(db.stats().early_cutoff, 1);
    }

    #[test]
    fn inactive_interned_paths_do_not_affect_project_identity() {
        let mut with_gap = MemoEngine::new(fixture_frontend);
        let a = with_gap.intern("a.js");
        with_gap.set_source(a, A_V1.to_owned());
        with_gap.intern("reserved.js");
        let b = with_gap.intern("b.js");
        with_gap.set_source(b, B_V1.to_owned());

        let (mut without_gap, _) = engine(&[("a.js", A_V1), ("b.js", B_V1)]);
        assert_eq!(with_gap.project(), without_gap.project());
    }

    #[test]
    fn module_path_is_part_of_project_identity() {
        let (mut original, _) = engine(&[("a.js", A_V1)]);
        let (mut renamed, _) = engine(&[("nested/renamed.js", A_V1)]);
        let original = original.project();
        let renamed = renamed.project();

        assert_eq!(original.modules, renamed.modules);
        assert_eq!(original.exports, renamed.exports);
        assert_ne!(
            original.interface_hash, renamed.interface_hash,
            "renaming a module must change the project identity"
        );
    }

    #[test]
    fn strong_project_identity_does_not_trust_weak_interface_summaries() {
        let alpha = Interface {
            interface_hash: 7,
            exports: vec!["alpha".to_owned()],
            linkage: Linkage::empty(),
            summaries: Vec::new(),
            summaries_digest: Digest::default(),
        };
        let beta = Interface {
            interface_hash: 7,
            exports: vec!["beta".to_owned()],
            linkage: Linkage::empty(),
            summaries: Vec::new(),
            summaries_digest: Digest::default(),
        };

        let alpha_project = Project::from_interfaces([("same.js", &alpha)]);
        let beta_project = Project::from_interfaces([("same.js", &beta)]);
        assert_eq!(alpha_project.interface_hash, beta_project.interface_hash);
        assert_ne!(alpha_project.identity_digest, beta_project.identity_digest);
        assert_ne!(alpha_project, beta_project);
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
    /// project. Parse must recompute (the text really did change), the interface and linkage
    /// recompute but produce equal values and so cut off, and the rollup is merely revalidated.
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
                ir_computed: 1,
                interface_computed: 1,
                project_computed: 0,
                validated: 10,
                early_cutoff: 2,
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
                ir_computed: 1,
                interface_computed: 1,
                project_computed: 1,
                validated: 10,
                early_cutoff: 1,
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

    /// Re-sending identical bytes is not an edit and must not move any input clock.
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

    #[test]
    fn source_revision_tracks_each_files_last_real_change() {
        let (mut db, files) = engine(&[("a.js", A_V1), ("b.js", B_V1)]);
        assert_eq!(db.source_revision(files[0]), Some(Revision(1)));
        assert_eq!(db.source_revision(files[1]), Some(Revision(2)));

        db.set_source(files[0], A_BODY_EDIT.to_owned());
        assert_eq!(db.source_revision(files[0]), Some(Revision(3)));
        assert_eq!(db.source_revision(files[1]), Some(Revision(2)));
        db.set_source(files[0], A_BODY_EDIT.to_owned());
        assert_eq!(db.source_revision(files[0]), Some(Revision(3)));

        db.remove_source(files[0]);
        assert_eq!(db.source_revision(files[0]), None);
        db.set_source(files[0], A_V1.to_owned());
        assert_eq!(db.source_revision(files[0]), Some(Revision(5)));
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
            2,
            "the interface recomputed to an equal value"
        );
    }

    /// Interning reserves identity but does not activate a source. The project remains warm until
    /// the first write changes the active file-set input.
    #[test]
    fn interning_is_invisible_to_the_project_until_the_source_is_written() {
        let (mut db, _) = engine(&[("a.js", A_V1)]);
        assert_eq!(db.project().modules, 1);
        db.reset_stats();
        let revision = db.revision();

        let b = db.intern("b.js");
        assert_eq!(db.revision(), revision, "interning is not a source write");
        assert_eq!(db.project().modules, 1);
        assert_eq!(
            db.project().exports,
            1,
            "b.js is absent until it is written"
        );
        assert_eq!(db.stats().project_computed, 0);

        db.set_source(b, B_V1.to_string());
        assert_eq!(db.project().exports, 2);
        assert_eq!(db.stats().project_computed, 1);
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
    fn source_files_lists_only_active_sources() {
        let (mut db, files) = engine(&[("a.js", A_V1)]);
        let b = db.intern("b.js");

        assert_eq!(db.source_files(), vec![(files[0], "a.js".to_owned())]);
        db.set_source(b, B_V1.to_owned());
        assert_eq!(
            db.source_files(),
            vec![(files[0], "a.js".to_owned()), (b, "b.js".to_owned())]
        );
    }

    #[test]
    fn removing_a_source_invalidates_project_and_excludes_it_from_rollups() {
        let (mut db, files) = cold_project();
        let file = files[0];
        db.reset_stats();

        db.remove_source(file);

        assert_eq!(db.source(file), None);
        assert_eq!(db.source_files(), vec![(files[1], "b.js".to_owned())]);
        let project = db.project();
        assert_eq!(project.modules, 1);
        assert_eq!(project.exports, 1);
        assert_eq!(db.stats().revision, 3);
        assert_eq!(db.stats().project_computed, 1);
        assert_eq!(db.stats().parse_computed, 0);
        assert_eq!(db.stats().interface_computed, 0);
    }

    #[test]
    fn removing_an_absent_source_is_idempotent() {
        let (mut db, files) = engine(&[("a.js", A_V1)]);
        db.remove_source(files[0]);

        let revision = db.revision();
        let stats = db.stats();
        db.remove_source(files[0]);
        assert_eq!(db.revision(), revision);
        assert_eq!(db.stats(), stats);

        let unset = db.intern("unset.js");
        let revision = db.revision();
        db.remove_source(unset);
        assert_eq!(db.revision(), revision);

        db.remove_source(FileId(7));
        assert_eq!(db.revision(), revision);
    }

    #[test]
    fn removing_a_source_releases_its_bytes_hir_and_bytecode() {
        let source = format!("// {}\n{A_V1}", "large deleted payload ".repeat(4_096));
        let (mut db, files) = engine(&[("large.js", &source)]);
        let file = files[0];
        db.parse(file);
        let _ = db.ir(file);
        let _ = db.module_ir(file);
        db.interface(file);
        assert!(db.file(file).source.capacity() >= source.len());

        db.remove_source(file);

        let state = db.file(file);
        assert_eq!(state.source.capacity(), 0);
        assert!(state.parse.is_none());
        assert!(state.ir.is_none());
        assert!(state.module_ir.is_none());
        assert!(
            state.interface.is_some(),
            "interface metadata stays reusable"
        );
        assert_eq!(db.intern("large.js"), file, "path identity is retained");
    }

    #[test]
    fn removed_source_can_be_restored_with_the_same_file_id() {
        let (mut db, files) = cold_project();
        let file = files[0];
        db.remove_source(file);
        db.project();
        db.reset_stats();

        assert_eq!(db.intern("a.js"), file);
        db.set_source(file, A_V1.to_owned());
        assert_eq!(db.source(file), Some(A_V1));
        assert_eq!(db.project().modules, 2);
        assert_eq!(db.project().exports, 2);
        assert_eq!(
            db.source_files(),
            vec![(file, "a.js".to_owned()), (files[1], "b.js".to_owned())]
        );
        assert_eq!(db.stats().revision, 4);
        assert_eq!(db.stats().parse_computed, 1);
        assert_eq!(db.stats().interface_computed, 1);
        assert_eq!(db.stats().early_cutoff, 1);
        assert_eq!(db.stats().project_computed, 1);
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
        assert!(
            db.active.is_empty(),
            "cycle unwinding must restore the stack"
        );
    }

    #[test]
    fn a_frontend_panic_does_not_turn_the_next_request_into_a_false_cycle() {
        let mut db = MemoEngine::new(panic_injecting_frontend);
        let file = db.intern("recover.js");
        db.set_source(file, "panic during parse".to_owned());

        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| db.parse(file)));
        assert!(failed.is_err(), "the injected frontend must panic");
        assert!(
            db.active.is_empty(),
            "a failed query must leave no active key"
        );

        db.set_source(file, A_V1.to_owned());
        assert_eq!(db.parse(file).exports, ["alpha"]);
        assert_eq!(db.interface(file).exports, ["alpha"]);
        assert_eq!(db.project().modules, 1);
        assert!(db.active.is_empty());
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
        assert_eq!(db.stats().early_cutoff, 2);
    }

    #[test]
    fn empty_project_is_well_defined() {
        let mut db = MemoEngine::new(fixture_frontend);
        assert_eq!(
            db.project(),
            Project::from_interfaces(std::iter::empty::<(&str, &Interface)>())
        );
        assert_eq!(db.revision(), Revision(0));
    }
    #[test]
    fn an_incomplete_check_supplies_every_module_its_program_covers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn frontend(path: &str, text: &str) -> Module {
            let mut module = fixture_frontend(path, text);
            module.source = text.into();
            module.source_kind = SourceKind::TypeScript;
            module
        }
        fn checker(
            _entry: &str,
            modules: &[(String, std::sync::Arc<Module>)],
            _auxiliary: &[(String, std::sync::Arc<[u8]>)],
        ) -> SourceCheck {
            CALLS.fetch_add(1, Ordering::SeqCst);
            // A gap in the entry leaves the check incomplete; the module it imports is not.
            let mut files: Vec<(String, bool)> = modules
                .iter()
                .map(|(path, _)| (path.clone(), path != "/main.ts"))
                .collect();
            files.sort();
            SourceCheck {
                checker_identity: "fixture-v1".into(),
                complete: false,
                types: Vec::new(),
                diagnostics: Vec::new(),
                files,
            }
        }
        CALLS.store(0, Ordering::SeqCst);
        let mut db = MemoEngine::new(frontend).with_source_check_query(checker);
        let root = db.intern("/main.ts");
        let dependency = db.intern("/dep.ts");
        db.set_source(root, "import { x } from './dep';".into());
        db.set_source(dependency, "export const x = 1;".into());
        let checked = db.source_check(root);
        assert!(!checked.complete);
        assert!(checked.is_complete_for("/dep.ts"));
        assert!(!checked.is_complete_for("/main.ts"));
        // Compiling either module reads the entry's check instead of checking from itself.
        db.module_ir(root);
        db.module_ir(dependency);
        db.module_ir_with_identity(dependency, "dep.ts");
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn source_check_tracks_alias_edits_file_set_and_reuses_unchanged_inputs() {
        use kiln_hir::source_types::{SourceType, TypeClass, TypeRole};
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn frontend(path: &str, text: &str) -> Module {
            let mut module = fixture_frontend(path, text);
            module.source = text.into();
            module.source_kind = SourceKind::TypeScript;
            module
        }
        fn checker(
            entry: &str,
            modules: &[(String, std::sync::Arc<Module>)],
            _auxiliary: &[(String, std::sync::Arc<[u8]>)],
        ) -> SourceCheck {
            CALLS.fetch_add(1, Ordering::SeqCst);
            let is_string = modules.iter().any(|(_, m)| m.source.contains("string"));
            with_files(SourceCheck {
                checker_identity: "fixture-v1".into(),
                complete: true,
                types: vec![SourceType {
                    path: entry.into(),
                    span: Span::new(0, 1),
                    role: TypeRole::Annotation,
                    display: if is_string { "string" } else { "number" }.into(),
                    class: if is_string {
                        TypeClass::String
                    } else {
                        TypeClass::Number
                    },
                }],
                diagnostics: Vec::new(),
                files: Vec::new(),
            })
        }
        CALLS.store(0, Ordering::SeqCst);
        let mut db = MemoEngine::new(frontend).with_source_check_query(checker);
        let root = db.intern("/main.ts");
        let dependency = db.intern("/alias.ts");
        db.set_source(root, "import type { Scalar } from './alias';".into());
        db.set_source(dependency, "type Scalar = number;".into());
        let old = db.source_check(root);
        assert_eq!(old.types[0].class, TypeClass::Number);
        assert_eq!(db.source_check(root), old);
        db.set_source(dependency, "type Scalar = number;".into());
        assert_eq!(db.source_check(root), old);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        db.set_source(dependency, "type Scalar = string;".into());
        assert_eq!(db.source_check(root).types[0].class, TypeClass::String);
        assert_eq!(old.types[0].class, TypeClass::Number);
        db.remove_source(dependency);
        assert_eq!(db.source_check(root).types[0].class, TypeClass::Number);
        assert_eq!(CALLS.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn checker_json_inputs_are_dependencies_but_not_project_modules() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn frontend(path: &str, text: &str) -> Module {
            let mut module = fixture_frontend(path, text);
            module.source = text.into();
            module.source_kind = if path.ends_with(".ts") {
                SourceKind::TypeScript
            } else {
                SourceKind::JavaScript
            };
            module
        }
        fn checker(
            _entry: &str,
            modules: &[(String, std::sync::Arc<Module>)],
            auxiliary: &[(String, std::sync::Arc<[u8]>)],
        ) -> SourceCheck {
            CALLS.fetch_add(1, Ordering::SeqCst);
            assert_eq!(modules.len(), 2);
            assert!(modules.iter().any(|(path, _)| path == "/project/virtual"));
            assert!(
                auxiliary
                    .iter()
                    .any(|(path, _)| path == "/project/tsconfig.jsonc")
            );
            with_files(SourceCheck {
                checker_identity: "json-auxiliary-v1".into(),
                complete: true,
                types: Vec::new(),
                diagnostics: Vec::new(),
                files: Vec::new(),
            })
        }

        CALLS.store(0, Ordering::SeqCst);
        let mut db = MemoEngine::new(frontend).with_source_check_query(checker);
        let entry = db.intern("/project/main.ts");
        let virtual_source = db.intern("/project/virtual");
        let config = db.intern("/project/tsconfig.jsonc");
        db.set_source(entry, "export const x = 1;".into());
        db.set_source(virtual_source, "export const y = 2;".into());
        db.set_source(config, "{\"compilerOptions\":{}}".into());

        db.source_check(entry);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(db.project().modules, 2);
        assert_eq!(
            db.stats().parse_computed,
            2,
            "config must not be parsed as JS/TS"
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);

        db.set_source(config, "{\"compilerOptions\":{\"strict\":true}}".into());
        db.source_check(entry);
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            2,
            "config bytes invalidate checking"
        );
        assert_eq!(
            db.project().modules,
            2,
            "config is excluded from project inventory"
        );
        assert_eq!(
            db.stats().parse_computed,
            2,
            "config remains auxiliary after edits"
        );
    }

    #[test]
    fn identity_module_compile_rechecks_and_attaches_project_contracts() {
        use kiln_hir::source_types::{SourceType, TypeClass, TypeRole};
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn checker(
            entry: &str,
            modules: &[(String, std::sync::Arc<Module>)],
            _auxiliary: &[(String, std::sync::Arc<[u8]>)],
        ) -> SourceCheck {
            CALLS.fetch_add(1, Ordering::SeqCst);
            let target = modules
                .iter()
                .find(|(path, _)| path == entry)
                .unwrap()
                .1
                .as_ref();
            let signature = target
                .type_syntax
                .signatures
                .iter()
                .find(|site| site.has_body)
                .unwrap();
            let parameter = signature.signature.parameters.first().unwrap();
            let is_string = modules
                .iter()
                .any(|(_, module)| module.source.contains("type Scalar = string"));
            with_files(SourceCheck {
                checker_identity: "identity-contract-v1".into(),
                complete: true,
                types: vec![SourceType {
                    path: entry.into(),
                    span: parameter.span,
                    role: TypeRole::Parameter,
                    display: if is_string { "string" } else { "number" }.into(),
                    class: if is_string {
                        TypeClass::String
                    } else {
                        TypeClass::Number
                    },
                }],
                diagnostics: Vec::new(),
                files: Vec::new(),
            })
        }

        fn contract_frontend(path: &str, text: &str) -> Module {
            let mut module = fixture_frontend(path, text);
            module.source = text.into();
            module.source_kind = SourceKind::TypeScript;
            if path.ends_with("main.ts") {
                let param_span = Span::new(32, 37);
                let function_span = Span::new(0, 65);
                let parameter = kiln_hir::TypeSignatureParameter {
                    name: Some("value".into()),
                    pattern: Some(Box::new(kiln_hir::Pattern::Identifier("value".into()))),
                    pattern_span: param_span,
                    span: param_span,
                    ty: None,
                    optional: false,
                    rest: false,
                    default: None,
                };
                let function = kiln_hir::Function {
                    name: Some("echo".into()),
                    inferred_name: None,
                    binds_own_name: true,
                    arrow: false,
                    generator: false,
                    asynchronous: false,
                    parameters: vec![kiln_hir::Parameter {
                        pattern: kiln_hir::Pattern::Identifier("value".into()),
                        type_annotation: None,
                        span: param_span,
                    }],
                    rest: None,
                    return_type: None,
                    body: vec![kiln_hir::Statement {
                        kind: kiln_hir::StatementKind::Return(Some(kiln_hir::Expression {
                            kind: kiln_hir::ExpressionKind::Identifier("value".into()),
                            span: param_span,
                        })),
                        span: param_span,
                    }],
                    strict: false,
                    span: function_span,
                };
                module.body = vec![kiln_hir::Statement {
                    kind: kiln_hir::StatementKind::Function(Box::new(function)),
                    span: function_span,
                }];
                module.type_syntax.signatures = vec![kiln_hir::FunctionSignatureSite {
                    owner_span: function_span,
                    signature: kiln_hir::TypeSignature {
                        span: function_span,
                        type_parameters: Vec::new(),
                        this_parameter: None,
                        parameters: vec![parameter],
                        rest: false,
                        return_type: None,
                    },
                    has_body: true,
                    body_start: None,
                }];
            }
            module
        }

        CALLS.store(0, Ordering::SeqCst);
        let mut db = MemoEngine::new(contract_frontend).with_source_check_query(checker);
        let entry = db.intern("/identity/main.ts");
        let alias = db.intern("/identity/alias.ts");
        db.set_source(
            entry,
            "import type { Scalar } from './alias'; export function echo(value: Scalar) { return value; }".into(),
        );
        db.set_source(alias, "export type Scalar = number;".into());

        let first = db.module_ir_with_identity(entry, "/logical/identity-main");
        let first = first.expect("compile initial module");
        assert_eq!(
            first.program.functions[0].parameter_type(0),
            Some(kiln_ir::TypeAnnotation::Number)
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        let warm = db.module_ir_with_identity(entry, "/logical/identity-main");
        assert_eq!(warm, Ok(first.clone()));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);

        db.set_source(alias, "export type Scalar = string;".into());
        let updated = db
            .module_ir_with_identity(entry, "/logical/identity-main")
            .expect("compile changed alias");
        assert_eq!(
            updated.program.functions[0].parameter_type(0),
            Some(kiln_ir::TypeAnnotation::String)
        );
        assert_eq!(
            first.program.functions[0].parameter_type(0),
            Some(kiln_ir::TypeAnnotation::Number)
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn compilation_reuses_imported_facts_without_reusing_entry_diagnostics() {
        use kiln_hir::source_types::{SourceDiagnostic, SourceType, TypeClass, TypeRole};
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn frontend(path: &str, text: &str) -> Module {
            let mut module = fixture_frontend(path, text);
            module.source = text.into();
            module.source_kind = SourceKind::TypeScript;
            module
        }
        fn checker(
            entry: &str,
            modules: &[(String, std::sync::Arc<Module>)],
            _auxiliary: &[(String, std::sync::Arc<[u8]>)],
        ) -> SourceCheck {
            CALLS.fetch_add(1, Ordering::SeqCst);
            with_files(SourceCheck {
                checker_identity: "shared-facts-v1".into(),
                complete: true,
                types: modules
                    .iter()
                    .map(|(path, _)| SourceType {
                        path: path.clone(),
                        span: Span::new(0, 1),
                        role: TypeRole::Annotation,
                        display: "number".into(),
                        class: TypeClass::Number,
                    })
                    .collect(),
                diagnostics: vec![SourceDiagnostic {
                    related: Vec::new(),
                    path: Some(entry.into()),
                    span: None,
                    code: None,
                    message: "entry diagnostic".into(),
                }],
                files: Vec::new(),
            })
        }
        CALLS.store(0, Ordering::SeqCst);
        let mut db = MemoEngine::new(frontend).with_source_check_query(checker);
        let root = db.intern("/main.ts");
        let dependency = db.intern("/dep.ts");
        db.set_source(root, "main".into());
        db.set_source(dependency, "dependency".into());
        db.source_check(root);
        assert!(db.ir(dependency).is_ok());
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        let own_check = db.source_check(dependency);
        assert_eq!(own_check.diagnostics[0].path.as_deref(), Some("/dep.ts"));
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
        db.set_source(root, "changed ambient input".into());
        assert!(db.ir(dependency).is_ok());
        assert_eq!(CALLS.load(Ordering::SeqCst), 3);
    }
}
