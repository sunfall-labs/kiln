//! The incremental query database (design.md §2, §3).
//!
//! Compilation is modelled as memoized queries over versioned inputs. The engine lives behind
//! the [`QueryEngine`] trait so the compiler is never tied to one implementation - see
//! docs/adr/0001 for why that boundary exists and docs/adr/0002 for what we expect of a
//! candidate backend such as salsa.

mod engine;
mod hash;

pub use engine::MemoEngine;
pub use hash::content_hash;
pub use kiln_hir::Module as HirModule;
pub use kiln_ir::{CompileDiagnostic, ModuleProgram, Program};

/// Memoized result of lowering owned HIR to parser-independent bytecode.
pub type Compiled = Result<Program, Vec<CompileDiagnostic>>;
/// Memoized linkage-aware bytecode for graph execution.
pub type CompiledModule = Result<ModuleProgram, Vec<CompileDiagnostic>>;

/// A parser/lowerer injected into the memo engine.
///
/// Keeping this as a plain function pointer makes the engine `Send`, cheap to construct, and
/// independent of Oxc. Production injects `frontend_oxc::parse`; database unit tests inject a
/// tiny deterministic fixture frontend.
pub type Frontend = fn(path: &str, source: &str) -> HirModule;

/// A source file interned by the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

/// Monotonic clock. Every source addition, replacement, or removal bumps it; nothing else does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Revision(pub u64);

/// The owned result of parsing and lowering one module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// Everything about the module, including things no dependent can observe.
    pub implementation_hash: u64,
    /// Convenience projection used by the current interface query. The complete owned module
    /// HIR is retained below for subsequent lowering and interpreter queries.
    pub exports: Vec<String>,
    pub lines: usize,
    pub module: HirModule,
}

impl Parsed {
    pub fn new(source: &str, module: HirModule) -> Self {
        Self {
            implementation_hash: content_hash(source),
            exports: module.runtime_export_names(),
            lines: source.lines().count(),
            module,
        }
    }
}

/// The externally visible surface of a module.
///
/// This is the early-cutoff point. A dependent is recomputed only when this value *changes*,
/// which is how design.md §3's "interface hash" is realised without maintaining a second hash
/// by hand - see docs/adr/0003.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub interface_hash: u64,
    pub exports: Vec<String>,
}

/// Whole-project rollup. Depends on every module interface and nothing else, so an edit that
/// does not change any interface must not recompute it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub modules: usize,
    pub exports: usize,
    pub interface_hash: u64,
}

/// Recompute accounting. Not debug output: the tests and the timing harness assert on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub revision: u64,
    pub parse_computed: u64,
    pub ir_computed: u64,
    pub interface_computed: u64,
    pub project_computed: u64,
    pub validated: u64,
    pub early_cutoff: u64,
}

/// The compiler's view of incremental state.
///
/// Implementations must guarantee:
/// 1. A query's value is recomputed only if a transitive dependency's value *changed*.
/// 2. Recomputing a query that yields an equal value does not invalidate its dependents.
/// 3. Reads are pure: the same query at the same revision returns an equal value.
pub trait QueryEngine {
    /// Intern a path, creating a [`FileId`] if new.
    fn intern(&mut self, path: &str) -> FileId;

    /// Replace an input.
    ///
    /// Writing byte-identical text is not an edit: the revision does not move, nothing is
    /// invalidated, and no dependent recomputes. A caller may therefore re-send state it is
    /// not certain the engine already holds without paying for it, which is what makes a
    /// stateless client safe to point at a warm daemon.
    fn set_source(&mut self, file: FileId, text: String);

    /// Return the interned files that currently have source text, together with their paths.
    ///
    /// Interning a path does not make it an active source: callers can use this snapshot to
    /// reconcile an authoritative source set and call [`QueryEngine::remove_source`] for paths
    /// that disappeared.
    fn source_files(&self) -> Vec<(FileId, String)>;

    /// Remove a source while retaining its interned [`FileId`] for a later restore.
    ///
    /// Removing an already-absent source is a no-op. Implementations must invalidate queries
    /// that read either this source or the active file set.
    fn remove_source(&mut self, file: FileId);

    fn source(&self, file: FileId) -> Option<&str>;

    fn parse(&mut self, file: FileId) -> Parsed;
    fn ir(&mut self, file: FileId) -> Compiled;
    /// Linkage-aware lowering. Engines may override this to memoize it independently; the
    /// default preserves compatibility for small test/reference engines.
    fn module_ir(&mut self, file: FileId) -> CompiledModule {
        kiln_ir::compile_module(&self.parse(file).module)
    }
    fn interface(&mut self, file: FileId) -> Interface;
    fn project(&mut self) -> Project;

    fn revision(&self) -> Revision;
    fn stats(&self) -> Stats;
    fn reset_stats(&mut self);
}
