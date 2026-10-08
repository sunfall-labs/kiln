//! The JSDoc comments of a JavaScript file, parsed as TypeScript's `jsdoc.go` does.
//!
//! Like [`TypeSyntax`](crate::TypeSyntax), these values own their data and address the module's
//! source by byte offset. A checker turns the tags of the comments attached to a node into
//! ordinary annotations, casts and declarations (`reparser.go`); nothing here is attached to a
//! node, because the parser that produced it knows no nodes.

use crate::type_syntax::TypeExpr;

/// Where a diagnostic of a comment is reported from, and so whether it is reported at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsDocDiagnosticKind {
    /// A syntax error of the comment. Reported if a node the checker attaches the comment to
    /// exists (`parseJSDocComment` only runs for the nodes that call `withJSDoc`).
    Parse,
    /// A grammar error of a type in the comment, reported with the type if it is reparsed.
    Grammar,
    /// An error of the checker about the syntax of a type in the comment, reported only for the
    /// parts of a comment that are reparsed.
    Checker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocDiagnostic {
    pub kind: JsDocDiagnosticKind,
    pub start: u32,
    /// 0: the end of the token at `start`.
    pub end: u32,
    pub code: u32,
    pub args: Vec<String>,
}

/// The span of an identifier in a tag. A missing identifier has an empty span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocName {
    pub start: u32,
    pub end: u32,
    /// The token's source text with its unicode escapes decoded.
    pub text: String,
}

impl JsDocName {
    pub fn missing(at: u32) -> JsDocName {
        JsDocName {
            start: at,
            end: at,
            text: String::new(),
        }
    }

    pub fn is_missing(&self) -> bool {
        self.start == self.end
    }
}

/// `JSDocTypeExpression`: what `parseJSDocType` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocTypeExpr {
    /// The type, without the `...` before it and the `=` after it. `None`: it did not parse.
    pub ty: Option<TypeExpr>,
    /// Start position, including the `...`.
    pub pos: u32,
    /// Start of the next token.
    pub end: u32,
    /// `...T` (`JSDocVariadicType`)
    pub is_variadic: bool,
    /// `T=` (`JSDocOptionalType`)
    pub is_optional: bool,
}

/// The type of a `@param`, `@property` or `@typedef` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsDocTagType {
    None,
    Expr(JsDocTypeExpr),
    /// `JSDocTypeLiteral`: built from the `@property` or `@param` tags that follow.
    Literal {
        properties: Vec<JsDocTag>,
        is_array: bool,
        pos: u32,
    },
}

/// `JSDocParameterOrPropertyTag`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocProperty {
    /// `a.b.c`
    pub name: Vec<JsDocName>,
    /// `[name]`, `[name=default]`
    pub is_bracketed: bool,
    pub is_name_first: bool,
    pub ty: JsDocTagType,
    /// The text of a tag nested in another tag. Empty for any other tag.
    pub comment: String,
}

/// `JSDocSignature`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocSignature {
    /// `@param` and `@this` tags.
    pub params: Vec<JsDocTag>,
    /// The type of the `@returns` tag.
    pub ret: Option<JsDocTypeExpr>,
    pub pos: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsDocTypeParameterModifier {
    Const,
    In,
    Out,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocTypeParameter {
    /// Position of its first token: the `[`, a modifier, or the name.
    pub pos: u32,
    pub name: JsDocName,
    pub modifiers: Vec<(JsDocTypeParameterModifier, u32)>,
    /// `[T=Default]`
    pub default: Option<JsDocTypeExpr>,
    pub end: u32,
}

/// `JSDocTemplateTag`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocTemplate {
    /// `@template {Constraint} T`: applies to the first type parameter.
    pub constraint: Option<JsDocTypeExpr>,
    pub params: Vec<JsDocTypeParameter>,
}

