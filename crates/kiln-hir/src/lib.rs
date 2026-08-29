//! Kiln's owned, allocator-independent frontend representation.
//!
//! Oxc deliberately uses arena-borrowed AST nodes. Those types are excellent inside one parse,
//! but they cannot be memoized across revisions and would couple the rest of the compiler to a
//! particular parser. This crate is the ownership boundary: frontends lower into these values,
//! and no Oxc type or lifetime crosses it.

/// A byte range in UTF-8 source text, with an exclusive end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }
}

/// The grammar selected for a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    JavaScript,
    Jsx,
    TypeScript,
    Tsx,
}

/// ECMAScript parse goal. Test262 and embedders must choose this independently of file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceGoal {
    Script,
    Module,
}

/// A parser diagnostic retained as an owned value in the query database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub span: Option<Span>,
}

/// Owned module HIR shared by parser adapters, incremental queries, and execution backends.
//
// `items` preserves cheap module-structure information while `body` contains the TypeScript-
// erased executable subset. Unsupported syntax remains explicit in `body`, allowing later
// compiler passes to diagnose it without confusing syntax checking with backend coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub source_kind: SourceKind,
    pub source_goal: SourceGoal,
    pub items: Vec<Item>,
    /// Executable, TypeScript-erased module body.
    pub body: Vec<Statement>,
    pub requested_modules: Vec<RequestedModule>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
    pub diagnostics: Vec<Diagnostic>,
    /// Oxc could not recover far enough to produce a meaningful program.
    pub panicked: bool,
}

/// A function's name, parameters and body.
///
/// Boxed at its use site: this is much larger than the other statement payloads, and an unboxed
/// variant would widen `StatementKind` for every statement in the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    /// `None` for a function expression that was never given one.
    pub name: Option<String>,
    pub parameters: Vec<Parameter>,
    pub body: Vec<Statement>,
    pub span: Span,
}

/// One declared parameter.
///
/// Simple identifier bindings only. Destructuring, defaults and rest are separate features with
/// their own semantics; representing them as a plain identifier here would be a lie that later
/// passes could not detect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub span: Span,
}

/// One `key: value` pair in an object literal.
///
/// Keys are already strings: an identifier key, a string key and a numeric key all name the same
/// property, and normalising here means later passes never have to know which syntax produced it.
/// Computed keys, shorthand methods and spread are separate features and are refused rather than
/// flattened into this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectProperty {
    pub key: String,
    pub value: Expression,
    pub span: Span,
}

