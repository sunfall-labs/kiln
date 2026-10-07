//! Owned results of source checking; these are contracts, never runtime proofs.
use crate::Span;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeClass {
    Number,
    String,
    Boolean,
    BigInt,
    Symbol,
    Nullish,
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
}
