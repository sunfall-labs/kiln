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

/// One `[[test]]` entry.
///
/// Fields the runner does not read — `catches`, `notes`, `effect_api`, `bundle` — are prose or
/// provenance for a human and are intentionally not modelled: parsing them would freeze an
/// editorial convention into code that has no use for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestTest {
    /// Corpus-wide identity, stable no matter which subdirectory a run was pointed at.
    pub id: String,
    /// Program source, relative to the manifest's own directory.
    pub source: PathBuf,
    /// JavaScript semantics the test would stop passing without; the changed-test selector's key.
    pub features: Vec<String>,
    /// What the corpus records about the test's runnability.  The runner does not read this: an
    /// observed result must never be steerable from the file that records the expectation.
    pub status: Option<String>,
    /// Exactly what the program must write, including its trailing newline.
    pub expected_stdout: String,
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

    /// The tests whose program lives at or below `root`.
    ///
    /// This is what lets one manifest at the top of a corpus serve `kiln test conformance/guard`
    /// and `kiln test conformance/effect` without either invocation picking up the other's tests.
    pub fn tests_under<'a>(&'a self, root: &'a Path) -> impl Iterator<Item = &'a ManifestTest> {
        self.tests
            .iter()
            .filter(move |test| self.source_path(test).starts_with(root))
    }
}

fn read_test(table: &Table) -> Result<ManifestTest, UnlocatedError> {
    Ok(ManifestTest {
        id: required_string(table, "id")?.to_owned(),
        source: PathBuf::from(required_string(table, "source")?),
        features: optional_strings(table, "features")?,
        status: optional_string(table, "status")?.map(str::to_owned),
        expected_stdout: required_string(table, "expected_stdout")?.to_owned(),
    })
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
        assert_eq!(
            manifest.source_path(test),
            Path::new("/corpus/guard/probe.js")
        );
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
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/manifest.toml");
        let manifest = Manifest::read(&path).expect("the corpus manifest is in the subset");
        // 32 ported plus the regression probes authored here, which are a separate group so
        // the ported counts keep meaning what they say.
        assert_eq!(manifest.tests().len(), 36);
        assert_eq!(
            manifest
                .tests_under(&manifest.directory().join("guard"))
                .count(),
            20
        );
        assert_eq!(
            manifest
                .tests_under(&manifest.directory().join("regress"))
                .count(),
            4
        );
        let probe = manifest
            .tests()
            .iter()
            .find(|test| test.id == "guard/g06-array-proto-iterator")
            .expect("the one-line probe is declared");
        assert_eq!(probe.expected_stdout, "function\n");
        assert!(probe.features.contains(&"iterator-protocol".to_owned()));
    }
}