/// The name of a `@typedef` or a `@callback`: `A.B.C` is `C` in the namespaces `A` and `B`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocDeclaredName {
    pub namespaces: Vec<JsDocName>,
    pub name: JsDocName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocTypedef {
    pub name: JsDocDeclaredName,
    pub ty: JsDocTagType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocCallback {
    pub name: JsDocDeclaredName,
    pub signature: JsDocSignature,
}

/// `ExpressionWithTypeArguments`, of `@implements`, `@augments` and `@extends`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocClassName {
    /// `a.b.c`
    pub name: Vec<JsDocName>,
    pub type_args: Option<Vec<TypeExpr>>,
    /// Start of the token after the type arguments.
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocImportSpecifier {
    /// Position of its first token: `type`, or the first name.
    pub start: u32,
    pub is_type_only: bool,
    pub imported: String,
    pub imported_pos: u32,
    pub local: String,
    pub local_pos: u32,
}

/// `import("x", { with: { "resolution-mode": "import" } })`, or the same clause of a `@import`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsDocResolutionMode {
    None,
    Import,
    Require,
}

/// `JSDocImportTag`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JsDocImport {
    /// `importClause != nil`
    pub has_clause: bool,
    pub default: Option<JsDocName>,
    pub namespace: Option<JsDocName>,
    /// Position of the first token of the import clause, and of the `*` of `* as namespace`.
    pub clause_start: u32,
    pub clause_end: u32,
    pub namespace_start: u32,
    pub named: Vec<JsDocImportSpecifier>,
    /// `importClause.NamedBindings` is a `NamedImports`.
    pub has_named_imports: bool,
    /// The module specifier and its position. `None` if it is not a string.
    pub specifier: Option<(String, u32)>,
    pub mode: Option<JsDocResolutionMode>,
    /// Start of the next token.
    pub end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsDocModifier {
    Public,
    Private,
    Protected,
    Readonly,
    Override,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsDocTagKind {
    Type(JsDocTypeExpr),
    Satisfies(JsDocTypeExpr),
    /// With the text of the tag.
    This(JsDocTypeExpr, String),
    Return(Option<JsDocTypeExpr>),
    /// `@param`, `@arg`, `@argument`
    Param(JsDocProperty),
    /// `@property`, `@prop`. Only under a `@typedef`.
    Property(JsDocProperty),
    Template(JsDocTemplate),
    Typedef(JsDocTypedef),
    Callback(JsDocCallback),
    Overload(JsDocSignature),
    Import(JsDocImport),
    Implements(JsDocClassName),
    /// `@augments`, `@extends`, with the tag name that was used.
    Augments(JsDocClassName, String),
    /// `@public`, `@private`, `@protected`, `@readonly`, `@override`
    Modifier(JsDocModifier),
    /// Any other tag. Ignored.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocTag {
    pub kind: JsDocTagKind,
    /// Position of the `@`.
    pub pos: u32,
    /// Position of the name after the `@`.
    pub name_pos: u32,
    /// The start of the next tag, or else the position before the `*/`.
    pub end: u32,
}

/// A JSDoc comment that has tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocComment {
    /// Start of its `/**` and end of its `*/`.
    pub start: u32,
    pub end: u32,
    pub tags: Vec<JsDocTag>,
    pub diagnostics: Vec<JsDocDiagnostic>,
}

/// One comment of the file, as `GetLeadingCommentRanges` sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommentRange {
    pub start: u32,
    pub end: u32,
    /// `isJSDocLikeText`
    pub jsdoc_like: bool,
    /// `KindSingleLineCommentTrivia`
    pub single_line: bool,
    /// `{@link` or `@see` is in it (`TokenFlagsPrecedingJSDocWithSeeOrLink`).
    pub see_or_link: bool,
}

/// The comments of a JavaScript file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JsDocSyntax {
    /// Every comment, in source order.
    pub comments: Vec<CommentRange>,
    /// The JSDoc comments that have tags, in source order.
    pub docs: Vec<JsDocComment>,
    /// `TokenFlagsPrecedingJSDocLeadingAsterisks`: the `*` at the start of a line of a type in a
    /// JSDoc comment, which the scanner passed over as trivia of the next token. Sorted.
    pub asterisks: Vec<u32>,
}

impl JsDocSyntax {
    /// Index of the comment that starts at `start`.
    pub fn at(&self, start: u32) -> Option<usize> {
        self.docs.binary_search_by_key(&start, |doc| doc.start).ok()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}
