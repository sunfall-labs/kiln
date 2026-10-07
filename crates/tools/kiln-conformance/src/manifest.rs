//! The manifest metadata source: a corpus whose files may not be edited.
//!
//! `conformance/manifest.toml` exists because Kiln's own JavaScript corpus was copied byte for
//! byte from the project it was proven in, and one of its probes is a single line whose entire
//! point is that it contains nothing else.  Adding Test262 frontmatter to those files would
//! change the fixtures, so their metadata sits beside them instead.  This module reads that file;
//! it does not run anything, and it deliberately sits next to the frontmatter reader rather than
//! replacing it — frontmatter remains the convention for tests Kiln writes itself.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::toml_lite::{self, Entry, Table};

/// The file name a corpus directory (or an ancestor of one) declares its tests in.
pub const MANIFEST_FILE_NAME: &str = "manifest.toml";

/// Versioned framing for the exact inputs that authorized a generated bundle.
const BUNDLE_PROVENANCE_HEADER: &[u8] = b"kiln-bundle-provenance-v2\n";

/// One `[[test]]` entry.
///
/// Fields the runner does not read — `catches`, `notes`, `effect_api` — are prose or
/// provenance for a human and are intentionally not modelled: parsing them would freeze an
/// editorial convention into code that has no use for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestTest {
    /// Corpus-wide identity, stable no matter which subdirectory a run was pointed at.
    pub id: String,
    /// Program source, relative to the manifest's own directory.
    pub source: PathBuf,
    /// A single-file build of `source`, relative to the manifest's own directory.
    ///
    /// Read, unlike `status`, because it names a *file* rather than an expectation: whether the
    /// bundle is there is a fact about the working tree, and the runner checks it rather than
    /// believing it. The bundles are not committed, so a checkout without them skips these tests
    /// exactly as before and one that has run `conformance/effect/build.sh` runs them.
    pub bundle: Option<PathBuf>,
    /// Additional files whose exact bytes participate in the generated bundle.
    ///
    /// The bundle bytes and `source` precede these frames and must not be repeated here. A bundle
    /// is runnable only when its adjacent provenance record matches every frame in manifest order.
    pub bundle_inputs: Vec<PathBuf>,
    /// JavaScript semantics the test would stop passing without; the changed-test selector's key.
    pub features: Vec<String>,
    /// What the corpus records about the test's runnability.  The runner does not read this: an
    /// observed result must never be steerable from the file that records the expectation.
    pub status: Option<String>,
    /// A host adapter the program cannot run without, currently only `"node"`.
    ///
    /// This is read, and `status` is not, because the two say different kinds of thing. `status`
    /// records an *expectation* — whether the test is thought to pass — and letting the file that
    /// holds the expectation decide whether it is checked would make the corpus unfalsifiable. A
    /// required host is a *capability*: the program imports `node:http`, so without the adapter
    /// there is no run to judge at all. Reading it converts 37 identical, mysterious assertion
    /// failures into 37 statements of which flag is missing.
    pub requires_host: Option<String>,
    /// Whether this case observes elapsed host time. Omitted or `"virtual"` keeps deterministic
    /// timer advancement; `"wall"` opts a host-backed Node case into real elapsed time.
    pub wall_clock: bool,
    /// Exactly what the program must write, including its trailing newline.
    pub expected_stdout: String,
    /// Text the report of the uncaught exception the program must end with contains -- node's
    /// stderr summary line, for a program node ends with exit status 1 (an unhandled rejection,
    /// say). Absent, the program must run to completion.
    pub expected_uncaught: Option<String>,
}

/// Every test one manifest declares, together with the directory its paths resolve against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    directory: PathBuf,
    tests: Vec<ManifestTest>,
}

