//! Owned TypeScript syntax retained beside the executable, erased HIR.
//!
//! These values deliberately contain no parser nodes, arena references, or checker IDs. Their
//! ranges address [`Module::source`](crate::Module::source), or the source of
//! [`TypeSyntax::original_module`] when that authored-source snapshot exists, so metadata remains
//! useful after the Oxc parse arena and semantic model have been dropped.

use crate::{Span, StringUnits};

/// What a decorator decorates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeDecoratorOwner {
    Class,
    Member,
    Parameter,
}

/// `@expression` and the node it decorates, by that node's span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDecoratorSite {
    pub owner: TypeDecoratorOwner,
    pub owner_span: Span,
    pub expression: crate::Expression,
}

/// An `import.defer(specifier, options)`, for the type checker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredImportSite {
    pub span: Span,
    /// The call as an ordinary `import(..)` of the same span.
    pub call: crate::Expression,
}

/// An array or object destructuring assignment target and where its elements are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentPatternSite {
    pub span: Span,
    /// In source order, holes included for an array; not the rest element.
    pub elements: Vec<AssignmentElementSite>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssignmentElementSite {
    /// The element with its default: `a = 1`, `y: z = 1`.
    pub span: Span,
    /// What is assigned to, without the default.
    pub target: Span,
    /// The name of an object property: `y` of `y: z`, or the shorthand.
    pub key: Option<Span>,
}

/// The parameter of a `catch` clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchSite {
    /// The `try` statement.
    pub try_span: Span,
    /// The binding, without its annotation.
    pub param_span: Span,
    /// `catch (e: T)`: where `T` is, and what it is.
    pub annotation: Option<(Span, TypeExpr)>,
}

/// A `@ts-ignore` or `@ts-expect-error` comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeCommentDirective {
    /// For a `/* */` comment, the start of its last line.
    pub start: u32,
    pub end: u32,
    /// `@ts-expect-error` rather than `@ts-ignore`.
    pub expect_error: bool,
}

/// The JSX pragmas of a file's leading comments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeJsxPragmas {
    /// `@jsxRuntime`: `Some(true)` is `classic`, `Some(false)` is `automatic`.
    pub classic: Option<bool>,
    pub factory: Option<String>,
    pub fragment_factory: Option<String>,
    pub import_source: Option<String>,
}

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
    /// `export = expression`, which executable HIR refuses as a statement. The checker lowers it
    /// to `ExportAssignment` from here.
    pub export_assignments: Vec<TypeExportAssignment>,
    /// `@d` on a class, a class member or a parameter, with the expression of each, in source
    /// order. Executable HIR has none.
    pub decorators: Vec<TypeDecoratorSite>,
    /// `/// <reference ... />` comments with none of `path`, `types`, `lib` or `no-default-lib`
    /// written as TypeScript reads them, by span: an invalid directive (1084).
    pub invalid_references: Vec<Span>,
    /// `export {}` statements, which name nothing and so are in no export list, by span. One
    /// makes a declaration file export only what it says (`setExportContextFlag`).
    pub empty_exports: Vec<Span>,
    /// Array and object binding patterns and destructuring assignment targets, by span, in
    /// source order. Executable HIR patterns have no spans; the checker places its pattern nodes
    /// with these.
    pub binding_patterns: Vec<Span>,
    /// Variable declarators with a definite assignment assertion (`let x!: T`), by span.
    pub definite_variables: Vec<Span>,
    /// ES `import`/`export` statements written inside `declare module "m" { ... }` bodies.
    /// Oxc's module record covers only the program's top level and executable HIR drops these
    /// statements, so the checker lowers them from here into the ambient module's body.
    pub ambient_module_items: AmbientModuleItems,
    /// Top-level `export as namespace X` (a UMD global), which executable HIR erases. The checker
    /// lowers each to `ExportAsNamespace` from here.
    pub namespace_exports: Vec<TypeNamespaceExport>,
    /// JSX elements and fragments of `body`, which executable HIR has none of: there, each is an
    /// unsupported expression with the element's span. Sorted by start.
    pub jsx: Vec<JsxSite>,
    /// Where declarations' names start, which executable HIR does not record: `node.Name()`.
    pub names: Vec<NameSite>,
    /// Triple-slash references retained from the parser's comment table.
    pub references: Vec<TypeReferenceDirective>,
    pub no_default_lib: bool,
    /// Where `import.meta` is written, by span. Executable HIR spells it as the object or call that
    /// stands for it at run time.
    pub import_metas: Vec<Span>,
    /// The elements of each destructuring assignment target (`[a, b] = xs`, `({ x, y: z } = o)`),
    /// which executable HIR patterns have no spans for. Sorted by `span`.
    pub assignment_patterns: Vec<AssignmentPatternSite>,
    /// The `catch (param)` of each `try`, which executable HIR reduces to a name (a pattern is
    /// destructured by its first statement).
    pub catches: Vec<CatchSite>,
    /// The `import.defer(..)` expressions, which executable HIR refuses as unsupported.
    pub deferred_imports: Vec<DeferredImportSite>,
    /// `/* @jsx h */`, `@jsxFrag`, `@jsxImportSource` and `@jsxRuntime` in the block comments
    /// before the first token (`multiLinePragmas`). The last of two identical pragmas wins.
    pub jsx_pragmas: TypeJsxPragmas,
    /// `// @ts-ignore` and `// @ts-expect-error` comments, in source order
    /// (`processCommentDirective`).
    pub comment_directives: Vec<TypeCommentDirective>,
    /// `// @ts-check` (`Some(true)`) or `// @ts-nocheck` (`Some(false)`) before the first token;
    /// the last one wins.
    pub check_directive: Option<bool>,
    /// TypeScript executable HIR lowered before TS erasure, JSX lowering, or DCE.
    /// JavaScript uses Module::body without duplicating its executable tree.
    pub body: Vec<crate::Statement>,
}

