//! Wire contract between the `kiln` CLI and the compiler daemon.
//!
//! Deliberately tiny and dependency-light: this crate is on the rebuild path of everything
//! else, and design.md §12 asks that foundational crates stay small and stable. Additive
//! changes only once the daemon is real - a client and daemon of different versions must be
//! able to detect the mismatch rather than misinterpret each other, hence `PROTOCOL_VERSION`.

use serde::{Deserialize, Serialize};

/// Bumped on any non-additive change. The daemon refuses a client that does not match.
pub const PROTOCOL_VERSION: u32 = 1;

/// A source file, interned by the daemon. Stable for the lifetime of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileId(pub u32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Liveness plus version negotiation.
    Ping { protocol: u32 },
    /// Replace a file's contents. Bumps the database revision.
    SetSource { path: String, text: String },
    /// Run the query pipeline for a file and return its summary.
    Check { path: String },
    /// Whole-project summary, which depends on every module interface.
    Project,
    /// Recompute counters, so callers can assert what was and was not invalidated.
    Stats,
    /// Reset all counters without discarding cached values.
    ResetStats,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Pong { protocol: u32, pid: u32 },
    Checked(ModuleSummary),
    Project(ProjectSummary),
    Stats(Stats),
    Ok,
    Error { message: String },
}

/// What the pipeline currently knows about one module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleSummary {
    pub path: String,
    pub file: FileId,
    /// Changes whenever anything in the file changes.
    pub implementation_hash: u64,
    /// Changes only when externally relevant semantics change (design.md §3).
    pub interface_hash: u64,
    pub exports: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub modules: usize,
    pub exports: usize,
    pub interface_hash: u64,
}

/// Per-query recompute counts. The point of the whole design is that these stay flat when
/// an edit does not cross an interface boundary, so they are a first-class observable, not
/// debug output: the tests and the timing harness both assert on them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub revision: u64,
    pub parse_computed: u64,
    pub interface_computed: u64,
    pub project_computed: u64,
    /// Times a query was validated without recomputing (dependencies unchanged).
    pub validated: u64,
    /// Times a recompute produced an identical value, cutting off its dependents.
    pub early_cutoff: u64,
}