impl Manifest {
    /// Read the manifest at `path`.
    pub fn read(path: &Path) -> Result<Self, ManifestError> {
        let text = fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_owned(),
            source,
        })?;
        let directory = path.parent().unwrap_or(Path::new(".")).to_owned();
        Self::from_text(&text, directory).map_err(|error| error.at(path))
    }

    /// Parse manifest text whose relative paths resolve against `directory`.
    fn from_text(text: &str, directory: PathBuf) -> Result<Self, UnlocatedError> {
        let tables = toml_lite::parse(text).map_err(UnlocatedError::Syntax)?;
        let mut tests = Vec::new();
        for table in tables.iter().filter(|table| table.is_array_element) {
            // `[[fixture]]` entries are committed inputs, not tests, and the manifest says so.
            // Counting one as a test would invent a failure out of a benchmark input.
            if table.name == "test" {
                tests.push(read_test(table)?);
            }
        }
        reject_duplicate_ids(&tests)?;
        tests.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(Self { directory, tests })
    }

    /// The directory relative `source` paths resolve against.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Every declared test, ordered by id.
    pub fn tests(&self) -> &[ManifestTest] {
        &self.tests
    }

    /// The absolute (or manifest-relative) path of a test's program.
    pub fn source_path(&self, test: &ManifestTest) -> PathBuf {
        self.directory.join(&test.source)
    }

    /// Every additional file whose change invalidates and selects this generated test.
    pub(crate) fn bundle_input_paths(&self, test: &ManifestTest) -> Vec<PathBuf> {
        test.bundle_inputs
            .iter()
            .map(|path| self.directory.join(path))
            .collect()
    }

    /// Resolve the executable and fail closed when a generated bundle cannot prove its inputs.
    pub(crate) fn resolve_program(&self, test: &ManifestTest) -> ProgramResolution {
        let source = self.source_path(test);
        let Some(bundle) = &test.bundle else {
            return ProgramResolution::ready(source);
        };
        let source_bytes = match fs::read(&source) {
            Ok(bytes) => bytes,
            // The authored program is a required corpus file. Let normal execution report its
            // read failure rather than disguising a broken manifest entry as a missing build.
            Err(_) => return ProgramResolution::ready(source),
        };
        let bundle = self.directory.join(bundle);
        let provenance = bundle_provenance_path(&bundle);
        let bundle_bytes = fs::read(&bundle);
        let recorded = fs::read(&provenance);
        if let (Ok(bundle_bytes), Ok(recorded)) = (bundle_bytes, recorded)
            && self
                .expected_bundle_provenance(test, &bundle_bytes, &source_bytes)
                .is_ok_and(|expected| expected == recorded)
        {
            return ProgramResolution::ready(bundle);
        }
        ProgramResolution {
            path: source,
            unavailable_reason: Some(
                "generated bundle is missing, stale, or unverified; rebuild it with \
                 conformance/effect/build.sh"
                    .to_owned(),
            ),
        }
    }

    fn expected_bundle_provenance(
        &self,
        test: &ManifestTest,
        bundle: &[u8],
        source: &[u8],
    ) -> io::Result<Vec<u8>> {
        let mut framed = BUNDLE_PROVENANCE_HEADER.to_vec();
        push_frame(&mut framed, bundle);
        push_frame(&mut framed, source);
        for path in &test.bundle_inputs {
            let bytes = fs::read(self.directory.join(path))?;
            push_frame(&mut framed, &bytes);
        }
        Ok(framed)
    }

    /// The tests whose program lives at or below `root`.
    ///
    /// This is what lets one manifest at the top of a corpus serve `kiln test conformance/guard`
    /// and `kiln test conformance/effect` without either invocation picking up the other's tests.
    pub fn tests_under<'a>(&'a self, root: &'a Path) -> impl Iterator<Item = &'a ManifestTest> {
        let root = normalized_path(root);
        self.tests.iter().filter(move |test| {
            // The manifest is allowed to describe a whole corpus while a caller asks for a
            // subtree.  Check every path that execution may read, not only the source: a
            // bundle or one of its provenance inputs outside the requested root would let a
            // narrow run consume files the caller did not select.  `starts_with` on raw paths
            // is not enough here because `guard/../outside.js` lexically starts with
            // `guard`, and a symlink can make an apparently-contained path escape as well.
            let mut paths = std::iter::once(self.source_path(test))
                .chain(test.bundle.as_ref().map(|path| self.directory.join(path)))
                .chain(
                    test.bundle_inputs
                        .iter()
                        .map(|path| self.directory.join(path)),
                );
            paths.all(|path| path_is_under(&path, &root))
        })
    }
}

/// Normalize a path without touching the filesystem.  Discovery must still report a missing
/// source, so containment cannot depend solely on `canonicalize`; lexical normalization handles
/// `.` and `..` first and the canonical check below covers existing symlink components.
fn normalized_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // For a relative path whose leading components are `..`, retain the parent
                // markers.  Absolute roots cannot pop past their root, so `pop` is sufficient
                // there.
                if !normalized.pop() && !path.is_absolute() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn path_is_under(path: &Path, root: &Path) -> bool {
    let path = normalized_path(path);
    let root = normalized_path(root);
    if !path.starts_with(&root) {
        return false;
    }

    // A missing source is still a useful manifest result (the runner can report it), but its
    // nearest existing ancestor can still expose a symlink escape.  Resolve that ancestor and
    // append the missing tail so both existing and missing paths receive the same containment
    // check.
    match (
        canonical_with_missing_tail(&root),
        canonical_with_missing_tail(&path),
    ) {
        (Some(root), Some(path)) => path.starts_with(root),
        // A containment check that cannot be proven is not safe to accept.  The only missing
        // path case that reaches here is handled by canonical_with_missing_tail itself, which
        // resolves its nearest real ancestor before appending the missing tail.
        _ => false,
    }
}