/// An owned executable statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementKind {
    Variable {
        kind: VariableKind,
        declarations: Vec<VariableDeclarator>,
    },
    Expression(Expression),
    Block(Vec<Statement>),
    If {
        test: Expression,
        consequent: Box<Statement>,
        alternate: Option<Box<Statement>>,
    },
    While {
        test: Expression,
        body: Box<Statement>,
    },
    Empty,
    /// A hoisted function declaration.
    ///
    /// The body is a statement list rather than a `Block` because a function body is not a block
    /// statement: its scope is the call, its parameters are bound in it, and `return` is legal
    /// only inside it. Flattening it here keeps later passes from unwrapping a block that never
    /// had block semantics.
    Function(Box<Function>),
    /// `return`, where no expression means `return undefined`.
    ///
    /// Kept as an `Option` so a frontend never has to synthesize a source-less expression to
    /// represent the absence of one.
    Return(Option<Expression>),
    /// A default expression export has runtime work and an exported binding. Keeping it distinct
    /// prevents later passes from accidentally executing and discarding the value as though it
    /// were an ordinary expression statement.
    ExportDefault(Expression),
    /// Syntactically valid JavaScript that this HIR version cannot execute yet.
    Unsupported {
        feature: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariableKind {
    Var,
    Let,
    Const,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableDeclarator {
    pub name: String,
    pub initializer: Option<Expression>,
    pub span: Span,
}

/// An owned executable expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionKind {
    Literal(Literal),
    Identifier(String),
    Unary {
        operator: UnaryOperator,
        argument: Box<Expression>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    Logical {
        operator: LogicalOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    Assignment {
        operator: AssignmentOperator,
        target: String,
        value: Box<Expression>,
    },
    Conditional {
        test: Box<Expression>,
        consequent: Box<Expression>,
        alternate: Box<Expression>,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
    /// `object.property = value`, including the compound forms.
    ///
    /// Kept separate from `Assignment` rather than widening its target into an enum: the two
    /// have genuinely different evaluation, since this one evaluates an object expression first
    /// and that one resolves a binding, and a shared shape would have every consumer branch on
    /// which it was anyway.
    MemberAssignment {
        operator: AssignmentOperator,
        object: Box<Expression>,
        property: String,
        value: Box<Expression>,
    },
    /// An object literal. Property order is source order, which is the order JavaScript
    /// enumerates string keys in and therefore not an implementation detail.
    Object {
        properties: Vec<ObjectProperty>,
    },
    StaticMember {
        object: Box<Expression>,
        property: String,
    },
    Unsupported {
        feature: String,
    },
}

/// Literal values use IEEE-754 bits so HIR equality is total, including NaN payloads and -0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Undefined,
    Null,
    Boolean(bool),
    Number(u64),
    String(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Plus,
    Negate,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Exponentiate,
    StrictEqual,
    StrictNotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOperator {
    And,
    Or,
    NullishCoalesce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentOperator {
    Assign,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Exponentiate,
}

impl Module {
    /// Runtime export names declared explicitly by this module.
    ///
    /// Type-only exports do not exist at runtime. Star exports have no local name until module
    /// resolution, so they are represented in [`Export`] but omitted here.
    pub fn runtime_export_names(&self) -> Vec<String> {
        self.exports
            .iter()
            .filter(|export| !export.type_only)
            .filter_map(|export| export.exported.as_ref())
            .map(ExportName::as_str)
            .map(str::to_owned)
            .collect()
    }

    /// Static module requests that participate in runtime instantiation/evaluation.
    ///
    /// Request-level `type_only` is not sufficient: TypeScript permits an ordinary-looking
    /// request whose individual bindings are all `type`, such as
    /// `import { type Shape } from "./shape.ts"`. Conversely, a side-effect import has no
    /// [`Import`] entry at all and is still a runtime edge.
    pub fn runtime_requested_modules(&self) -> Vec<&RequestedModule> {
        self.requested_modules
            .iter()
            .filter(|request| {
                request
                    .occurrences
                    .iter()
                    .any(|occurrence| self.occurrence_is_runtime(*occurrence))
            })
            .collect()
    }

    fn occurrence_is_runtime(&self, occurrence: ModuleRequestOccurrence) -> bool {
        if occurrence.type_only {
            return false;
        }
        match occurrence.kind {
            ModuleRequestKind::Import => {
                let mut bindings = self
                    .imports
                    .iter()
                    .filter(|import| import.statement_span == occurrence.statement_span)
                    .peekable();
                bindings.peek().is_none() || bindings.any(|import| !import.type_only)
            }
            ModuleRequestKind::Export => {
                let mut bindings = self
                    .exports
                    .iter()
                    .filter(|export| export.statement_span == occurrence.statement_span)
                    .peekable();
                bindings.peek().is_none() || bindings.any(|export| !export.type_only)
            }
        }
    }
}

/// One top-level syntactic item, reduced to information stable across parser implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Import,
    Export,
    Variable,
    Function,
    Class,
    Block,
    Expression,
    ControlFlow,
    Empty,
    TypeScript,
    Other,
}

/// A statically requested module, in first-occurrence source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedModule {
    pub specifier: String,
    pub occurrences: Vec<ModuleRequestOccurrence>,
}

/// One syntactic occurrence of a static module request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleRequestOccurrence {
    pub statement_span: Span,
    pub specifier_span: Span,
    pub type_only: bool,
    pub kind: ModuleRequestKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleRequestKind {
    Import,
    Export,
}

/// The provider-side name selected by an import or re-export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportName {
    Name(String),
    Default,
    Namespace,
}

/// One runtime or type-only import binding. Side-effect-only imports have no binding and are
/// represented by [`RequestedModule`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub source: String,
    pub imported: ImportName,
    pub local: String,
    pub type_only: bool,
    pub span: Span,
    pub statement_span: Span,
}

/// A name visible to importers of this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportName {
    Name(String),
    Default,
}

impl ExportName {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Default => "default",
        }
    }
}

/// Where an exported binding comes from. These forms are distinct in ECMAScript and must remain
/// distinct until graph linking; collapsing them loses live-binding and ambiguity semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportOrigin {
    Local {
        local: String,
    },
    /// `export default <expression>` has no source-level local binding. The IR compiler may
    /// synthesize one, but the HIR must not invent a JavaScript identifier for it.
    DefaultExpression,
    ReExport {
        source: String,
        imported: ImportName,
    },
    Star {
        source: String,
    },
    NamespaceReExport {
        source: String,
    },
    Unsupported {
        source: Option<String>,
        description: String,
    },
}

/// An ECMAScript export entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    /// The name visible to importers. `None` is reserved for `export * from ...`.
    pub exported: Option<ExportName>,
    pub origin: ExportOrigin,
    pub type_only: bool,
    pub span: Span,
    pub statement_span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_names_exclude_types_and_unresolved_stars() {
        let module = Module {
            source_kind: SourceKind::TypeScript,
            source_goal: SourceGoal::Module,
            items: Vec::new(),
            body: Vec::new(),
            requested_modules: Vec::new(),
            imports: Vec::new(),
            exports: vec![
                Export {
                    exported: Some(ExportName::Name("value".into())),
                    origin: ExportOrigin::Local {
                        local: "value".into(),
                    },
                    type_only: false,
                    span: Span::default(),
                    statement_span: Span::default(),
                },
                Export {
                    exported: Some(ExportName::Name("Shape".into())),
                    origin: ExportOrigin::Local {
                        local: "Shape".into(),
                    },
                    type_only: true,
                    span: Span::default(),
                    statement_span: Span::default(),
                },
                Export {
                    exported: None,
                    origin: ExportOrigin::Star {
                        source: "./other.js".into(),
                    },
                    type_only: false,
                    span: Span::default(),
                    statement_span: Span::default(),
                },
            ],
            diagnostics: Vec::new(),
            panicked: false,
        };

        assert_eq!(module.runtime_export_names(), ["value"]);
    }
}
