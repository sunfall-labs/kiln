//! Finding the tests under a corpus root, whichever way they declare themselves.
//!
//! Two sources sit side by side here.  Test262 frontmatter is the default for tests Kiln writes
//! itself and is the only source that can express the negative-test and strictness contracts.  A
//! manifest is the way in for a corpus that arrived from somewhere else and must stay byte for
//! byte identical to the tree it was proven in.  Both are turned into [`ConformanceTest`], so a
//! coordinator selects, runs and reports without knowing which one a test came from.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};

use kiln_test262::TestCase;

use crate::case::{RunMode, VariantResult};
use crate::manifest::{MANIFEST_FILE_NAME, Manifest, ManifestError, ManifestTest};
use crate::{program, test262};

/// Where a test's metadata came from.  Reported so a summary can say which convention a corpus
/// uses without exposing the shape of either one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provenance {
    /// A `/*--- … ---*/` block inside the test file.
    Frontmatter,
    /// A `manifest.toml` beside (or above) the test file.
    Manifest,
}

/// One discovered test, in the form the runner consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceTest {
    id: String,
    source_path: PathBuf,
    path: PathBuf,
    features: Vec<String>,
    declaration: Declaration,
}

/// The part of a test that differs by metadata source.  Private: downstream code is meant to be
/// indifferent to which source declared a test, and a public enum would invite matching on it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Declaration {
    /// Boxed so that a manifest test does not carry the footprint of a whole parsed Test262 case.
    Frontmatter(Box<TestCase>),
    /// The program must run to completion and write exactly this.
    ExpectedStdout(String),
}

impl ConformanceTest {
    /// Stable identity within the corpus.
    ///
    /// A manifest test keeps the id the manifest gave it, which does not move when a run is
    /// pointed at a subdirectory; a frontmatter test is identified by its path relative to the
    /// root, because that is the only name it has.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The file holding the program.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The authored source whose changes select this test.
    ///
    /// This differs from [`Self::path`] when a manifest points at a generated JavaScript bundle:
    /// the bundle is executed, but editing the TypeScript source must still select the case.
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// Feature tags, per `docs/design.md` §9; the changed-test selector keys on these.
    pub fn features(&self) -> &[String] {
        &self.features
    }

    pub fn provenance(&self) -> Provenance {
        match self.declaration {
            Declaration::Frontmatter(_) => Provenance::Frontmatter,
            Declaration::ExpectedStdout(_) => Provenance::Manifest,
        }
    }

    /// Run every realm variant this test calls for.
    ///
    /// A manifest test has exactly one: its expected output was observed from a single ordinary
    /// script run, so re-running it under an injected strict directive would be judging a
    /// different program against that observation.
    pub fn run(&self, mode: RunMode) -> Vec<VariantResult> {
        match &self.declaration {
            Declaration::Frontmatter(case) => test262::run_case_variants(case, mode),
            Declaration::ExpectedStdout(expected) => {
                vec![program::run_against_expected_stdout(
                    &self.path, expected, mode,
                )]
            }
        }
    }

    fn from_manifest(test: &ManifestTest, source_path: PathBuf, path: PathBuf) -> Self {
        Self {
            id: test.id.clone(),
            source_path,
            path,
            features: test.features.clone(),
            declaration: Declaration::ExpectedStdout(test.expected_stdout.clone()),
        }
    }

    fn from_frontmatter(root: &Path, case: TestCase) -> Self {
        let id = case
            .path
            .strip_prefix(root)
            .unwrap_or(&case.path)
            .to_string_lossy()
            .replace('\\', "/");
        Self {
            id,
            source_path: case.path.clone(),
            path: case.path.clone(),
            features: case.metadata.features.clone(),
            declaration: Declaration::Frontmatter(Box::new(case)),
        }
    }
}

/// Discover every test at or below `root`, ordered by id.
///
/// A `manifest.toml` at `root` or above it wins for the tests it declares there; otherwise the
/// tree is read as Test262 frontmatter.  A manifest that declares nothing under `root` is not
/// this root's manifest and is ignored, so an unrelated file further up the tree cannot capture
/// a directory it says nothing about.
pub fn discover(root: &Path) -> Result<Vec<ConformanceTest>, DiscoveryError> {
    if let Some(manifest) = governing_manifest(root)? {
        return Ok(manifest
            .tests_under(root)
            .map(|test| {
                ConformanceTest::from_manifest(
                    test,
                    manifest.source_path(test),
                    manifest.program_path(test),
                )
            })
            .collect());
    }
    let mut tests: Vec<_> = kiln_test262::discover(root)
        .map_err(DiscoveryError::Frontmatter)?
        .into_iter()
        .map(|case| ConformanceTest::from_frontmatter(root, case))
        .collect();
    tests.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(tests)
}

fn governing_manifest(root: &Path) -> Result<Option<Manifest>, DiscoveryError> {
    for ancestor in root.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        let candidate = ancestor.join(MANIFEST_FILE_NAME);
        if !candidate.is_file() {
            // A corpus's manifest lives inside the repository holding the corpus, so the search
            // stops at the repository root.  Without that bound an unrelated `manifest.toml` in a
            // parent directory - a home directory, a packaging tree - could fail a conformance run
            // it has nothing to do with, simply by not being in this reader's TOML subset.
            if ancestor.join(".git").exists() {
                break;
            }
            continue;
        }
        let manifest = Manifest::read(&candidate).map_err(DiscoveryError::Manifest)?;
        let describes_root = manifest.tests_under(root).next().is_some();
        // Stop at the first manifest either way.  Walking past one that does not mention this
        // root would let a distant file quietly take over a corpus that has its own conventions.
        return Ok(describes_root.then_some(manifest));
    }
    Ok(None)
}

/// Why a corpus could not be turned into a list of tests.
#[derive(Debug)]
pub enum DiscoveryError {
    Manifest(ManifestError),
    Frontmatter(kiln_test262::DiscoveryError),
}

impl Display for DiscoveryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(error) => write!(f, "conformance manifest: {error}"),
            Self::Frontmatter(error) => write!(f, "conformance frontmatter: {error}"),
        }
    }
}

impl Error for DiscoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Manifest(error) => Some(error),
            Self::Frontmatter(error) => Some(error),
        }
    }
}