fn canonical_with_missing_tail(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        // `exists` follows links and turns a dangling symlink into an indistinguishable missing
        // component.  symlink_metadata lets us stop at the link itself; canonicalize then either
        // proves where it points or returns None, rejecting the path.
        match fs::symlink_metadata(&current) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(current.file_name()?.to_owned());
                current = current.parent()?.to_path_buf();
            }
            // Permission and other filesystem errors must not become a containment bypass.
            Err(_) => return None,
        }
    }
    let mut canonical = fs::canonicalize(current).ok()?;
    for component in missing.iter().rev() {
        canonical.push(component);
    }
    Some(canonical)
}

fn read_test(table: &Table) -> Result<ManifestTest, UnlocatedError> {
    let requires_host = optional_string(table, "requires_host")?.map(str::to_owned);
    let wall_clock = match optional_string(table, "clock")? {
        None | Some("virtual") => false,
        Some("wall") => true,
        Some(_) => {
            return Err(UnlocatedError::Schema {
                line: table.get("clock").map_or(table.line, |entry| entry.line),
                message: "`clock` must be `virtual` or `wall`".to_owned(),
            });
        }
    };
    if wall_clock && requires_host.as_deref() != Some("node") {
        return Err(UnlocatedError::Schema {
            line: table.get("clock").map_or(table.line, |entry| entry.line),
            message: "`clock = \"wall\"` requires `requires_host = \"node\"`".to_owned(),
        });
    }
    Ok(ManifestTest {
        id: required_string(table, "id")?.to_owned(),
        source: PathBuf::from(required_string(table, "source")?),
        bundle: optional_string(table, "bundle")?.map(PathBuf::from),
        bundle_inputs: optional_strings(table, "bundle_inputs")?
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        features: optional_strings(table, "features")?,
        status: optional_string(table, "status")?.map(str::to_owned),
        requires_host,
        wall_clock,
        expected_stdout: required_string(table, "expected_stdout")?.to_owned(),
        expected_uncaught: optional_string(table, "expected_uncaught")?.map(str::to_owned),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgramResolution {
    pub(crate) path: PathBuf,
    pub(crate) unavailable_reason: Option<String>,
}

impl ProgramResolution {
    fn ready(path: PathBuf) -> Self {
        Self {
            path,
            unavailable_reason: None,
        }
    }
}

fn bundle_provenance_path(bundle: &Path) -> PathBuf {
    let mut path = bundle.as_os_str().to_os_string();
    path.push(".kiln-inputs");
    PathBuf::from(path)
}

fn push_frame(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(bytes.len().to_string().as_bytes());
    output.push(b'\n');
    output.extend_from_slice(bytes);
}

fn reject_duplicate_ids(tests: &[ManifestTest]) -> Result<(), UnlocatedError> {
    for (index, test) in tests.iter().enumerate() {
        if let Some(earlier) = tests[..index].iter().find(|other| other.id == test.id) {
            return Err(UnlocatedError::Schema {
                line: 0,
                message: format!("duplicate test id `{}`", earlier.id),
            });
        }
    }
    Ok(())
}

fn required_string<'a>(table: &'a Table, key: &str) -> Result<&'a str, UnlocatedError> {
    let entry = table.get(key).ok_or_else(|| UnlocatedError::Schema {
        line: table.line,
        message: format!("[[{}]] is missing required key `{key}`", table.name),
    })?;
    typed_string(entry)
}

fn optional_string<'a>(table: &'a Table, key: &str) -> Result<Option<&'a str>, UnlocatedError> {
    table.get(key).map(typed_string).transpose()
}

fn typed_string(entry: &Entry) -> Result<&str, UnlocatedError> {
    entry.value.as_str().ok_or_else(|| UnlocatedError::Schema {
        line: entry.line,
        message: format!(
            "`{}` must be a string, got {}",
            entry.key,
            entry.value.type_name()
        ),
    })
}

