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
    pub exports: Vec<Export>,
    pub diagnostics: Vec<Diagnostic>,
    /// Oxc could not recover far enough to produce a meaningful program.
    pub panicked: bool,
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
            .filter_map(|export| export.name.clone())
            .collect()
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
    pub occurrences: Vec<Span>,
}

/// An ECMAScript export entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    /// The name visible to importers. `None` means `export * from ...`.
    pub name: Option<String>,
    /// The source module for a re-export.
    pub source: Option<String>,
    pub type_only: bool,
    pub span: Span,
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
            exports: vec![
                Export {
                    name: Some("value".into()),
                    source: None,
                    type_only: false,
                    span: Span::default(),
                },
                Export {
                    name: Some("Shape".into()),
                    source: None,
                    type_only: true,
                    span: Span::default(),
                },
                Export {
                    name: None,
                    source: Some("./other.js".into()),
                    type_only: false,
                    span: Span::default(),
                },
            ],
            diagnostics: Vec::new(),
            panicked: false,
        };

        assert_eq!(module.runtime_export_names(), ["value"]);
    }
}
