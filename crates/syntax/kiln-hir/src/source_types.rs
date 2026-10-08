//! Owned results of source checking; these are contracts, never runtime proofs.
use crate::Span;
/// Checker metadata beside a project's sources: the symbolic links among the directories of its
/// packages, as a JSON object from the absolute path of each link to the absolute path it names.
/// The project's own `node_modules` holds it, so no module or package can have its path.
pub const CHECKER_LINKS_FILE: &str = "node_modules/.kiln-links.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeClass {
    Number,
    String,
    Boolean,
    BigInt,
    Symbol,
    Nullish,
    /// `T[]` or `ReadonlyArray<T>`: a JavaScript array at run time (if the type is true).
    Array,
    /// A tuple type; also a JavaScript array at run time.
    Tuple,
    Any,
    Unknown,
    Composite,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeRole {
    Expression,
    Parameter,
    Return,
    Annotation,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceType {
    pub path: String,
    pub span: Span,
    pub display: String,
    pub class: TypeClass,
    pub role: TypeRole,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDiagnostic {
    pub path: Option<String>,
    pub span: Option<Span>,
    pub code: Option<u32>,
    pub message: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckResult {
    pub checker_identity: String,
    pub types: Vec<SourceType>,
    pub diagnostics: Vec<SourceDiagnostic>,
    pub complete: bool,
    /// The source files of the checked program, by path, each with whether its own facts are
    /// complete: nothing it imports, directly or not, has a gap the checker could not cover. A
    /// gap in a script or in a global augmentation can change any file, so it leaves none
    /// complete. Empty when the check produced no program.
    pub files: Vec<(String, bool)>,
}

impl CheckResult {
    /// Whether the checked program contains the source file at `path`.
    pub fn covers(&self, path: &str) -> bool {
        self.file(path).is_some()
    }

    /// Whether the facts of `path` can be trusted: the whole check is complete, or nothing the
    /// file depends on has a gap.
    pub fn is_complete_for(&self, path: &str) -> bool {
        self.complete || self.file(path).is_some_and(|complete| complete)
    }

    fn file(&self, path: &str) -> Option<bool> {
        self.files
            .binary_search_by(|(file, _)| file.as_str().cmp(path))
            .ok()
            .map(|at| self.files[at].1)
    }
}