fn optional_strings(table: &Table, key: &str) -> Result<Vec<String>, UnlocatedError> {
    match table.get(key) {
        None => Ok(Vec::new()),
        Some(entry) => entry
            .value
            .as_array()
            .map(<[String]>::to_vec)
            .ok_or_else(|| UnlocatedError::Schema {
                line: entry.line,
                message: format!(
                    "`{}` must be an array of strings, got {}",
                    entry.key,
                    entry.value.type_name()
                ),
            }),
    }
}

/// A manifest failure before the file it came from is known.
enum UnlocatedError {
    Syntax(toml_lite::TomlError),
    Schema { line: usize, message: String },
}

impl UnlocatedError {
    fn at(self, path: &Path) -> ManifestError {
        let path = path.to_owned();
        match self {
            Self::Syntax(error) => ManifestError::Syntax {
                path,
                line: error.line,
                message: error.message,
            },
            Self::Schema { line, message } => ManifestError::Schema {
                path,
                line,
                message,
            },
        }
    }
}

/// Why a manifest could not be turned into tests.
///
/// Every variant names the file, because the first question about a broken manifest is always
/// which one — a corpus can be pointed at from any directory above it.
#[derive(Debug)]
pub enum ManifestError {
    /// The manifest could not be read.
    Io { path: PathBuf, source: io::Error },
    /// The manifest is not valid TOML in the subset this crate reads.
    Syntax {
        path: PathBuf,
        line: usize,
        message: String,
    },
    /// The manifest parsed, but an entry does not describe a test.
    Schema {
        path: PathBuf,
        /// One-based line of the offending entry's header; `0` when it applies to the whole file.
        line: usize,
        message: String,
    },
}

impl Display for ManifestError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Syntax {
                path,
                line,
                message,
            }
            | Self::Schema {
                path,
                line,
                message,
            } => {
                write!(f, "{}", path.display())?;
                if *line > 0 {
                    write!(f, ":{line}")?;
                }
                write!(f, ": {message}")
            }
        }
    }
}

impl Error for ManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Syntax { .. } | Self::Schema { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_MANIFEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TemporaryManifest {
        directory: PathBuf,
    }