/// A JSX element or fragment, for the type checker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsxSite {
    pub span: Span,
    /// The tag name: an identifier, `this`, a member access, or a string for a name with `-` or
    /// `:`. `None` for a fragment.
    pub tag: Option<crate::Expression>,
    /// The name in `</tag>`. `None` for `<tag />` and for a fragment.
    pub close_tag: Option<crate::Expression>,
    pub attributes: Vec<JsxAttributeSite>,
    pub children: Vec<JsxChildSite>,
    /// The end of the opening element or fragment.
    pub opening_end: u32,
    /// The start of `</tag>` or `</>`. `None` for `<tag />`.
    pub close_start: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsxAttributeSite {
    /// `name`, `name="text"`, `name={expression}`, `name=<element />`.
    Named {
        name: String,
        span: Span,
        /// The value, and the span of its braces if it has them.
        value: Option<(crate::Expression, Option<Span>)>,
    },
    /// `{...argument}`
    Spread {
        span: Span,
        argument: crate::Expression,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsxChildSite {
    /// Text that is not only white space with a line break.
    Text { value: String, span: Span },
    /// An element or a fragment.
    Element(crate::Expression),
    /// `{expression}`, `{}`, `{...expression}`: the expression, if any, and the braces.
    Braces {
        expression: Option<crate::Expression>,
        braces: Span,
    },
}

/// The name of a declaration, a member or a property. Found by the end of the node that owns it
/// (which decorators and `export` do not move) and its text: an identifier's name, `#name`, a
/// string literal's value, a number as written, or `[` for a computed name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameSite {
    pub owner_end: u32,
    pub name: String,
    /// The start of the name; for a computed name, of its `[`.
    pub start: u32,
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

/// The module-record view of the `import`/`export` statements of every ambient external module
/// body in a file, in the same shape as [`Module::imports`](crate::Module::imports),
/// [`Module::exports`](crate::Module::exports) and
/// [`Module::requested_modules`](crate::Module::requested_modules). A body owns the entries whose
/// `statement_span` lies inside its declaration's span. Inline `export <declaration>` is not
/// recorded: the declaration carries its own export flag, as at the top level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AmbientModuleItems {
    pub imports: Vec<crate::Import>,
    pub exports: Vec<crate::Export>,
    pub requested_modules: Vec<crate::RequestedModule>,
}

/// `export = expression`: the statement's span and its expression, lowered as executable HIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeExportAssignment {
    pub span: Span,
    pub expression: crate::Expression,
}

/// `export as namespace name`: the statement's span, the name and where the name starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeNamespaceExport {
    pub span: Span,
    pub name: String,
    pub name_start: u32,
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
    /// `( … )`: the owner span includes the parentheses, which executable HIR drops.
    Parenthesized,
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
    /// The `extends` expression when it is not a plain or dotted name: `extends mixin(A, B)`.
    pub extends_expression: Option<Box<crate::Expression>>,
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
    /// `x!: T`: the exclamation token of a property, which executable HIR drops.
    pub definite: bool,
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
    /// The initializer, when `has_default`.
    pub default: Option<Box<crate::Expression>>,
}
