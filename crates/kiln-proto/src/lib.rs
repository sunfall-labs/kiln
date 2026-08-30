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

/// Bumped whenever old and new peers could interpret the wire contract differently. The daemon
/// refuses a client that does not match.
pub const PROTOCOL_VERSION: u32 = 15;

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
    /// to answer a question whose response was "nothing changed". The list is authoritative:
    /// active daemon sources omitted from it are removed before the response is sent.
    Sync {
        files: Vec<SourceHash>,
    },
    /// Run the query pipeline for a file and return its summary.
    Check {
        path: String,
        /// Project root used to reconcile relative editor-buffer spellings with this file.
        project_root: String,
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
    /// Build from an exact database snapshot, publishing only if that snapshot remains current.
    NativeBuildAtRevision {
        path: String,
        project_root: String,
        output: Option<String>,
        profile: NativeProfile,
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
    /// The requested snapshot was superseded before the native artifact could be published.
    Stale {
        current_revision: u64,
    },
    NativeSharedBuilt(NativeSharedLibrarySummary),
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

/// Measurements and cache observability for one embedded shared-library build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSharedLibrarySummary {
    pub module_symbol: String,
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

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let encoded = serde_json::to_value(&request).expect("serialize strong source digest");
        assert_eq!(
            encoded,
            serde_json::json!({
                "Sync": {
                    "files": [{
                        "path": "main.js",
                        "hash": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                    }]
                }
            })
        );
        let decoded: Request =
            serde_json::from_value(encoded).expect("round trip strong source digest");
        let Request::Sync { files } = decoded else {
            panic!("decoded request must remain Sync");
        };
        assert_eq!(files[0].hash, source_digest("hello"));
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
