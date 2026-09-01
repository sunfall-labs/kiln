//! Wire contract between the `kiln` CLI and the compiler daemon.
//!
//! Deliberately tiny and dependency-light: this crate is on the rebuild path of everything
//! else, and design.md §12 asks that foundational crates stay small and stable. Additive
//! changes only once the daemon is real - a client and daemon of different versions must be
//! able to detect the mismatch rather than misinterpret each other, hence `PROTOCOL_VERSION`.

use std::fmt;

use kiln_digest::Digest;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// How long one execution may run, counted in bytecode steps.
///
/// A budget is a promise to *whoever is waiting*, not a property of the language. A conformance
/// runner and a resident daemon both need a program that loops forever to fail rather than hang,
/// because something else is queued behind it. A person running their own program is not in that
/// position: node imposes no instruction budget, and the smallest schema `ajv` can compile costs
/// more than a million steps on its own -- so a budget there does not protect anybody, it just
/// answers "your program is too long" to a program that was not.
///
/// So the caller says which of the two it is, and the engine's `ExecutionLimits::default()` stays
/// the bounded one that every test already relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StepBudget {
    /// Run to completion, as node would. Nothing else is waiting on this execution.
    #[default]
    Unmetered,
    /// Refuse after this many steps, so a loop that never ends is reported rather than endured.
    Limited(u64),
}

/// Bumped whenever old and new peers could interpret the wire contract differently. The daemon
/// refuses a client that does not match.
pub const PROTOCOL_VERSION: u32 = 25;

/// SHA-256 of a source file's exact UTF-8 bytes.
///
/// This digest is the whole [`Request::Sync`] coherence decision, so unlike the readable 64-bit
/// FNV summaries elsewhere in the protocol it must be collision resistant. The wire form is
/// exactly 64 lowercase hexadecimal characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceDigest(Digest);

impl SourceDigest {
    pub fn to_hex(self) -> String {
        self.0.to_hex()
    }

    /// Rebuild a digest from its wire spelling.
    ///
    /// For readers that persist a digest and hand it back rather than recomputing it -- the CLI's
    /// stat-keyed source index is the one that exists. It is the inverse of [`Self::to_hex`] and
    /// nothing else: an unparseable spelling is an error, never a guess, so a corrupt cache
    /// degrades into re-reading the file.
    pub fn from_hex(value: &str) -> Result<Self, kiln_digest::DigestParseError> {
        Digest::from_hex(value).map(Self)
    }
}

/// Hash one source snapshot for [`Request::Sync`].
pub fn source_digest(text: &str) -> SourceDigest {
    SourceDigest(Digest::hash(text.as_bytes()))
}

impl Serialize for SourceDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_hex())
    }
}

impl<'de> Deserialize<'de> for SourceDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SourceDigestVisitor;

        impl Visitor<'_> for SourceDigestVisitor {
            type Value = SourceDigest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a 64-character lowercase hexadecimal SHA-256 digest")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
                {
                    return Err(E::custom(
                        "source digest must contain only lowercase hexadecimal characters",
                    ));
                }
                Digest::from_hex(value).map(SourceDigest).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(SourceDigestVisitor)
    }
}

/// A file as the client sees it: the path the daemon knows it by, and SHA-256 of its text.
///
/// The hash is the point of [`Request::Sync`]. A stateless client can afford to name every file
/// in the project on every command; it cannot afford to send every file's contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceHash {
    pub path: String,
    pub hash: SourceDigest,
}

/// One module request resolved to a pinned target.
///
/// All paths are canonical spellings supplied by the client-side Node resolver. Manifest
/// provenance is present when package metadata influenced the decision and absent for decisions
/// such as extension fallback that require pinning but did not consult a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageResolution {
    pub importer: String,
    pub specifier: String,
    pub target: String,
    pub condition: NodeRequestKind,
    pub package_json: Option<String>,
    pub package_json_hash: Option<SourceDigest>,
}

/// The package export condition selected by one source request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NodeRequestKind {
    Import,
    Require,
}

/// The Node grammar selected for one synchronized source path.
///
/// This is deliberately a wire-owned value rather than a dependency on the resolver crate. The
/// daemon must be able to replay the client's classification after package metadata disappears or
/// changes on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeModuleFormat {
    EsModule,
    CommonJs,
}

/// The effective Node format for one canonical synchronized source path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSourceFormat {
    pub path: String,
    pub format: NodeModuleFormat,
}

/// The package-resolution snapshot belonging to one [`Request::Sync`].
///
/// Entries are canonicalized by `(importer, specifier, condition)`. Construct maps through [`Self::new`],
/// which sorts entries and rejects duplicate import keys. Wire deserialization is stricter: it
/// rejects a map that is not already sorted and unique, so every accepted map has one stable
/// representation and identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolutionMap {
    pub project_root: String,
    pub entries: Vec<PackageResolution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_formats: Vec<NodeSourceFormat>,
}