    impl TemporaryManifest {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "kiln-bundle-provenance-test-{}-{}",
                std::process::id(),
                TEMP_MANIFEST_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&directory).expect("create temporary manifest directory");
            Self { directory }
        }
    }

    impl Drop for TemporaryManifest {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn manifest(text: &str) -> Result<Manifest, ManifestError> {
        Manifest::from_text(text, PathBuf::from("/corpus"))
            .map_err(|error| error.at(Path::new("/corpus/manifest.toml")))
    }

    const ONE_TEST: &str = "\
[[test]]
id = \"guard/probe\"
source = \"guard/probe.js\"
status = \"runnable\"
features = [\"iteration\"]
catches = \"\"\"
Prose the runner has no use for.\\
\"\"\"
expected_stdout = \"\"\"
ok
\"\"\"
";

    #[test]
    fn reads_a_test_and_ignores_prose_fields() {
        let manifest = manifest(ONE_TEST).expect("manifest is well formed");
        let test = &manifest.tests()[0];
        assert_eq!(test.id, "guard/probe");
        assert_eq!(test.expected_stdout, "ok\n");
        assert_eq!(test.features, ["iteration"]);
        assert_eq!(test.status.as_deref(), Some("runnable"));
        assert!(test.bundle_inputs.is_empty());
        assert!(!test.wall_clock);
        assert_eq!(
            manifest.source_path(test),
            Path::new("/corpus/guard/probe.js")
        );
    }

    #[test]
    fn host_clock_is_an_explicit_node_manifest_policy() {
        let parsed = manifest(
            "[[test]]\nid = \"clock\"\nsource = \"clock.cjs\"\nrequires_host = \"node\"\nclock = \"wall\"\nexpected_stdout = \"pass\\n\"\n",
        )
        .expect("valid Node wall-clock policy");
        assert!(parsed.tests()[0].wall_clock);

        let invalid = manifest(
            "[[test]]\nid = \"clock\"\nsource = \"clock.js\"\nclock = \"wall\"\nexpected_stdout = \"pass\\n\"\n",
        )
        .expect_err("wall time requires the Node host capability");
        assert!(
            invalid
                .to_string()
                .contains("requires `requires_host = \"node\"`")
        );
    }

    #[test]
    fn a_bundle_runs_only_with_exact_provenance_for_every_declared_input() {
        let root = TemporaryManifest::new();
        let source = b"console.log('source');\n";
        let bundle = b"console.log('bundle');\n";
        let lock = b"pinned build input\n";
        fs::write(root.directory.join("source.js"), source).expect("write source");
        fs::write(root.directory.join("build.lock"), lock).expect("write build input");
        fs::write(root.directory.join("bundle.js"), bundle).expect("write bundle");
        let manifest = Manifest::from_text(
            r#"[[test]]
id = "bundle"
source = "source.js"
bundle = "bundle.js"
bundle_inputs = ["build.lock"]
expected_stdout = "bundle\n"
"#,
            root.directory.clone(),
        )
        .unwrap_or_else(|_| panic!("manifest parses"));
        let test = &manifest.tests()[0];

        let missing = manifest.resolve_program(test);
        assert!(missing.unavailable_reason.is_some());
        assert_eq!(missing.path, root.directory.join("source.js"));

        let expected = manifest
            .expected_bundle_provenance(test, bundle, source)
            .expect("frame bundle inputs");
        fs::write(root.directory.join("bundle.js.kiln-inputs"), expected)
            .expect("write provenance");
        let ready = manifest.resolve_program(test);
        assert_eq!(ready.path, root.directory.join("bundle.js"));
        assert!(ready.unavailable_reason.is_none());

        fs::write(root.directory.join("bundle.js"), "replaced output\n")
            .expect("replace generated bundle");
        let replaced = manifest.resolve_program(test);
        assert!(
            replaced.unavailable_reason.is_some(),
            "the provenance record authorizes exact output bytes, not just a path"
        );
        fs::write(root.directory.join("bundle.js"), bundle).expect("restore generated bundle");

        fs::write(root.directory.join("build.lock"), "changed build input\n")
            .expect("change declared input");
        let stale = manifest.resolve_program(test);
        assert_eq!(stale.path, root.directory.join("source.js"));
        assert!(stale.unavailable_reason.is_some());
    }

    #[test]
    fn fixtures_are_not_tests() {
        let text =
            format!("{ONE_TEST}\n[[fixture]]\nid = \"big\"\npath = \"big.js\"\nbytes = 12\n");
        let manifest = manifest(&text).expect("manifest is well formed");
        assert_eq!(manifest.tests().len(), 1);
    }

    #[test]
    fn tests_are_selected_by_the_directory_a_run_was_pointed_at() {
        let text = format!(
            "{ONE_TEST}\n[[test]]\nid = \"effect/one\"\nsource = \"effect/src/one.ts\"\nexpected_stdout = \"\"\"\nx\n\"\"\"\n"
        );
        let manifest = manifest(&text).expect("manifest is well formed");
        let under_guard: Vec<_> = manifest
            .tests_under(Path::new("/corpus/guard"))
            .map(|test| test.id.as_str())
            .collect();
        assert_eq!(under_guard, ["guard/probe"]);
        assert_eq!(manifest.tests_under(Path::new("/corpus")).count(), 2);
        assert_eq!(
            manifest.tests_under(Path::new("/elsewhere")).count(),
            0,
            "a manifest that describes nothing under a root must not claim that root"
        );
    }

    #[test]
    fn subtree_selection_normalizes_paths_and_rejects_escaping_inputs() {
        let text = format!(
            "{ONE_TEST}\n[[test]]\nid = \"escape\"\nsource = \"guard/../outside.js\"\nexpected_stdout = \"\"\n"
        );
        let manifest = manifest(&text).expect("manifest is well formed");

        // Raw starts_with would incorrectly claim `escape` for /corpus/guard.  A path that
        // escapes the requested subtree must not become runnable merely because its spelling
        // begins with that subtree.
        let selected: Vec<_> = manifest
            .tests_under(Path::new("/corpus/./guard"))
            .map(|test| test.id.as_str())
            .collect();
        assert_eq!(selected, ["guard/probe"]);
    }

    #[cfg(unix)]
    #[test]
    fn subtree_selection_rejects_a_symlinked_input_outside_the_root() {
        use std::os::unix::fs::symlink;

        let fixture = TemporaryManifest::new();
        let guard = fixture.directory.join("guard");
        let outside = fixture.directory.join("outside");
        fs::create_dir_all(&guard).expect("create requested subtree");
        fs::create_dir_all(&outside).expect("create outside directory");
        fs::write(outside.join("probe.js"), "console.log('outside');\n")
            .expect("write outside source");
        symlink(&outside, guard.join("linked")).expect("create escaping source symlink");

        let manifest = Manifest::from_text(
            "[[test]]\nid = \"escape\"\nsource = \"guard/linked/probe.js\"\nexpected_stdout = \"\"\n",
            fixture.directory.clone(),
        )
        .ok()
        .expect("manifest is well formed");
        assert_eq!(
            manifest.tests_under(&guard).count(),
            0,
            "a symlink to a file outside the requested subtree must not be selected"
        );
    }

    #[cfg(unix)]
    #[test]
    fn subtree_selection_rejects_a_dangling_symlink_even_if_its_target_appears_later() {
        use std::os::unix::fs::symlink;

        let fixture = TemporaryManifest::new();
        let guard = fixture.directory.join("guard");
        let target = fixture.directory.join("created-later");
        fs::create_dir_all(&guard).expect("create requested subtree");
        symlink(&target, guard.join("linked")).expect("create dangling source symlink");

        let manifest = Manifest::from_text(
            "[[test]]\nid = \"escape\"\nsource = \"guard/linked/probe.js\"\nexpected_stdout = \"\"\n",
            fixture.directory.clone(),
        )
        .ok()
        .expect("manifest is well formed");
        assert_eq!(
            manifest.tests_under(&guard).count(),
            0,
            "a dangling symlink is not an ordinary missing source"
        );

        // A source appearing after discovery must not turn a previously unsafe path into a
        // runnable one on the next scan.
        fs::create_dir_all(&target).expect("create symlink target later");
        fs::write(target.join("probe.js"), "console.log('outside');\n")
            .expect("write target source later");
        assert_eq!(
            manifest.tests_under(&guard).count(),
            0,
            "a later-created target remains outside the requested subtree"
        );
    }

    #[test]
    fn a_malformed_manifest_names_the_file_and_the_line() {
        let missing = manifest("[[test]]\nid = \"a\"\nsource = \"a.js\"\n")
            .expect_err("expected_stdout is required");
        assert!(missing.to_string().contains("manifest.toml:1"), "{missing}");
        assert!(missing.to_string().contains("expected_stdout"), "{missing}");

        let wrong_type =
            manifest("[[test]]\nid = [\"a\"]\nsource = \"a.js\"\nexpected_stdout = \"\"\n")
                .expect_err("id must be a string");
        assert!(
            wrong_type.to_string().contains("must be a string"),
            "{wrong_type}"
        );

        let syntax = manifest("[[test]]\nid = \"a\"\nid = \"b\"\n").expect_err("duplicate key");
        assert!(syntax.to_string().contains("manifest.toml:3"), "{syntax}");

        let duplicate = manifest(&format!("{ONE_TEST}{ONE_TEST}")).expect_err("duplicate id");
        assert!(
            duplicate
                .to_string()
                .contains("duplicate test id `guard/probe`"),
            "{duplicate}"
        );
    }

    #[test]
    fn the_repositorys_own_manifest_reads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../conformance/manifest.toml");
        let manifest = Manifest::read(&path).expect("the corpus manifest is in the subset");
        assert!(!manifest.tests().is_empty());
        for (group, representative) in [
            ("effect", "effect/01-core"),
            ("guard", "guard/g06-array-proto-iterator"),
            ("regress", "regress/r14-dynamic-import-attributes"),
            ("server", "server/s01-http-hello"),
        ] {
            let root = manifest.directory().join(group);
            assert!(
                manifest
                    .tests_under(&root)
                    .any(|test| test.id == representative),
                "manifest is missing representative {representative}"
            );
        }
        assert_eq!(
            manifest
                .tests_under(&manifest.directory().join("effect"))
                .count(),
            12
        );
        assert_eq!(
            manifest
                .tests_under(&manifest.directory().join("guard"))
                .count(),
            20
        );
        let probe = manifest
            .tests()
            .iter()
            .find(|test| test.id == "guard/g06-array-proto-iterator")
            .expect("the one-line probe is declared");
        assert_eq!(probe.expected_stdout, "function\n");
        assert!(probe.features.contains(&"iterator-protocol".to_owned()));
        let dynamic_import = manifest
            .tests()
            .iter()
            .find(|test| test.id == "regress/r14-dynamic-import-attributes")
            .expect("the dynamic import regression is declared");
        assert_eq!(
            dynamic_import.source,
            Path::new("regress/r14-dynamic-import-attributes.mjs")
        );
        assert!(
            dynamic_import
                .features
                .contains(&"import-attributes".to_owned())
        );
    }
}
