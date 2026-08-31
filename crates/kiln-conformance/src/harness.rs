//! The Test262 assertion harness, loaded once and shared by every case.
//!
//! Test262 keeps its assertion library in a `harness/` directory beside the tests rather than
//! inside them, so a test that says `assert.sameValue(x, 1)` is only meaningful when `assert.js`
//! has been evaluated in the same realm first.  A runner that does not load it cannot execute
//! anything except the handful of `raw` tests, which is why this exists.
//!
//! The files are read once and held as text.  There are a few dozen of them and every one of the
//! suite's hundred thousand variants needs at least two, so reading them per case would dominate
//! the run; holding them costs a few hundred kilobytes and nothing else.  The text is never
//! interpreted here — assembling it with the case source is [`crate::test262`]'s job.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The two files Test262 evaluates before every non-`raw` test, in the order it evaluates them.
pub const DEFAULT_INCLUDES: [&str; 2] = ["assert.js", "sta.js"];

/// Marks a throw that means "this runner does not provide that host facility".
///
/// A test that asks for a capability the host has not implemented has not found a language bug,
/// and reporting it as a failure would bury the ones that are.  The classifier looks for this
/// text and answers [`crate::Outcome::Unsupported`] instead.
pub const HOST_CAPABILITY_MARKER: &str = "kiln: host capability not provided:";

/// The `$262` host object required by Test262's INTERPRETING.md.
///
/// `$262` is supplied by the *host*, not by the language, so it belongs here rather than in the
/// interpreter: it is the runner's statement about what this embedding can do.  `global` is real;
/// every facility Kiln cannot yet provide throws a marked error so the case is classified as
/// unsupported rather than failing with `$262 is not defined`, which says nothing about why.
///
/// `detachArrayBuffer` is spelled as a `transfer` whose result is dropped rather than as a hook
/// of its own, because that *is* detaching: `transfer` is the operation the language defines for
/// giving a buffer's bytes away, and it leaves the receiver detached as its whole purpose.  A
/// second, host-only path to the same state would be a second thing to keep correct, and the
/// engine would then have a way to detach a buffer that no program could reach.
/// The harness file Test262 requires for every `async` test, per INTERPRETING.md.
pub const ASYNC_INCLUDE: &str = "doneprintHandle.js";

/// The text `$DONE` prints when an async test finished successfully.
pub const ASYNC_COMPLETE: &str = "Test262:AsyncTestComplete";

/// The prefix `$DONE` prints when an async test finished with an error.
pub const ASYNC_FAILURE: &str = "Test262:AsyncTestFailure:";

pub const HOST_OBJECT_PRELUDE: &str = r#"var print = function (message) { console.log(message); };
var $262 = {
  global: globalThis,
  agent: undefined,
  createRealm: function () {
    throw new Error("kiln: host capability not provided: $262.createRealm");
  },
  evalScript: function () {
    throw new Error("kiln: host capability not provided: $262.evalScript");
  },
  detachArrayBuffer: function (buffer) {
    ArrayBuffer.prototype.transfer.call(buffer);
  },
  gc: function () {
    throw new Error("kiln: host capability not provided: $262.gc");
  }
};
"#;

/// A loaded Test262 `harness/` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Harness {
    root: PathBuf,
    sources: BTreeMap<String, String>,
}

impl Harness {
    /// Find and load the harness for a discovery root.
    ///
    /// Test262 is laid out as `<checkout>/test/…` beside `<checkout>/harness/…`, and a runner is
    /// normally pointed at the `test` directory or a subtree of it, so the harness is found by
    /// walking upwards.  A tree with no harness answers `None` rather than failing: Kiln's own
    /// corpus is a legitimate Test262-shaped tree that has no harness and needs none.
    pub fn discover(start: &Path) -> Option<Self> {
        let mut directory = if start.is_dir() {
            Some(start)
        } else {
            start.parent()
        };
        while let Some(current) = directory {
            let candidate = current.join("harness");
            if candidate.join("assert.js").is_file() {
                return Self::load(&candidate).ok();
            }
            directory = current.parent();
        }
        None
    }

    /// Load every `.js` file in one harness directory.
    pub fn load(root: &Path) -> Result<Self, std::io::Error> {
        let mut sources = BTreeMap::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "js")
                && let Some(name) = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            {
                sources.insert(name, fs::read_to_string(&path)?);
            }
        }
        Ok(Self {
            root: root.to_owned(),
            sources,
        })
    }

    /// The directory this harness was loaded from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The source of one harness file, by its Test262 `includes` name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.sources.get(name).map(String::as_str)
    }

    /// The files a case needs, in evaluation order: the default pair, then its own `includes`.
    ///
    /// A name this harness does not have is returned as an error rather than skipped, because a
    /// silently omitted include turns a real assertion into an undefined-variable failure that
    /// looks like a language bug.
    pub fn prelude_for(&self, includes: &[String]) -> Result<Vec<&str>, String> {
        let mut sources = Vec::with_capacity(DEFAULT_INCLUDES.len() + includes.len());
        let names = DEFAULT_INCLUDES
            .iter()
            .copied()
            .chain(includes.iter().map(String::as_str));
        for name in names {
            match self.get(name) {
                Some(source) => sources.push(source),
                None => {
                    return Err(format!(
                        "harness file `{name}` is not in {}",
                        self.root.display()
                    ));
                }
            }
        }
        Ok(sources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("kiln-harness-{label}-{stamp}"));
        fs::create_dir_all(root.join("harness")).expect("create harness");
        fs::create_dir_all(root.join("test").join("nested")).expect("create tests");
        fs::write(root.join("harness").join("assert.js"), "var assert = 1;\n").expect("assert");
        fs::write(root.join("harness").join("sta.js"), "var sta = 2;\n").expect("sta");
        fs::write(
            root.join("harness").join("compareArray.js"),
            "var ca = 3;\n",
        )
        .expect("ca");
        root
    }

    #[test]
    fn finds_the_harness_by_walking_up_from_a_subtree() {
        let root = scratch("walk");
        let harness = Harness::discover(&root.join("test").join("nested")).expect("found");
        assert_eq!(harness.root(), root.join("harness"));
        assert_eq!(harness.get("assert.js"), Some("var assert = 1;\n"));
    }

    #[test]
    fn a_tree_without_a_harness_is_not_an_error() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("kiln-harness-none-{stamp}"));
        fs::create_dir_all(&root).expect("create");
        assert_eq!(Harness::discover(&root), None);
    }

    #[test]
    fn prelude_is_the_default_pair_then_includes_and_reports_a_missing_file() {
        let root = scratch("prelude");
        let harness = Harness::discover(&root.join("test")).expect("found");
        let prelude = harness
            .prelude_for(&["compareArray.js".to_owned()])
            .expect("prelude");
        assert_eq!(
            prelude,
            ["var assert = 1;\n", "var sta = 2;\n", "var ca = 3;\n"]
        );

        let missing = harness
            .prelude_for(&["propertyHelper.js".to_owned()])
            .expect_err("missing include is an error");
        assert!(missing.contains("propertyHelper.js"));
    }

    #[test]
    fn host_prelude_detach_array_buffer_uses_the_language_transfer_operation() {
        assert!(HOST_OBJECT_PRELUDE.contains(
            "detachArrayBuffer: function (buffer) {\n    ArrayBuffer.prototype.transfer.call(buffer);"
        ));
    }
}