/// A malformed package-resolution snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionMapError {
    EmptyField { field: &'static str, index: usize },
    IncompleteManifest { index: usize },
    NodeFormatsNotSorted { previous: String, current: String },
    DuplicateNodeFormat { path: String },
    EntriesNotSorted { previous: String, current: String },
    DuplicateEntry { importer: String, specifier: String },
}

impl fmt::Display for ResolutionMapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField { field, index } => {
                write!(formatter, "resolution entry {index} has an empty {field}")
            }
            Self::IncompleteManifest { index } => write!(
                formatter,
                "resolution entry {index} must provide both package_json and package_json_hash"
            ),
            Self::NodeFormatsNotSorted { previous, current } => write!(
                formatter,
                "node format entries are not sorted: {previous:?} appears before {current:?}"
            ),
            Self::DuplicateNodeFormat { path } => {
                write!(formatter, "duplicate node format for {path:?}")
            }
            Self::EntriesNotSorted { previous, current } => write!(
                formatter,
                "resolution entries are not sorted: {previous:?} appears before {current:?}"
            ),
            Self::DuplicateEntry {
                importer,
                specifier,
            } => write!(
                formatter,
                "duplicate package resolution for {importer:?} and {specifier:?}"
            ),
        }
    }
}

impl std::error::Error for ResolutionMapError {}

#[derive(Deserialize)]
struct ResolutionMapWire {
    project_root: String,
    entries: Vec<PackageResolution>,
    #[serde(default)]
    node_formats: Vec<NodeSourceFormat>,
}

impl<'de> Deserialize<'de> for ResolutionMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ResolutionMapWire::deserialize(deserializer)?;
        Self::from_sorted(wire.project_root, wire.entries, wire.node_formats)
            .map_err(de::Error::custom)
    }
}

impl ResolutionMap {
    /// Construct a map, sorting entries into canonical order and rejecting duplicate imports.
    pub fn new(
        project_root: impl Into<String>,
        mut entries: Vec<PackageResolution>,
    ) -> Result<Self, ResolutionMapError> {
        entries.sort_by(Self::compare_entries);
        let map = Self {
            project_root: project_root.into(),
            entries,
            node_formats: Vec::new(),
        };
        map.validate()?;
        Ok(map)
    }

    /// Construct a map with the effective Node format for every source the client synchronized.
    pub fn with_node_formats(
        project_root: impl Into<String>,
        mut entries: Vec<PackageResolution>,
        mut node_formats: Vec<NodeSourceFormat>,
    ) -> Result<Self, ResolutionMapError> {
        entries.sort_by(Self::compare_entries);
        node_formats.sort_by(|left, right| left.path.cmp(&right.path));
        let map = Self {
            project_root: project_root.into(),
            entries,
            node_formats,
        };
        map.validate()?;
        Ok(map)
    }

