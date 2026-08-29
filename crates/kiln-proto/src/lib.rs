//! Wire contract between the `kiln` CLI and the compiler daemon.
//!
//! Deliberately tiny and dependency-light: this crate is on the rebuild path of everything
//! else, and design.md §12 asks that foundational crates stay small and stable. Additive
//! changes only once the daemon is real - a client and daemon of different versions must be
//! able to detect the mismatch rather than misinterpret each other, hence `PROTOCOL_VERSION`.

use serde::{Deserialize, Serialize};

/// Bumped on any non-additive change. The daemon refuses a client that does not match.
pub const PROTOCOL_VERSION: u32 = 7;

/// A file as the client sees it: the path the daemon knows it by, and `kiln_db::content_hash`
/// of its text.
///
/// The hash is the point of [`Request::Sync`]. A stateless client can afford to name every file
/// in the project on every command; it cannot afford to send every file's contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceHash {
    pub path: String,
    pub hash: u64,
}

/// A source file, interned by the daemon. Stable for the lifetime of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileId(pub u32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Liveness plus version negotiation.
    Ping {
        protocol: u32,
    },
    /// Replace a file's contents. Bumps the database revision.
    SetSource {
        path: String,
        text: String,
    },
    /// The client's whole view of the project, named but not sent.
    ///
    /// Answered with [`Response::Needed`]: the subset whose contents the daemon does not
    /// already hold, which is all the client then has to push. Without this a stateless client
    /// re-sends every source on every command -- on a 320-file project that was ~370KB of JSON
    /// to answer a question whose response was "nothing changed".
    Sync {
        files: Vec<SourceHash>,
    },
    /// Run the query pipeline for a file and return its summary.
    Check {
        path: String,
    },
    /// Compile the cached HIR and execute one module through the interpreter.
    Run {
        path: String,
        /// Project root used to resolve this module's explicit relative imports.
        project_root: String,
    },
    /// Emit, compile, and link one module through the native backend.
    NativeBuild {
        path: String,
        project_root: String,
        /// Requested output path. The client supplies the default so daemon and one-shot
        /// invocations produce the same artifact location.
        output: Option<String>,
        profile: NativeProfile,
    },
    /// Emit, compile, link, and execute one module through the native backend.
    NativeRun {
        path: String,
        project_root: String,
        profile: NativeProfile,
    },
    /// Discover, select, and execute a conformance corpus in this resident process.
    Test {
        root: String,
        mode: TestMode,
        /// `None` requests the complete corpus; `Some` requests failures, smoke, and tests
        /// affected by these repository-relative paths.
        changed_paths: Option<Vec<String>>,
        previously_failed: Vec<String>,
    },
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
    Pong {
        protocol: u32,
        pid: u32,
    },
    Checked(ModuleSummary),
    Ran(RunSummary),
    NativeBuilt(NativeBuildSummary),
    NativeRan(NativeRunSummary),
    Tested(TestSummary),
    Project(ProjectSummary),
    Stats(Stats),
    /// The paths from a [`Request::Sync`] whose contents the daemon still needs.
    Needed {
        paths: Vec<String>,
    },
    Ok,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestMode {
    Parse,
    Interpreter,
}

/// Optimization and diagnostics profile for native compilation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NativeProfile {
    #[default]
    Dev,
    Check,
    Release,
}

/// Measurements and cache observability for one native build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeBuildSummary {
    pub module_symbol: String,
    pub output_path: String,
    pub compiler_identity: String,
    pub generated_c_cache_hit: bool,
    pub app_object_cache_hit: bool,
    pub runtime_object_cache_hit: bool,
    pub launcher_object_cache_hit: bool,
    pub executable_cache_hit: bool,
    pub generated_c_bytes: u64,
    pub app_object_bytes: u64,
    pub runtime_object_bytes: u64,
    pub launcher_object_bytes: u64,
    pub executable_bytes: u64,
    pub emit_millis: u64,
    pub compile_millis: u64,
    pub link_millis: u64,
    pub total_millis: u64,
}

/// Result of a native process launch, including the build that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRunSummary {
    /// Conventional process code; `None` means the process was terminated by a signal.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub build: NativeBuildSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestOutcome {
    Pass,
    Fail,
    Unsupported,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestCaseSummary {
    /// Stable corpus-relative identity, including the strictness variant when applicable.
    pub id: String,
    pub path: String,
    pub variant: String,
    pub outcome: TestOutcome,
    pub elapsed_nanos: u64,
    pub diagnostics: Vec<Diagnostic>,
}

/// How many cases shared one reason for not passing.
///
/// A conformance run over a corpus the compiler cannot execute yet has a pass count worth little
/// and a reason distribution worth a lot: it is the implementation backlog, ordered by how many
/// tests each entry unblocks. Carried on the wire so the aggregation happens once, next to the
/// runner that produced the reasons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestReasonSummary {
    pub count: usize,
    pub outcome: TestOutcome,
    /// `phase: message`, using the compiler's own words.
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSummary {
    pub discovered: usize,
    pub selected: usize,
    pub passed: usize,
    pub failed: usize,
    pub unsupported: usize,
    pub skipped: usize,
    pub elapsed_millis: u64,
    pub cases: Vec<TestCaseSummary>,
    /// Non-passing cases grouped by reason, largest group first.
    pub reasons: Vec<TestReasonSummary>,
}

/// Result of one interpreter launch. Compile/runtime failures are structured diagnostics rather
/// than transport errors so callers can retain source ranges and any stdout produced first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub path: String,
    pub file: FileId,
    pub succeeded: bool,
    pub stdout: String,
    pub value: Option<String>,
    pub instructions: u64,
    pub diagnostics: Vec<Diagnostic>,
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
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub message: String,
    pub start: Option<u32>,
    pub end: Option<u32>,
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
    #[serde(default)]
    pub ir_computed: u64,
    pub interface_computed: u64,
    pub project_computed: u64,
    /// Times a query was validated without recomputing (dependencies unchanged).
    pub validated: u64,
    /// Times a recompute produced an identical value, cutting off its dependents.
    pub early_cutoff: u64,
}
