//! Owned TypeScript syntax retained beside the executable, erased HIR.
//!
//! These values deliberately contain no parser nodes, arena references, or checker IDs. Their
//! ranges address [`Module::source`](crate::Module::source), or the source of
//! [`TypeSyntax::original_module`] when that authored-source snapshot exists, so metadata remains
//! useful after the Oxc parse arena and semantic model have been dropped.

use crate::{Span, StringUnits};

/// Rich, parser-independent type syntax belonging to one module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeSyntax {
    /// Complete authored module metadata before TypeScript erasure or transformation.
    ///
    /// The sidecar module's own `type_syntax.original_module` is always `None`, so this is a
    /// single-level snapshot rather than a recursive chain. It preserves original imports,
    /// exports, items, body, and source coordinates for checker consumers while the enclosing
    /// `Module` can retain generated-JavaScript coordinates for execution.
    pub original_module: Option<std::sync::Arc<crate::Module>>,
    /// Type declarations, including aliases, interfaces, and module-level overloads.
    pub declarations: Vec<TypeDeclaration>,
    /// Type-bearing sites in executable and declaration syntax.
    pub annotations: Vec<TypeAnnotationSite>,
    /// Exact source spans of binding identifiers used to place checker pattern nodes.
    pub binding_identifiers: Vec<BindingIdentifierSite>,
    /// Declaration spans wrapped by `export`, captured from the original Oxc AST.
    pub exported_declarations: Vec<Span>,
    /// Declaration spans wrapped by `export default`, captured from the original Oxc AST.
    pub default_exported_declarations: Vec<Span>,
    /// Original class declarations and expressions, including TS-only modifiers erased from HIR.
    pub classes: Vec<TypeClassSite>,
    /// Signatures attached to original function declarations and expressions.
    pub signatures: Vec<FunctionSignatureSite>,
    /// Explicit generic arguments on calls, constructors, and instantiation expressions.
    pub instantiations: Vec<TypeInstantiationSite>,
    /// Source-level expression wrappers removed by executable HIR lowering.
    pub expression_wrappers: Vec<TypeExpressionWrapperSite>,
    /// Import attributes attached to static imports and re-exports.
    pub module_attributes: Vec<TypeModuleAttributes>,
    /// `import X = ...` aliases, which Oxc's executable HIR intentionally erases.
    pub import_equals: Vec<TypeImportEquals>,
    /// Triple-slash references retained from the parser's comment table.
    pub references: Vec<TypeReferenceDirective>,
    pub no_default_lib: bool,
    /// TypeScript executable HIR lowered before TS erasure, JSX lowering, or DCE.
    /// JavaScript uses Module::body without duplicating its executable tree.
    pub body: Vec<crate::Statement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingIdentifierSite {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeReferenceDirective {
    pub kind: TypeReferenceKind,
    pub value: String,
    pub position: u32,
    pub resolution_mode: Option<TypeResolutionMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeModuleAttributesKeyword {
    With,
    Assert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeModuleAttributeKeyKind {
    Identifier,
    StringLiteral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeModuleAttributes {
    pub statement_span: Span,
    pub keyword_span: Span,
    /// Exact `{ ... }` span, including empty clauses and any inner comments.
    pub attributes_span: Span,
    pub keyword: TypeModuleAttributesKeyword,
    pub attributes: Vec<TypeModuleAttribute>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeModuleAttribute {
    pub key: StringUnits,
    pub key_kind: TypeModuleAttributeKeyKind,
    pub value: StringUnits,
    pub span: Span,
    pub key_span: Span,
    pub value_span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeReferenceKind {
    Path,
    Types,
    Lib,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeResolutionMode {
    Import,
    Require,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeImportEquals {
    pub name: String,
    pub span: Span,
    pub reference: TypeImportEqualsReference,
    pub type_only: bool,
    pub exported: bool,
    pub declared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeImportEqualsReference {
    ExternalModule(String),
    EntityName(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignatureSite {
    pub owner_span: Span,
    pub signature: TypeSignature,
    pub has_body: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInstantiationSite {
    pub owner_span: Span,
    pub operand_span: Option<Span>,
    pub arguments: Vec<TypeExpr>,
    pub kind: TypeInstantiationKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeExpressionWrapperSite {
    pub owner_span: Span,
    /// Parentheses are stripped, but nested TS wrappers retain their own owner span.
    pub operand_span: Span,
    pub kind: TypeExpressionWrapperKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeExpressionWrapperKind {
    NonNull,
    AsConst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeInstantiationKind {
    Call,
    Construct,
    Expression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDeclaration {
    pub name: Option<String>,
    pub span: Span,
    pub kind: TypeDeclarationKind,
    pub type_parameters: Vec<TypeParameter>,
    pub extends: Vec<TypeExpr>,
    pub members: Vec<TypeMember>,
    /// Exact source spelling of the declaration, useful to consumers that support syntax not
    /// yet represented by the structured form.
    pub source_text: String,
    /// Original flags written on the declaration.
    pub exported: bool,
    pub default_exported: bool,
    pub declared: bool,
    pub const_: bool,
    pub global: bool,
    pub enum_members: Vec<EnumMember>,
    /// Executable declarations in a namespace/module body, lowered from the original AST.
    pub body: Vec<crate::Statement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumMember {
    pub name: StringUnits,
    pub name_kind: EnumMemberNameKind,
    pub span: Span,
    pub initializer: Option<crate::Expression>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumMemberNameKind {
    Identifier,
    StringLiteral,
    ComputedString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDeclarationKind {
    Class,
    Variable,
    Alias(TypeExpr),
    Interface,
    Enum,
    Namespace,
    Overload,
    Import,
    Module,
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAnnotationSite {
    pub site: TypeSite,
    /// Span of the complete syntactic owner, such as the parameter or expression.
    pub owner_span: Span,
    /// Span of the type expression itself.
    pub span: Span,
    /// For expression wrappers, the exact expression operand covered by the annotation.
    pub operand_span: Option<Span>,
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeClassSite {
    pub span: Span,
    pub name: Option<String>,
    pub name_pos: u32,
    pub exported: bool,
    pub default_exported: bool,
    pub type_parameters: Vec<TypeParameter>,
    pub extends: Option<TypeExpr>,
    pub implements: Vec<TypeExpr>,
    pub abstract_: bool,
    pub declared: bool,
    pub members: Vec<TypeClassMemberSite>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeClassMemberSite {
    pub span: Span,
    pub name: Option<String>,
    pub key: Option<crate::ObjectKey>,
    pub kind: TypeClassMemberKind,
    pub static_: bool,
    pub optional: bool,
    pub readonly: bool,
    pub abstract_: bool,
    pub accessibility: Option<TypeAccessibility>,
    pub override_: bool,
    pub key_type: Option<TypeExpr>,
    pub value_type: Option<TypeExpr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeAccessibility {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeClassMemberKind {
    Field,
    Accessor,
    Index,
    Method,
    Constructor,
    ParameterProperty,
    Getter,
    Setter,
    StaticBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSite {
    Parameter { index: u32, name: Option<String> },
    Return,
    Variable { name: Option<String> },
    Property { name: Option<String> },
    ClassField { name: Option<String> },
    AsExpression,
    SatisfiesExpression,
    TypePredicate,
    ImportType,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeExpr {
    pub span: Span,
    pub kind: TypeExprKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExprKind {
    Keyword(TypeKeyword),
    Reference {
        name: Vec<String>,
        arguments: Vec<TypeExpr>,
    },
    Union(Vec<TypeExpr>),
    Intersection(Vec<TypeExpr>),
    Literal(TypeLiteral),
    Array(Box<TypeExpr>),
    Tuple(Vec<TupleElement>),
    Object(Vec<TypeMember>),
    Function(TypeSignature),
    Constructor(TypeSignature),
    Conditional {
        check: Box<TypeExpr>,
        extends: Box<TypeExpr>,
        when_true: Box<TypeExpr>,
        when_false: Box<TypeExpr>,
    },
    Mapped {
        parameter: TypeParameter,
        constraint: Box<TypeExpr>,
        name_type: Option<Box<TypeExpr>>,
        value: Box<TypeExpr>,
        readonly: Modifier,
        optional: Modifier,
    },
    Keyof(Box<TypeExpr>),
    Infer(TypeParameter),
    Typeof {
        name: Vec<String>,
        arguments: Vec<TypeExpr>,
    },
    IndexedAccess {
        object: Box<TypeExpr>,
        index: Box<TypeExpr>,
    },
    Operator {
        operator: TypeOperator,
        operand: Box<TypeExpr>,
    },
    Template {
        quasis: Vec<StringUnits>,
        types: Vec<TypeExpr>,
    },
    Import {
        specifier: String,
        /// Import attributes/options from `import("module", options)`.
        options: Option<Box<crate::Expression>>,
        qualifier: Vec<String>,
        arguments: Vec<TypeExpr>,
        is_typeof: bool,
    },
    Predicate {
        parameter: String,
        asserts: bool,
        ty: Option<Box<TypeExpr>>,
    },
    Parenthesized(Box<TypeExpr>),
    Unsupported {
        syntax: String,
    },
    /// Preserved source form awaiting a structured lowering in a later frontend revision.
    Raw {
        syntax: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKeyword {
    Any,
    Unknown,
    Never,
    Void,
    Undefined,
    Null,
    Boolean,
    Number,
    BigInt,
    String,
    Symbol,
    Object,
    Intrinsic,
    This,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeLiteral {
    String(StringUnits),
    Number(String),
    BigInt(String),
    Boolean(bool),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    None,
    Add,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeOperator {
    Readonly,
    Unique,
    Keyof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeParameter {
    pub name: String,
    pub span: Span,
    pub constraint: Option<Box<TypeExpr>>,
    pub default: Option<Box<TypeExpr>>,
    pub const_: bool,
    pub in_: bool,
    pub out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleElement {
    pub name: Option<String>,
    pub ty: TypeExpr,
    pub optional: bool,
    pub rest: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeMember {
    pub name: Option<String>,
    /// Owned key, including computed expressions such as `[Symbol.iterator]`.
    pub key: Option<crate::ObjectKey>,
    pub span: Span,
    pub optional: bool,
    pub readonly: bool,
    pub this_parameter: Option<Box<TypeSignatureParameter>>,
    pub kind: TypeMemberKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeMemberKind {
    Property(TypeExpr),
    Method(TypeSignature),
    Call(TypeSignature),
    Construct(TypeSignature),
    Index {
        parameter: String,
        key_type: TypeExpr,
        value_type: TypeExpr,
    },
    Getter(Option<TypeExpr>),
    Setter(TypeSignature),
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSignature {
    pub span: Span,
    pub type_parameters: Vec<TypeParameter>,
    pub this_parameter: Option<Box<TypeSignatureParameter>>,
    pub parameters: Vec<TypeSignatureParameter>,
    pub rest: bool,
    pub return_type: Option<Box<TypeExpr>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSignatureParameter {
    pub name: Option<String>,
    /// Original owned binding shape, retained for destructured parameters in signatures.
    pub pattern: Option<Box<crate::Pattern>>,
    /// Exact source span of the binding pattern, excluding rest punctuation and type syntax.
    pub pattern_span: Span,
    pub span: Span,
    pub ty: Option<TypeExpr>,
    pub optional: bool,
    pub rest: bool,
    pub has_default: bool,
}