    /// Check that this map is a valid canonical snapshot.
    pub fn validate(&self) -> Result<(), ResolutionMapError> {
        if self.project_root.is_empty() {
            return Err(ResolutionMapError::EmptyField {
                field: "project_root",
                index: 0,
            });
        }
        for (index, entry) in self.entries.iter().enumerate() {
            for (field, value) in [
                ("importer", &entry.importer),
                ("specifier", &entry.specifier),
                ("target", &entry.target),
            ] {
                if value.is_empty() {
                    return Err(ResolutionMapError::EmptyField { field, index });
                }
            }
            match (&entry.package_json, &entry.package_json_hash) {
                (Some(path), Some(_)) if path.is_empty() => {
                    return Err(ResolutionMapError::EmptyField {
                        field: "package_json",
                        index,
                    });
                }
                (Some(_), Some(_)) | (None, None) => {}
                _ => return Err(ResolutionMapError::IncompleteManifest { index }),
            }
        }
        for pair in self.entries.windows(2) {
            match Self::compare_entries(&pair[0], &pair[1]) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    return Err(ResolutionMapError::DuplicateEntry {
                        importer: pair[0].importer.clone(),
                        specifier: pair[0].specifier.clone(),
                    });
                }
                std::cmp::Ordering::Greater => {
                    return Err(ResolutionMapError::EntriesNotSorted {
                        previous: Self::entry_key(&pair[0]),
                        current: Self::entry_key(&pair[1]),
                    });
                }
            }
        }
        for (index, format) in self.node_formats.iter().enumerate() {
            if format.path.is_empty() {
                return Err(ResolutionMapError::EmptyField {
                    field: "node_formats.path",
                    index,
                });
            }
            if let Some(previous) = index
                .checked_sub(1)
                .and_then(|previous| self.node_formats.get(previous))
            {
                match previous.path.cmp(&format.path) {
                    std::cmp::Ordering::Less => {}
                    std::cmp::Ordering::Equal => {
                        return Err(ResolutionMapError::DuplicateNodeFormat {
                            path: format.path.clone(),
                        });
                    }
                    std::cmp::Ordering::Greater => {
                        return Err(ResolutionMapError::NodeFormatsNotSorted {
                            previous: previous.path.clone(),
                            current: format.path.clone(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Return the full-width SHA-256 identity of this canonical resolution snapshot.
    ///
    /// The identity uses a versioned domain and length-prefixed UTF-8 fields, including the
    /// package manifest digest. This prevents concatenation and prefix ambiguities from turning
    /// distinct resolution maps into the same cache key.
    pub fn identity(&self) -> Digest {
        debug_assert!(self.validate().is_ok());
        let mut bytes = Vec::new();
        frame(&mut bytes, b"kiln-resolution-map\0v4");
        frame(&mut bytes, self.project_root.as_bytes());
        frame_u64(&mut bytes, self.entries.len() as u64);
        for entry in &self.entries {
            frame(&mut bytes, entry.importer.as_bytes());
            frame(&mut bytes, entry.specifier.as_bytes());
            frame(&mut bytes, entry.target.as_bytes());
            frame(
                &mut bytes,
                match entry.condition {
                    NodeRequestKind::Import => b"import",
                    NodeRequestKind::Require => b"require",
                },
            );
            match (&entry.package_json, &entry.package_json_hash) {
                (Some(path), Some(hash)) => {
                    frame(&mut bytes, b"manifest");
                    frame(&mut bytes, path.as_bytes());
                    frame(&mut bytes, hash.0.as_bytes());
                }
                (None, None) => frame(&mut bytes, b"no-manifest"),
                _ => unreachable!("validated resolution manifest provenance"),
            }
        }
        frame_u64(&mut bytes, self.node_formats.len() as u64);
        for format in &self.node_formats {
            frame(&mut bytes, format.path.as_bytes());
            frame(
                &mut bytes,
                match format.format {
                    NodeModuleFormat::EsModule => b"esm",
                    NodeModuleFormat::CommonJs => b"cjs",
                },
            );
        }
        Digest::hash(&bytes)
    }

    fn from_sorted(
        project_root: String,
        entries: Vec<PackageResolution>,
        node_formats: Vec<NodeSourceFormat>,
    ) -> Result<Self, ResolutionMapError> {
        let map = Self {
            project_root,
            entries,
            node_formats,
        };
        map.validate()?;
        Ok(map)
    }

    fn compare_entries(left: &PackageResolution, right: &PackageResolution) -> std::cmp::Ordering {
        left.importer
            .cmp(&right.importer)
            .then_with(|| left.specifier.cmp(&right.specifier))
            .then_with(|| left.condition.cmp(&right.condition))
    }

    fn entry_key(entry: &PackageResolution) -> String {
        format!("({}, {})", entry.importer, entry.specifier)
    }
}

fn frame(output: &mut Vec<u8>, value: &[u8]) {
    frame_u64(output, value.len() as u64);
    output.extend_from_slice(value);
}

fn frame_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

/// Connection-local identity for one staged source synchronization.
///
/// A sync is deliberately split into begin, stage, and commit messages so a stateless client can
/// upload only the files the daemon is missing without exposing a partially uploaded project to
/// concurrent queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SyncId(pub u64);

/// Monotonic identity of the daemon's authoritative source set.
///
/// Source-consuming requests carry the view they synchronized against. The daemon validates that
/// view while holding the same engine lock used to prepare the request, so a query cannot prepare
/// against one source set and execute against another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceView(pub u64);

/// A source file, interned by the daemon. Stable for the lifetime of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileId(pub u32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Liveness plus version negotiation.
    Ping {
        protocol: u32,
    },
    /// Replace a file's contents using a compare-and-swap against the authoritative source view.
    /// Returns [`Response::SourceUpdated`] with the new view.
    SetSource {
        path: String,
        text: String,
        source_view: SourceView,
    },
    /// Begin a complete synchronization of the client's source and package-resolution manifests.
    ///
    /// This request is begin-only and does not mutate the authoritative source set. It is
    /// answered with [`Response::Needed`] containing a connection-local [`SyncId`] and the subset
    /// whose contents the daemon does not already hold. The client stages those files and then
    /// commits the complete snapshot with [`Request::CommitSync`]. The resolution map is required
    /// so the daemon never has to reread mutable package metadata while linking the snapshot.
    Sync {
        files: Vec<SourceHash>,
        resolutions: ResolutionMap,
    },
    /// Stage one digest-checked file for an in-flight synchronization.
    StageSource {
        sync_id: SyncId,
        path: String,
        text: String,
    },
    /// Atomically publish a previously staged complete synchronization.
    CommitSync {
        sync_id: SyncId,
    },
    /// Run the query pipeline for a file and return its summary.
    Check {
        path: String,
        /// Project root used to reconcile relative editor-buffer spellings with this file.
        project_root: String,
        source_view: SourceView,
    },
    /// Compile the cached HIR and execute one module through the interpreter.
    Run {
        path: String,
        /// Project root used to resolve this module's explicit relative imports.
        project_root: String,
        /// Install the explicit Tokio-owned Node Adapter for this execution.
        node: bool,
        /// How long this execution may run. The client decides, because the answer depends on
        /// who is waiting: a corpus runner needs a loop to fail, a person running their own
        /// program does not.
        steps: StepBudget,
        source_view: SourceView,
    },
    /// Emit, compile, and link one module through the native backend.
    NativeBuild {
        path: String,
        project_root: String,
        /// Enable the typed Node resolver and native runtime-module materializers.
        node: bool,
        /// Requested output path. The client supplies the default so daemon and one-shot
        /// invocations produce the same artifact location.
        output: Option<String>,
        profile: NativeProfile,
        source_view: SourceView,
    },
    /// Build from an exact database snapshot, publishing only if that snapshot remains current.
    NativeBuildAtRevision {
        path: String,
        project_root: String,
        /// Enable the typed Node resolver and native runtime-module materializers.
        node: bool,
        output: Option<String>,
        profile: NativeProfile,
        source_view: SourceView,
        expected_revision: u64,
    },
    /// Emit, compile, and link one scalar module as a shared library for an embedding host.
    NativeSharedBuild {
        path: String,
        project_root: String,
        /// Requested output path. The client supplies the default so daemon and one-shot
        /// invocations produce the same artifact location.
        output: Option<String>,
        profile: NativeProfile,
        source_view: SourceView,
    },
    /// Emit, compile, link, and execute one module through the native backend.
    NativeRun {
        path: String,
        project_root: String,
        profile: NativeProfile,
        /// Enable the typed Node resolver and native runtime-module materializers.
        node: bool,
        source_view: SourceView,
    },
    /// Discover, select, and execute a conformance corpus in this resident process.
    Test {
        root: String,
        mode: TestMode,
        /// Canonical root against which `changed_paths` are relative. The daemon only reuses a
        /// resident corpus when that root contains the corpus, so an external checkout cannot be
        /// mistaken for unchanged project state.
        #[serde(default)]
        changed_root: Option<String>,
        /// `None` requests the complete corpus; `Some` requests failures, smoke, and tests
        /// affected by these repository-relative paths.
        changed_paths: Option<Vec<String>>,
        previously_failed: Vec<String>,
    },
    /// Whole-project summary, which depends on every module interface.
    Project {
        source_view: SourceView,
    },
    /// Sweep this project's artifact cache back under its byte budget.
    ///
    /// Carries no source view: the cache is addressed by content, so a sweep is independent of
    /// the authoritative source set and can never be made stale by an edit. `dry_run` asks for
    /// the report a sweep would produce without unlinking anything.
    CacheGc {
        project_root: String,
        dry_run: bool,
    },
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
    /// The requested database revision was superseded before the native artifact could be
    /// published. This is distinct from [`Response::StaleSourceView`], which rejects a request
    /// before preparation because its authoritative source view is old.
    Stale {
        current_revision: u64,
    },
    NativeSharedBuilt(NativeSharedLibrarySummary),
    NativeRan(NativeRunSummary),
    Tested(TestSummary),
    Project(ProjectSummary),
    /// What one [`Request::CacheGc`] observed and did.
    CacheSwept(CacheGcSummary),
    Stats(Stats),
    /// The paths from a [`Request::Sync`] whose contents the daemon still needs.
    Needed {
        sync_id: SyncId,
        paths: Vec<String>,
    },
    /// A complete synchronization was atomically published.
    Synced {
        source_view: SourceView,
    },
    /// A direct [`Request::SetSource`] update was atomically published.
    SourceUpdated {
        source_view: SourceView,
    },
    /// The request's source view is no longer authoritative.
    StaleSourceView {
        current: SourceView,
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
    Node,
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

/// Measurements and cache observability for one embedded shared-library build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSharedLibrarySummary {
    /// Legacy ABI-v1 entry point exported by the shared library.
    pub module_symbol: String,
    /// ABI-v2 owned-result descriptor accessor exported by the shared library.
    pub module_api_symbol: String,
    pub output_path: String,
    /// Canonical library to package; add its parent directory to the host's loader search path.
    pub loader_path: String,
    pub header_path: String,
    pub compiler_identity: String,
    pub generated_c_cache_hit: bool,
    pub object_cache_hit: bool,
    pub library_cache_hit: bool,
    pub generated_c_bytes: u64,
    pub object_bytes: u64,
    pub library_bytes: u64,
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
    /// Exact scalar returned by the native entry point. Failed or signalled processes have none.
    pub scalar_value: Option<ScalarValueSummary>,
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
    /// Whether this request reused the resident daemon's parsed corpus inventory and harness.
    /// False for one-shot coordination and whenever corpus inputs require rediscovery.
    #[serde(default)]
    pub discovery_cache_hit: bool,
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
    /// Database revision whose source state was interpreted.
    pub revision: u64,
    pub path: String,
    pub file: FileId,
    pub succeeded: bool,
    pub stdout: String,
    pub value: Option<String>,
    /// Exact scalar identity for differential checks. `None` means execution failed or returned
    /// a value outside the scalar HostOps boundary; `value` remains the human display spelling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scalar_value: Option<ScalarValueSummary>,
    pub instructions: u64,
    pub diagnostics: Vec<Diagnostic>,
}

/// Owned, exactly comparable scalar identity shared with native-result observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScalarValueSummary {
    Undefined,
    Null,
    Boolean(bool),
    /// Raw IEEE-754 binary64 bits, preserving negative zero and NaN payloads.
    Number(u64),
    String(String),
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
    /// Runtime export names declared directly by this module.
    pub exports: Vec<String>,
    /// Entry-rooted, graph-resolved runtime export names when the request supplied enough
    /// project context to link the module. Star-forwarded names appear here; ambiguous names do
    /// not. `None` preserves the distinction between an unresolved graph and an empty surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_exports: Option<Vec<String>>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Source module that owns the diagnostic when it differs from or is more precise than the
    /// request entry. Older peers omit this and clients fall back to the entry display path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
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

/// What one `kiln cache gc` did to one project's artifact cache.
///
/// The cache is layered, and only writable layers are swept, so the per-layer reports are kept
/// separate rather than summed: a total would hide which layer is the one under pressure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheGcSummary {
    pub cache_root: String,
    /// True when nothing was unlinked and the numbers describe what a real sweep would do.
    pub dry_run: bool,
    /// Bytes one layer was allowed to keep.
    pub budget_bytes: u64,
    pub layers: Vec<CacheLayerSweep>,
}

/// One layer's sweep, mirroring the cache crate's own report.
///
/// Counts are `u64` rather than `usize` so the wire form does not depend on the pointer width of
/// whichever peer encoded it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheLayerSweep {
    pub layer_index: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub records_scanned: u64,
    pub records_evicted: u64,
    /// Of those, records evicted because no key this toolchain can build addresses them.
    pub identity_stale_evicted: u64,
    pub records_raced: u64,
    pub records_within_grace: u64,
    pub temporaries_skipped: u64,
    pub orphan_markers_removed: u64,
    pub unrecognized_entries: u64,
    /// True when the layer is still over budget because every remaining record is protected.
    pub over_budget_after_sweep: bool,
}

impl CacheLayerSweep {
    pub const fn bytes_reclaimed(&self) -> u64 {
        self.bytes_before.saturating_sub(self.bytes_after)
    }
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
    /// Daemon-owned entry-rooted graph snapshot cache hits.  Query engines that do not own a
    /// graph cache report zero for these additive fields.
    #[serde(default)]
    pub graph_cache_hits: u64,
    /// Subset of graph-cache hits produced by patching one body-only module over shared topology.
    #[serde(default)]
    pub graph_cache_partial_hits: u64,
    #[serde(default)]
    pub graph_cache_misses: u64,
    #[serde(default)]
    pub graph_cache_evictions: u64,
    #[serde(default)]
    pub graph_cache_invalidations: u64,
    #[serde(default)]
    pub graph_cache_entries: u64,
    #[serde(default)]
    pub graph_cache_modules: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_v25_run_step_budget_wire_contract_round_trips() {
        assert_eq!(PROTOCOL_VERSION, 25);

        let requests = [
            (
                Request::Sync {
                    files: vec![SourceHash {
                        path: "main.js".to_owned(),
                        hash: source_digest("hello"),
                    }],
                    resolutions: empty_resolution_map(),
                },
                serde_json::json!({
                    "Sync": {
                        "files": [{
                            "path": "main.js",
                            "hash": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                        }],
                        "resolutions": {"project_root": "/project", "entries": []}
                    }
                }),
            ),
            (
                Request::StageSource {
                    sync_id: SyncId(9),
                    path: "main.js".to_owned(),
                    text: "hello".to_owned(),
                },
                serde_json::json!({
                    "StageSource": {"sync_id": 9, "path": "main.js", "text": "hello"}
                }),
            ),
            (
                Request::CommitSync { sync_id: SyncId(9) },
                serde_json::json!({"CommitSync": {"sync_id": 9}}),
            ),
            (
                Request::SetSource {
                    path: "main.js".to_owned(),
                    text: "hello".to_owned(),
                    source_view: SourceView(12),
                },
                serde_json::json!({
                    "SetSource": {"path": "main.js", "text": "hello", "source_view": 12}
                }),
            ),
            (
                Request::Project {
                    source_view: SourceView(12),
                },
                serde_json::json!({"Project": {"source_view": 12}}),
            ),
            (
                Request::Run {
                    path: "main.mjs".to_owned(),
                    project_root: "/project".to_owned(),
                    node: true,
                    steps: StepBudget::Limited(1_000_000),
                    source_view: SourceView(12),
                },
                serde_json::json!({"Run": {
                    "path": "main.mjs",
                    "project_root": "/project",
                    "node": true,
                    "steps": {"Limited": 1_000_000},
                    "source_view": 12
                }}),
            ),
            (
                Request::NativeBuild {
                    path: "server.mjs".to_owned(),
                    project_root: "/project".to_owned(),
                    node: true,
                    output: Some("/project/server".to_owned()),
                    profile: NativeProfile::Release,
                    source_view: SourceView(12),
                },
                serde_json::json!({"NativeBuild": {
                    "path": "server.mjs",
                    "project_root": "/project",
                    "node": true,
                    "output": "/project/server",
                    "profile": "Release",
                    "source_view": 12
                }}),
            ),
            (
                Request::CacheGc {
                    project_root: "/project".to_owned(),
                    dry_run: true,
                },
                serde_json::json!({"CacheGc": {"project_root": "/project", "dry_run": true}}),
            ),
            (
                Request::Test {
                    root: "/project/conformance".to_owned(),
                    mode: TestMode::Node,
                    changed_root: Some("/project".to_owned()),
                    changed_paths: Some(vec!["crates/kiln-runtime/src/node_net.rs".to_owned()]),
                    previously_failed: Vec::new(),
                },
                serde_json::json!({"Test": {
                    "root": "/project/conformance",
                    "mode": "Node",
                    "changed_root": "/project",
                    "changed_paths": ["crates/kiln-runtime/src/node_net.rs"],
                    "previously_failed": []
                }}),
            ),
        ];

        for (request, expected) in requests {
            let encoded = serde_json::to_value(&request).expect("serialize protocol request");
            assert_eq!(encoded, expected);
            let decoded: Request =
                serde_json::from_value(encoded).expect("decode protocol request");
            assert_eq!(
                serde_json::to_value(decoded).expect("re-serialize protocol request"),
                expected
            );
        }

        let responses = [
            (
                Response::Needed {
                    sync_id: SyncId(9),
                    paths: vec!["main.js".to_owned()],
                },
                serde_json::json!({"Needed": {"sync_id": 9, "paths": ["main.js"]}}),
            ),
            (
                Response::Synced {
                    source_view: SourceView(12),
                },
                serde_json::json!({"Synced": {"source_view": 12}}),
            ),
            (
                Response::SourceUpdated {
                    source_view: SourceView(13),
                },
                serde_json::json!({"SourceUpdated": {"source_view": 13}}),
            ),
            (
                Response::StaleSourceView {
                    current: SourceView(13),
                },
                serde_json::json!({"StaleSourceView": {"current": 13}}),
            ),
            (
                Response::CacheSwept(CacheGcSummary {
                    cache_root: "/project/.kiln/cache".to_owned(),
                    dry_run: true,
                    budget_bytes: 2_147_483_648,
                    layers: vec![CacheLayerSweep {
                        layer_index: 0,
                        bytes_before: 900,
                        bytes_after: 400,
                        records_scanned: 3,
                        records_evicted: 1,
                        identity_stale_evicted: 0,
                        records_raced: 0,
                        records_within_grace: 1,
                        temporaries_skipped: 0,
                        orphan_markers_removed: 0,
                        unrecognized_entries: 0,
                        over_budget_after_sweep: false,
                    }],
                }),
                serde_json::json!({"CacheSwept": {
                    "cache_root": "/project/.kiln/cache",
                    "dry_run": true,
                    "budget_bytes": 2_147_483_648u64,
                    "layers": [{
                        "layer_index": 0,
                        "bytes_before": 900,
                        "bytes_after": 400,
                        "records_scanned": 3,
                        "records_evicted": 1,
                        "identity_stale_evicted": 0,
                        "records_raced": 0,
                        "records_within_grace": 1,
                        "temporaries_skipped": 0,
                        "orphan_markers_removed": 0,
                        "unrecognized_entries": 0,
                        "over_budget_after_sweep": false
                    }]
                }}),
            ),
        ];

        for (response, expected) in responses {
            let encoded = serde_json::to_value(&response).expect("serialize protocol response");
            assert_eq!(encoded, expected);
            let decoded: Response =
                serde_json::from_value(encoded).expect("decode protocol response");
            assert_eq!(
                serde_json::to_value(decoded).expect("re-serialize protocol response"),
                expected
            );
        }
    }

    #[test]
    fn test_summary_reports_discovery_reuse_and_defaults_old_payloads_to_cold() {
        let summary = TestSummary {
            discovered: 35,
            discovery_cache_hit: true,
            selected: 5,
            passed: 5,
            ..TestSummary::default()
        };
        let encoded = serde_json::to_value(&summary).expect("serialize test summary");
        assert_eq!(encoded["discovery_cache_hit"], true);

        let legacy = serde_json::json!({
            "discovered": 35,
            "selected": 5,
            "passed": 5,
            "failed": 0,
            "unsupported": 0,
            "skipped": 0,
            "elapsed_millis": 1,
            "cases": [],
            "reasons": []
        });
        let decoded: TestSummary =
            serde_json::from_value(legacy).expect("decode pre-v25 test summary");
        assert!(!decoded.discovery_cache_hit);
    }

    #[test]
    fn source_digest_is_sha256_and_sync_serializes_one_canonical_hex_value() {
        assert_eq!(
            source_digest("").to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            source_digest("hello").to_hex(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );

        let request = Request::Sync {
            files: vec![SourceHash {
                path: "main.js".to_owned(),
                hash: source_digest("hello"),
            }],
            resolutions: empty_resolution_map(),
        };
        let encoded = serde_json::to_value(&request).expect("serialize strong source digest");
        assert_eq!(
            encoded,
            serde_json::json!({
                "Sync": {
                    "files": [{
                        "path": "main.js",
                        "hash": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                    }],
                    "resolutions": {"project_root": "/project", "entries": []}
                }
            })
        );
        let decoded: Request =
            serde_json::from_value(encoded).expect("round trip strong source digest");
        let Request::Sync { files, resolutions } = decoded else {
            panic!("decoded request must remain Sync");
        };
        assert_eq!(files[0].hash, source_digest("hello"));
        assert_eq!(resolutions, empty_resolution_map());
    }

    fn empty_resolution_map() -> ResolutionMap {
        ResolutionMap::new("/project", Vec::new()).expect("empty resolution map is valid")
    }

    fn package_resolution(importer: &str, specifier: &str, target: &str) -> PackageResolution {
        PackageResolution {
            importer: importer.to_owned(),
            specifier: specifier.to_owned(),
            target: target.to_owned(),
            condition: NodeRequestKind::Import,
            package_json: Some("/project/node_modules/pkg/package.json".to_owned()),
            package_json_hash: Some(source_digest("package metadata")),
        }
    }

    #[test]
    fn resolution_map_sorts_entries_and_identity_is_order_independent() {
        let first = package_resolution(
            "/project/src/main.ts",
            "z",
            "/project/node_modules/z/index.js",
        );
        let second = package_resolution(
            "/project/src/main.ts",
            "a",
            "/project/node_modules/a/index.js",
        );
        let reversed = ResolutionMap::new("/project", vec![first.clone(), second.clone()])
            .expect("entries with distinct keys are valid");
        let sorted = ResolutionMap::new("/project", vec![second, first])
            .expect("entries with distinct keys are valid");

        assert_eq!(reversed.entries[0].specifier, "a");
        assert_eq!(reversed.entries, sorted.entries);
        assert_eq!(reversed.identity(), sorted.identity());
    }

    #[test]
    fn resolution_map_rejects_duplicate_import_keys() {
        let entry = package_resolution(
            "/project/src/main.ts",
            "pkg",
            "/project/node_modules/pkg/a.js",
        );
        let error = ResolutionMap::new("/project", vec![entry.clone(), entry])
            .expect_err("one importer/specifier key must have one target");
        assert!(matches!(error, ResolutionMapError::DuplicateEntry { .. }));
    }

    #[test]
    fn resolution_map_wire_requires_canonical_sorted_unique_entries() {
        let first = package_resolution(
            "/project/src/main.ts",
            "a",
            "/project/node_modules/a/index.js",
        );
        let second = package_resolution(
            "/project/src/main.ts",
            "z",
            "/project/node_modules/z/index.js",
        );
        let unsorted = serde_json::json!({
            "project_root": "/project",
            "entries": [second, first]
        });
        assert!(serde_json::from_value::<ResolutionMap>(unsorted).is_err());

        let duplicate = package_resolution(
            "/project/src/main.ts",
            "pkg",
            "/project/node_modules/pkg/index.js",
        );
        let duplicate_wire = serde_json::json!({
            "project_root": "/project",
            "entries": [duplicate.clone(), duplicate]
        });
        assert!(serde_json::from_value::<ResolutionMap>(duplicate_wire).is_err());
    }

    #[test]
    fn resolution_identity_is_length_framed_and_includes_manifest_digest() {
        let left = package_resolution("a", "bc", "/project/node_modules/pkg/index.js");
        let right = package_resolution("ab", "c", "/project/node_modules/pkg/index.js");
        let left_map = ResolutionMap::new("/project", vec![left]).expect("valid map");
        let right_map = ResolutionMap::new("/project", vec![right]).expect("valid map");
        assert_ne!(left_map.identity(), right_map.identity());

        let mut changed_hash = package_resolution(
            "/project/src/main.ts",
            "ab",
            "/project/node_modules/pkg/index.js",
        );
        changed_hash.package_json_hash = Some(source_digest("different package metadata"));
        let changed_map = ResolutionMap::new("/project", vec![changed_hash]).expect("valid map");
        assert_ne!(left_map.identity(), changed_map.identity());

        let formatted_map = ResolutionMap::with_node_formats(
            "/project",
            vec![package_resolution(
                "a",
                "bc",
                "/project/node_modules/pkg/index.js",
            )],
            vec![NodeSourceFormat {
                path: "/project/node_modules/pkg/index.js".to_owned(),
                format: NodeModuleFormat::EsModule,
            }],
        )
        .expect("format is a valid pinned decision");
        assert_ne!(left_map.identity(), formatted_map.identity());

        let encoded = serde_json::to_value(&formatted_map).expect("encode format snapshot");
        assert_eq!(
            encoded["node_formats"],
            serde_json::json!([{
                "path": "/project/node_modules/pkg/index.js",
                "format": "EsModule"
            }])
        );
        assert_eq!(
            serde_json::from_value::<ResolutionMap>(encoded).expect("decode format snapshot"),
            formatted_map
        );
    }

    #[test]
    fn manifest_free_resolution_is_canonical_and_has_a_distinct_identity() {
        let mut without_manifest = package_resolution("a", "./lib", "/project/lib.js");
        without_manifest.package_json = None;
        without_manifest.package_json_hash = None;
        let without = ResolutionMap::new("/project", vec![without_manifest.clone()])
            .expect("extension fallback is a valid pinned decision");

        let with = ResolutionMap::new(
            "/project",
            vec![package_resolution("a", "./lib", "/project/lib.js")],
        )
        .expect("manifest-backed decision is valid");
        assert_ne!(without.identity(), with.identity());

        without_manifest.package_json = Some("/project/package.json".to_owned());
        assert!(matches!(
            ResolutionMap::new("/project", vec![without_manifest]),
            Err(ResolutionMapError::IncompleteManifest { .. })
        ));
    }

    #[test]
    fn source_digest_rejects_the_old_number_and_noncanonical_text() {
        for hash in [
            serde_json::json!(7),
            serde_json::json!("abc"),
            serde_json::json!("2CF24DBA5FB0A30E26E83B2AC5B9E29E1B161E5C1FA7425E73043362938B9824"),
            serde_json::json!("gcf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"),
        ] {
            let encoded = serde_json::json!({"path": "main.js", "hash": hash});
            assert!(
                serde_json::from_value::<SourceHash>(encoded).is_err(),
                "invalid source digest must not cross the wire"
            );
        }
    }

    #[test]
    fn scalar_value_identity_round_trips_without_losing_number_bits_or_string_bytes() {
        let values = [
            ScalarValueSummary::Undefined,
            ScalarValueSummary::Null,
            ScalarValueSummary::Boolean(false),
            ScalarValueSummary::Boolean(true),
            ScalarValueSummary::Number((-0.0_f64).to_bits()),
            ScalarValueSummary::Number(f64::NAN.to_bits()),
            ScalarValueSummary::Number(u64::MAX),
            ScalarValueSummary::String("line one\nline two\0".to_owned()),
        ];
        for value in values {
            let encoded = serde_json::to_vec(&value).expect("serialize scalar identity");
            let decoded = serde_json::from_slice(&encoded).expect("deserialize scalar identity");
            assert_eq!(value, decoded);
        }
    }

    #[test]
    fn older_run_summaries_without_scalar_identity_decode_as_none() {
        let encoded = serde_json::json!({
            "revision": 1,
            "path": "main.js",
            "file": 1,
            "succeeded": true,
            "stdout": "",
            "value": "undefined",
            "instructions": 1,
            "diagnostics": []
        });
        let summary: RunSummary = serde_json::from_value(encoded).expect("decode old summary");
        assert_eq!(summary.scalar_value, None);
    }

    #[test]
    fn older_stats_without_graph_cache_fields_decode_with_zeroes() {
        let stats: Stats = serde_json::from_value(serde_json::json!({
            "revision": 7,
            "parse_computed": 2,
            "ir_computed": 1,
            "interface_computed": 1,
            "project_computed": 0,
            "validated": 3,
            "early_cutoff": 1
        }))
        .expect("older stats remain decodable");
        assert_eq!(stats.revision, 7);
        assert_eq!(stats.graph_cache_hits, 0);
        assert_eq!(stats.graph_cache_partial_hits, 0);
        assert_eq!(stats.graph_cache_modules, 0);
    }

    #[test]
    fn graph_cache_partial_hits_round_trip_as_an_additive_stats_field() {
        let stats = Stats {
            graph_cache_hits: 4,
            graph_cache_partial_hits: 3,
            graph_cache_misses: 1,
            ..Stats::default()
        };
        let encoded = serde_json::to_vec(&stats).expect("serialize stats");
        let decoded: Stats = serde_json::from_slice(&encoded).expect("deserialize stats");
        assert_eq!(decoded, stats);
    }

    #[test]
    fn module_summary_distinguishes_declared_exports_from_an_optional_resolved_surface() {
        let old = serde_json::json!({
            "path": "barrel.js",
            "file": 1,
            "implementation_hash": 2,
            "interface_hash": 3,
            "exports": [],
            "diagnostics": []
        });
        let decoded: ModuleSummary =
            serde_json::from_value(old).expect("older additive summary still decodes");
        assert_eq!(decoded.resolved_exports, None);

        let summary = ModuleSummary {
            path: "barrel.js".to_owned(),
            file: FileId(1),
            implementation_hash: 2,
            interface_hash: 3,
            exports: Vec::new(),
            resolved_exports: Some(vec!["answer".to_owned()]),
            diagnostics: Vec::new(),
        };
        let encoded = serde_json::to_value(&summary).expect("resolved summary serializes");
        assert_eq!(encoded["resolved_exports"], serde_json::json!(["answer"]));
    }

    #[test]
    fn diagnostic_provenance_is_additive() {
        let old = serde_json::json!({
            "message": "expected an identifier",
            "start": 7,
            "end": 8
        });
        let decoded: Diagnostic =
            serde_json::from_value(old).expect("older diagnostic still decodes");
        assert_eq!(decoded.path, None);

        let diagnostic = Diagnostic {
            path: Some("/src/dep.js".to_owned()),
            message: "unresolved binding".to_owned(),
            start: Some(1),
            end: Some(4),
        };
        let encoded = serde_json::to_value(diagnostic).expect("diagnostic serializes");
        assert_eq!(encoded["path"], "/src/dep.js");
    }
}
