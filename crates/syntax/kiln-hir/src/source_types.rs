//! Owned results of source checking; these are contracts, never runtime proofs.
use crate::{Span, TypeAnnotation};
use std::collections::BTreeMap;
/// Checker metadata beside a project's sources: the symbolic links among the directories of its
/// packages, as a JSON object from the absolute path of each link to the absolute path it names.
/// The project's own `node_modules` holds it, so no module or package can have its path.
pub const CHECKER_LINKS_FILE: &str = "node_modules/.kiln-links.json";

/// Metadata format identity, included in the versioned bytecode contract.
pub const SOURCE_TYPE_METADATA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordLayoutId(pub u32);

impl std::fmt::Display for RecordLayoutId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NativeFieldType {
    Number,
    Int32,
    Boolean,
    String,
    Record(RecordLayoutId),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordField {
    pub name: String,
    pub ty: NativeFieldType,
    pub readonly: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordLayout {
    pub fields: Vec<RecordField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceTypeAnnotations {
    pub version: u16,
    pub checker_identity: String,
    pub parameters: Vec<Option<TypeAnnotation>>,
    pub return_type: Option<TypeAnnotation>,
    pub record_layouts: Vec<RecordLayout>,
    pub parameter_layouts: Vec<Option<RecordLayoutId>>,
    pub return_layout: Option<RecordLayoutId>,
    /// Maps authored span start offset (`decl.span.start` or `expr.span.start`) to its `RecordLayoutId`.
    pub span_layouts: BTreeMap<u32, RecordLayoutId>,
    /// Maps authored `(span.start, span.end)` to its `RecordLayoutId`.
    pub exact_span_layouts: BTreeMap<(u32, u32), RecordLayoutId>,
    /// Maps local slot index (`u32`) in this function/program body to its `RecordLayoutId`.
    pub local_layouts: BTreeMap<u32, RecordLayoutId>,
    /// Maps `Instruction::NewObject` bytecode PC (`u32`) in this function/program body to its `RecordLayoutId`.
    pub new_object_layouts: BTreeMap<u32, RecordLayoutId>,
    pub parameter_array_elements: Vec<Option<NativeFieldType>>,
    pub return_array_element: Option<NativeFieldType>,
    /// Maps authored span start offset (`decl.span.start` or `expr.span.start`) to its homogeneous array element type.
    pub span_array_elements: BTreeMap<u32, NativeFieldType>,
    /// Maps authored `(span.start, span.end)` to its homogeneous array element type.
    pub exact_span_array_elements: BTreeMap<(u32, u32), NativeFieldType>,
    /// Maps local slot index (`u32`) in this function/program body to its homogeneous array element type.
    pub local_array_elements: BTreeMap<u32, NativeFieldType>,
    /// Maps `Instruction::NewArray` bytecode PC (`u32`) in this function/program body to its homogeneous array element type.
    pub new_array_elements: BTreeMap<u32, NativeFieldType>,
}

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
    pub record_layout: Option<RecordLayoutId>,
    pub array_element: Option<NativeFieldType>,
}

impl SourceType {
    pub fn new(
        path: String,
        span: Span,
        display: String,
        class: TypeClass,
        role: TypeRole,
    ) -> Self {
        Self {
            path,
            span,
            display,
            class,
            role,
            record_layout: None,
            array_element: None,
        }
    }

    pub const fn with_record_layout(mut self, layout: Option<RecordLayoutId>) -> Self {
        self.record_layout = layout;
        self
    }

    pub const fn with_array_element(mut self, element: Option<NativeFieldType>) -> Self {
        self.array_element = element;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDiagnostic {
    pub path: Option<String>,
    pub span: Option<Span>,
    pub code: Option<u32>,
    pub message: String,
    /// Other locations the checker points at (`'age' is declared here.`), each with its own path,
    /// span and message and no related entries of its own.
    pub related: Vec<SourceDiagnostic>,
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
    pub record_layouts: Vec<RecordLayout>,
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
