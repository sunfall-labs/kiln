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

/// A source file interned by the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

/// Monotonic clock. Every input write bumps it; nothing else does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Revision(pub u64);

/// The result of parsing a module. A stand-in until the Oxc frontend lands (design.md §14.4);
/// what matters now is that it is derived, cached, and depended upon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// Everything about the module, including things no dependent can observe.
    pub implementation_hash: u64,
    pub exports: Vec<String>,
    pub lines: usize,
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

    /// Replace an input. Bumps the revision even if the text is unchanged; the engine is
    /// expected to cut that off downstream rather than pretend the write did not happen.
    fn set_source(&mut self, file: FileId, text: String);

    fn source(&self, file: FileId) -> Option<&str>;

    fn parse(&mut self, file: FileId) -> Parsed;
    fn interface(&mut self, file: FileId) -> Interface;
    fn project(&mut self) -> Project;

    fn revision(&self) -> Revision;
    fn stats(&self) -> Stats;
    fn reset_stats(&mut self);
}
