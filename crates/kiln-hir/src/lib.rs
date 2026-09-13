//! Kiln's owned, allocator-independent frontend representation.
//!
//! Oxc deliberately uses arena-borrowed AST nodes. Those types are excellent inside one parse,
//! but they cannot be memoized across revisions and would couple the rest of the compiler to a
//! particular parser. This crate is the ownership boundary: frontends lower into these values,
//! and no Oxc type or lifetime crosses it.

use std::sync::Arc;

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
    /// The text every [`Span`] in this module indexes into.
    ///
    /// A span is a byte range and nothing else, so without the text beside it it names a
    /// position in something already dropped. Diagnostics got away with that by re-reading the
    /// file; `Function.prototype.toString` cannot, because the source it must answer with may
    /// have come from an `eval` or the `Function` constructor and was never a file at all.
    ///
    /// Shared rather than copied: a nested function's span lies inside its enclosing function's,
    /// so retaining text per function would store the outermost body once per function inside
    /// it. One buffer per module and a span per function is the same information at a fraction
    /// of the size.
    pub source: Arc<str>,
    pub source_kind: SourceKind,
    pub source_goal: SourceGoal,
    /// The body opened with a `"use strict"` directive prologue.
    ///
    /// A directive is not a statement -- it is a string literal the language reads before the
    /// body runs -- so it cannot survive as one, and every pass below here would otherwise have
    /// to re-parse the source text to learn whether the code it is lowering may throw on a
    /// refused write. A Module goal is strict whatever its text says; this records only what the
    /// text said, and the goal is applied where the goal is known.
    pub strict: bool,
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

/// A literal Node loader request discovered by the source frontend.
///
/// These are lazy runtime edges rather than ordinary static imports: the target is resolved when
/// the loader call executes, but the closed-world graph still needs the literal target available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRuntimeRequest {
    pub specifier: String,
    /// The filename captured by `createRequire(import.meta.url)`, or the source module path for
    /// `process.getBuiltinModule`. Kept as source identity so relative requests resolve through
    /// the ordinary resolver with the same importer base.
    pub importer: String,
}

/// Coarse TypeScript type annotation hints.
///
/// Order the trusted annotations by Static Hermes's published results:
/// (number, int, boolean, string).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TypeAnnotation {
    Number,
    Int,
    Boolean,
    String,
    Void,
    Any,
    Other,
}

impl TypeAnnotation {
    pub fn is_number(&self) -> bool {
        matches!(self, TypeAnnotation::Number | TypeAnnotation::Int)
    }

    pub fn is_int(&self) -> bool {
        matches!(self, TypeAnnotation::Int)
    }

    pub fn is_boolean(&self) -> bool {
        matches!(self, TypeAnnotation::Boolean)
    }

    pub fn is_string(&self) -> bool {
        matches!(self, TypeAnnotation::String)
    }
}

/// A function's name, parameters and body.
///
/// Boxed at its use site: this is much larger than the other statement payloads, and an unboxed
/// variant would widen `StatementKind` for every statement in the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    /// `None` for a function expression that was never given one.
    pub name: Option<String>,
    /// The source wrote that name, rather than it being inferred from what the function was
    /// assigned to.
    ///
    /// The two are not the same thing. A *written* name on a function expression is visible
    /// inside the body and nowhere else, which is what lets a recursive function expression call
    /// itself. An inferred one is metadata -- `f.name` and a stack frame -- and binding it would
    /// invent a scope the program did not write, so that `var f = function () {}; f = 1;` would
    /// stop seeing the reassignment from inside.
    pub binds_own_name: bool,
    /// An arrow function. It has no `arguments` of its own and takes `this` lexically, so the
    /// distinction has to survive lowering even while neither is implemented yet.
    pub arrow: bool,
    /// A generator. Calling it produces an object rather than running the body.
    pub generator: bool,
    /// An `async` function. Calling it starts the body and hands back a promise, and each `await`
    /// inside it suspends exactly the way a `yield` does -- which is why the two share a
    /// representation from here down.
    pub asynchronous: bool,
    pub parameters: Vec<Parameter>,
    /// `...rest`, which binds an array of the arguments beyond the declared parameters. Separate
    /// from `parameters` because it takes no positional slot and its arity is not fixed.
    pub rest: Option<Pattern>,
    /// TypeScript return type annotation hint, if present.
    pub return_type: Option<TypeAnnotation>,
    pub body: Vec<Statement>,
    /// The body opened with a `"use strict"` directive of its own.
    ///
    /// Only what this body's own prologue said. Strictness is inherited -- a function written
    /// inside strict code is strict without saying so -- and the inheriting is the compiler's,
    /// which is the only place that knows what encloses what.
    pub strict: bool,
    pub span: Span,
}

/// One declared parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    /// A parameter binds a pattern, not a name: `f(a)`, `f(a = 1)` and `f({ a })` differ only in
    /// which pattern they bind, and a default is a [`Pattern::WithDefault`] like any other.
    pub pattern: Pattern,
    /// TypeScript type annotation hint, if present.
    pub type_annotation: Option<TypeAnnotation>,
    pub span: Span,
}

/// One arm of a `switch`. `test` is `None` for `default`, which may appear anywhere among them
/// and still falls through like any other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchCase {
    pub test: Option<Expression>,
    pub body: Vec<Statement>,
}

/// One `key: value` pair in an object literal.
///
/// Keys are already strings: an identifier key, a string key and a numeric key all name the same
/// property, and normalising here means later passes never have to know which syntax produced it.
/// A spread is not one of these -- see [`ObjectMember`] -- and getters and setters are refused
/// rather than flattened into this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectProperty {
    pub key: ObjectKey,
    pub value: Expression,
    /// How the pair was written, which matters for exactly one key.
    ///
    /// `{ __proto__: v }` sets the object's prototype; `{ __proto__ }` and `{ __proto__() {} }`
    /// are ordinary own properties named `__proto__`. They are three separate productions in the
    /// grammar and nothing about the key or the value tells them apart, so the syntax is recorded
    /// here rather than the conclusion drawn from it.
    pub form: PropertyForm,
    pub span: Span,
}

/// Which object-literal production a property came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyForm {
    /// `{ key: value }`, the only form that can set a prototype.
    KeyValue,
    /// `{ key }`, which is `{ key: key }` everywhere except the `__proto__` rule.
    Shorthand,
    /// `{ key() {} }`, including the `async` and generator spellings.
    Method,
}

/// One entry in an object literal.
///
/// A spread is not a property with a special key: it copies whatever its source has *at the
/// point it appears*, so a property written after it wins and one written before it loses. That
/// ordering is only representable by keeping members in one source-ordered list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectMember {
    Property(ObjectProperty),
    Spread(Expression),
    /// `get x() { … }` or `set x(v) { … }` in a literal. Its own member kind rather than a
    /// property whose value is a function, because reading one is a call.
    Accessor {
        key: ObjectKey,
        kind: MethodKind,
        function: Function,
    },
}

/// How an object literal names a property.
///
/// A computed key is not just a string that happens to be written differently: it is an
/// arbitrary expression, evaluated in source order between the properties around it, and it can
/// produce a symbol. Collapsing it into a string at lowering would lose both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectKey {
    Static(String),
    Computed(Expression),
    /// `#name`. Not a string key that happens to start with a hash: a private name is not
    /// inherited, is not enumerable, and cannot be reached by any computed access -- so the only
    /// thing it shares with a string key is where it is written.
    Private(String),
}

/// A class: a constructor function plus methods installed on its prototype.
///
/// Kept as its own node rather than desugared in the frontend. The desugaring is real -- a class
/// *is* a constructor and a prototype -- but doing it here would lose which methods were static,
/// which were accessors, and where `super` appeared, all of which later passes need and none of
/// which survives a lowering to plain assignments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    pub name: Option<String>,
    /// The name a *named evaluation* supplies to an anonymous class: `var C = class {}` reports
    /// `"C"`.
    ///
    /// Separate from `name` because the two do different things. A written name becomes a
    /// binding visible inside the class body -- `class A { m() { return A } }` resolves `A` from
    /// inside `m`, and nowhere else -- while an inferred one is metadata and nothing more.
    /// Folding it into `name` would invent a scope the program did not write, which is exactly
    /// why the frontend refused to infer a class name at all and `var C = class {}` read back
    /// the empty string that no engine produces.
    pub inferred_name: Option<String>,
    pub superclass: Option<Expression>,
    /// `None` when the class declares no constructor; one is synthesized.
    pub constructor: Option<Function>,
    pub methods: Vec<ClassMethod>,
    /// Field initialisers and `static { }` blocks, in source order.
    ///
    /// An instance field runs inside the constructor, after `super()` and before the body, and
    /// it is evaluated in the class's enclosing scope -- so a field can close over a variable
    /// the class was defined next to. Keeping them here rather than rewriting them into the
    /// constructor at lowering preserves that they are fields.
    ///
    /// Blocks share the list rather than having one of their own because the two interleave and
    /// the order is observable: `class C { static a = f(); static { g() } }` runs `f` before
    /// `g`, and two lists could only put every block before or after every field.
    pub initialisers: Vec<ClassInitialiser>,
    pub span: Span,
}

/// One thing a class body *runs*, as opposed to one it merely installs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassInitialiser {
    Field(ClassField),
    /// `static { ... }`: a body evaluated once, where the class is defined, with `this` bound to
    /// the constructor.
    ///
    /// A whole function rather than a splice of statements because that is what the block is:
    /// its `var` declarations stay inside it, `return` is a syntax error rather than a return
    /// from the enclosing function, and `this` is the constructor rather than whatever `this`
    /// the surrounding code had.
    StaticBlock(Function),
}

impl ClassInitialiser {
    /// The field this is, for the passes that walk keys and initialiser expressions and have
    /// nothing to say about a block.
    pub const fn as_field(&self) -> Option<&ClassField> {
        match self {
            Self::Field(field) => Some(field),
            Self::StaticBlock(_) => None,
        }
    }

    /// The block this is, for the passes that walk function bodies.
    pub const fn as_static_block(&self) -> Option<&Function> {
        match self {
            Self::StaticBlock(body) => Some(body),
            Self::Field(_) => None,
        }
    }
}

/// One element of a list built by evaluating expressions in order.
///
/// Shared by array literals and argument lists, which are the same construct: both evaluate their
/// entries left to right and both let a spread contribute however many an iterable holds. Keeping
/// one type means the spread rule is written once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListElement {
    Item(Expression),
    /// A hole: `[1, , 3]`. Not an element whose value is `undefined` -- it is *absent*, which
    /// `in`, `Object.keys` and the array methods can all tell apart.
    Hole,
    /// `...xs`, which contributes every element of an iterable rather than one value.
    Spread(Expression),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchClause {
    /// `None` for the optional-binding form, `catch { }`.
    pub binding: Option<String>,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassField {
    pub key: ObjectKey,
    /// `None` for `x;`, which initialises to undefined.
    pub value: Option<Expression>,
    pub is_static: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassMethod {
    pub key: ObjectKey,
    pub function: Function,
    /// Static methods go on the constructor; instance methods go on its prototype.
    pub is_static: bool,
    /// Whether this defines the property or reads and writes it.
    pub kind: MethodKind,
}

/// How a member relates to the property it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    /// An ordinary method: the property *is* the function.
    Normal,
    /// `get x()`: reading the property calls this.
    Get,
    /// `set x(v)`: writing the property calls this.
    Set,
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
    /// `do body while (test)`. Its own node rather than a `while` with the body copied in front:
    /// duplicating the body would duplicate its side effects in the source the diagnostics point
    /// at, and would double the code size of a nested one.
    DoWhile {
        body: Box<Statement>,
        test: Expression,
    },
    /// `switch (discriminant) { case … }`.
    ///
    /// Cases are in source order and keep their bodies separate, because a matched case falls
    /// through into the *next* one rather than ending -- so the bodies are one run of statements
    /// entered at different points, not a set of alternatives.
    Switch {
        discriminant: Expression,
        cases: Vec<SwitchCase>,
    },
    /// `for (init; test; update) body`.
    ///
    /// Kept as its own node rather than desugared into `while`: `continue` must run the update,
    /// which a `while` desugaring loses, and a `let` binding in the initialiser is scoped to the
    /// loop rather than to the enclosing block.
    For {
        init: Option<Box<Statement>>,
        test: Option<Expression>,
        update: Option<Expression>,
        body: Box<Statement>,
    },
    /// `with (object) body`.
    ///
    /// The object becomes a scope: every name the body mentions is looked for on it before the
    /// bindings around it, and found there unless `Symbol.unscopables` says otherwise. That is
    /// the one construct in the language whose scope is decided at run time, so it cannot be
    /// desugared into a block over a temporary -- the names inside it are not the names a
    /// compiler can see.
    With {
        object: Expression,
        body: Box<Statement>,
    },
    /// `label: body`.
    ///
    /// Kept rather than dropped, because a label is not decoration: it is what lets a `break` or
    /// `continue` name a loop other than the innermost one, and on a non-loop statement it makes
    /// a block a `break` can leave.
    Labelled {
        label: String,
        body: Box<Statement>,
    },
    /// `for (const name in object) body`.
    ///
    /// Enumerates the object's own enumerable string keys. Not lowered to a `for-of` over a key
    /// array: a key deleted by the body must not be visited, which needs a check the loop itself
    /// performs and an array snapshot cannot express.
    ForIn {
        kind: VariableKind,
        name: String,
        object: Expression,
        body: Box<Statement>,
    },
    /// `for (const name of iterable) body`.
    ///
    /// The binding is fresh on each iteration for `let` and `const`, which is what makes a
    /// closure created in the body capture that iteration's value rather than the last one.
    ForOf {
        kind: VariableKind,
        /// A pattern, so `for (const [key, value] of entries)` binds both.
        pattern: Pattern,
        iterable: Expression,
        body: Box<Statement>,
        /// `for await (… of …)`: take the async iterator when there is one, and await each step.
        ///
        /// A flag rather than a separate statement, because the two differ only in where the
        /// awaits go -- the iterator protocol, the per-iteration binding, and closing the
        /// iterator on an early exit are the same loop.
        awaiting: bool,
    },
    /// `throw expression`.
    Throw(Expression),
    /// `try { block } catch (param) { handler } finally { cleanup }`.
    Try {
        block: Vec<Statement>,
        /// `None` for `try/finally`; the binding is `None` for `catch { }` with no parameter.
        handler: Option<CatchClause>,
        /// Empty when there is no `finally`.
        finalizer: Vec<Statement>,
    },
    /// `break`, optionally labelled.
    Break {
        label: Option<String>,
    },
    /// `continue`, optionally labelled.
    Continue {
        label: Option<String>,
    },
    Empty,
    /// A hoisted function declaration.
    ///
    /// The body is a statement list rather than a `Block` because a function body is not a block
    /// statement: its scope is the call, its parameters are bound in it, and `return` is legal
    /// only inside it. Flattening it here keeps later passes from unwrapping a block that never
    /// had block semantics.
    Function(Box<Function>),
    /// A class declaration.
    Class(Box<Class>),
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
    /// `using x = expr`: a `const` binding whose value is disposed when its scope is left.
    ///
    /// A kind rather than a flag beside one, because everything else about the binding -- block
    /// scope, the temporal dead zone, the refusal to reassign -- is exactly `const`, and the
    /// disposal is the only thing that reads it.
    Using,
    /// `await using x = expr`: as [`VariableKind::Using`], disposed asynchronously.
    AwaitUsing,
}

impl VariableKind {
    /// Whether a binding of this kind registers its value for disposal, and whether the disposal
    /// is awaited.
    #[must_use]
    pub fn disposal(self) -> Option<bool> {
        match self {
            Self::Using => Some(false),
            Self::AwaitUsing => Some(true),
            Self::Var | Self::Let | Self::Const => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableDeclarator {
    pub pattern: Pattern,
    pub initializer: Option<Expression>,
    pub span: Span,
}

/// A binding target.
///
/// A pattern is a tree because destructuring nests, and defaults are a wrapper rather than a
/// field on each shape: `[a = 1]`, `{ b = 2 }` and `[[c = 3]]` are the same construct at
/// different depths, and giving every shape its own optional default would say otherwise.
///
/// Three of the shapes store through a reference rather than binding a name. They belong here
/// even though a declaration can never use them -- `let [o.p] = xs` is a syntax error -- because
/// `[o.p, q] = xs` mixes them with names freely, and the alternative is a second pattern tree
/// that differs from this one only in which leaves it admits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    Identifier(String),
    /// `[object.property] = xs`. The same target shape as
    /// [`ExpressionKind::MemberAssignment`], and split from the computed and private forms for
    /// the same reason it is: the three evaluate different things before the store.
    Member {
        object: Box<Expression>,
        property: String,
    },
    /// `[object[property]] = xs`, mirroring [`ExpressionKind::ComputedAssignment`].
    Computed {
        object: Box<Expression>,
        property: Box<Expression>,
    },
    /// `[object.#name] = xs`, mirroring [`ExpressionKind::PrivateAssignment`]: the field has to
    /// already exist on the object, and it not existing is a TypeError rather than a property
    /// being created.
    PrivateMember {
        object: Box<Expression>,
        name: String,
    },
    /// `{ a, b: renamed, ...rest }`
    Object {
        properties: Vec<PatternProperty>,
        /// A pattern rather than a name, because `({ ...o.rest } = src)` stores the leftovers
        /// through a reference. A declaration's rest is always an [`Pattern::Identifier`].
        rest: Option<Box<Pattern>>,
    },
    /// `[a, , b, ...rest]`. `None` is a hole, which binds nothing.
    Array {
        elements: Vec<Option<Pattern>>,
        rest: Option<Box<Pattern>>,
    },
    /// `pattern = default`, where the default applies only when the value is `undefined`.
    WithDefault {
        inner: Box<Pattern>,
        default: Box<Expression>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternProperty {
    pub key: PatternKey,
    pub value: Pattern,
}

/// Which property of the source a pattern property reads.
///
/// Not an [`ObjectKey`]: a pattern cannot name a private field as the *source* of a property --
/// `({ #x: a } = o)` is not syntax -- and a key that could be private would put a case here that
/// no lowering can produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternKey {
    /// `{ a: x }`, `{ "a": x }` and `{ 0: x }`, all normalised to the string they name.
    Static(String),
    /// `{ [expression]: x }`, evaluated where the source wrote it -- before the value is read
    /// out of the source, and once even when a rest has to exclude the key it produced.
    Computed(Expression),
}

impl Pattern {
    /// The single name this pattern binds, when it binds exactly one.
    pub fn as_identifier(&self) -> Option<&str> {
        match self {
            Self::Identifier(name) => Some(name),
            _ => None,
        }
    }
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
        arguments: Vec<ListElement>,
        /// `a?.()`: when the callee is nullish the whole enclosing chain is undefined and the
        /// call does not happen -- so the arguments are not evaluated either.
        optional: bool,
    },
    /// `super(...)` in a derived constructor: run the parent constructor against this instance
    /// rather than allocating a new one.
    SuperCall {
        arguments: Vec<ListElement>,
    },
    /// `super.property`, which resolves on the parent's prototype rather than on `this`.
    SuperMember {
        property: String,
    },
    /// `super[expression]`. The same lookup as [`ExpressionKind::SuperMember`], with the key
    /// computed rather than written -- kept apart from it so the key stays an expression that
    /// has to be evaluated, in the order the language evaluates it.
    SuperComputedMember {
        property: Box<Expression>,
    },
    /// `x++`, `++x`, `x--`, `--x`.
    ///
    /// Not desugared into an assignment: the postfix form evaluates to the *old* value, which a
    /// plain `x = x + 1` cannot express, and the distinction is invisible once lowered.
    Update {
        increment: bool,
        prefix: bool,
        target: String,
    },
    /// `object.property++`, and the other three spellings.
    ///
    /// Separate from [`ExpressionKind::Update`] because the target is a place rather than a name:
    /// the object is evaluated once and both the read and the write go through that one
    /// reference, which a name never needs.
    UpdateMember {
        increment: bool,
        prefix: bool,
        object: Box<Expression>,
        property: Box<ObjectKey>,
    },
    /// `yield value` and `yield* iterable`.
    ///
    /// An expression, not a statement: it evaluates to whatever the caller sends back with the
    /// next `next(v)`, which is the whole reason generators can receive values.
    Yield {
        delegate: bool,
        value: Option<Box<Expression>>,
    },
    /// `this`. Not an identifier: it cannot be declared, assigned or shadowed, and modelling it
    /// as a name would let a binding called `this` capture it.
    This,
    /// A class expression.
    Class(Box<Class>),
    /// A function expression or arrow function used as a value.
    Function(Box<Function>),
    /// An array literal. Holes (`[1, , 2]`) are not represented: they are a distinct concept
    /// from `undefined` elements and are refused by the frontend rather than flattened here.
    Array {
        elements: Vec<ListElement>,
    },
    /// `a, b, c`: every operand is evaluated, and the last one is the value.
    Sequence {
        operands: Vec<Expression>,
    },
    /// `object[property]`, where the key is computed rather than written.
    ComputedMember {
        object: Box<Expression>,
        property: Box<Expression>,
        /// `?.[`, with the same whole-chain short circuit as [`ExpressionKind::StaticMember`].
        optional: bool,
    },
    /// `#x in object`: does this object carry the class's private field?
    ///
    /// A separate node from [`ExpressionKind::PrivateMember`], because reading a private field
    /// the object does not have is a TypeError while asking about one is the whole point.
    PrivateIn {
        name: String,
        object: Box<Expression>,
    },
    /// `new.target`: the constructor a `new` call named, or `undefined` outside one.
    ///
    /// Not an identifier -- `new` is a keyword and `target` is not a property of it -- so it
    /// cannot be resolved like `this` is; it needs a node of its own.
    NewTarget,
    /// `[a, b] = pair` and `({ a } = record)`: an assignment whose target is a pattern.
    ///
    /// Separate from [`ExpressionKind::Assignment`], whose target is one name, because the shapes
    /// have nothing in common past the `=`. Only `=` can take a pattern -- `[a] += x` is not a
    /// thing -- so no operator is carried.
    PatternAssignment {
        pattern: Pattern,
        value: Box<Expression>,
    },
    /// `object[property] = value`, including the compound forms.
    ComputedAssignment {
        operator: AssignmentOperator,
        object: Box<Expression>,
        property: Box<Expression>,
        value: Box<Expression>,
    },
    /// `new callee(...arguments)`.
    New {
        callee: Box<Expression>,
        arguments: Vec<ListElement>,
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
    /// `super.property = value`, including the compound forms.
    SuperMemberAssignment {
        operator: AssignmentOperator,
        property: String,
        value: Box<Expression>,
    },
    /// `super[property] = value`, including the compound forms.
    SuperComputedAssignment {
        operator: AssignmentOperator,
        property: Box<Expression>,
        value: Box<Expression>,
    },
    /// An object literal. Property order is source order, which is the order JavaScript
    /// enumerates string keys in and therefore not an implementation detail.
    Object {
        members: Vec<ObjectMember>,
    },
    /// The boundary of an optional chain.
    ///
    /// It has to be marked, because `?.` does not short-circuit only its own access: in
    /// `a?.b.c`, a nullish `a` makes the *whole* expression undefined rather than reading `.c` of
    /// undefined. The chain is the region those jumps leave, and nothing smaller says where it
    /// ends.
    Chain {
        expression: Box<Expression>,
    },
    /// A template literal. `quasis` is always one longer than `expressions`, so the two
    /// interleave starting and ending with text -- an empty string standing in where the source
    /// wrote none.
    ///
    /// Not lowered to concatenation in the frontend: the desugaring is real but it belongs in one
    /// place, and doing it here would put a `+` in the HIR that the source never wrote.
    Template {
        quasis: Vec<String>,
        expressions: Vec<Expression>,
    },
    /// The template object a tagged template hands its tag as the first argument.
    ///
    /// An array of the cooked strings carrying a `raw` array of the same length. `cooked` is
    /// `None` for a segment whose escape is not valid -- a syntax error in an untagged template
    /// and `undefined` here, because the whole point of a tag is to see text the language would
    /// have rejected.
    ///
    /// Separate from [`ExpressionKind::Call`] rather than folded into it, so a tagged template
    /// is an ordinary call whose first argument happens to be built this way, and every rule
    /// about how a callee is evaluated stays written once.
    TemplateStrings {
        cooked: Vec<Option<String>>,
        raw: Vec<String>,
    },
    /// `await value`, which suspends until the value settles.
    ///
    /// Not lowered to `yield` in the frontend even though that is what it becomes: `await` and
    /// `yield` are different in the source and a diagnostic that named the wrong one would be
    /// confusing.
    Await(Box<Expression>),
    /// `import(specifier)`, which is an expression rather than a declaration: it names its module
    /// at run time and hands back a promise for that module's namespace.
    ///
    /// Not a [`RequestedModule`], because there is nothing to request until the specifier has been
    /// evaluated -- which is the whole difference from a static import, and why the linker cannot
    /// see one.
    ImportCall {
        specifier: Box<Expression>,
        /// The second argument, `import(s, { with: … })`. Evaluated for its side effects wherever
        /// it appears; nothing reads it yet.
        options: Option<Box<Expression>>,
    },
    /// `object.#name`.
    ///
    /// Its own node because the lookup is different in kind: a private name is found on the
    /// object itself or nowhere, and "nowhere" is a TypeError rather than `undefined`. That
    /// own-only rule *is* the brand check -- an object of another class simply does not have it.
    PrivateMember {
        object: Box<Expression>,
        name: String,
        /// Whether the access was written `?.#name`, which short-circuits the whole chain to
        /// `undefined` on a nullish object rather than asking it for the name.
        optional: bool,
    },
    /// `object.#name = value`, including the compound forms.
    ///
    /// Separate from [`ExpressionKind::MemberAssignment`] for the same reason the read is
    /// separate: the target must already exist, and it not existing is an error rather than a
    /// property being created.
    PrivateAssignment {
        operator: AssignmentOperator,
        object: Box<Expression>,
        name: String,
        value: Box<Expression>,
    },
    /// A regular expression literal, kept as its source rather than as a compiled program: the
    /// pattern is a value the program can read back through `source`, and compiling it belongs to
    /// whoever owns the engine.
    RegExp {
        pattern: String,
        flags: String,
    },
    /// `delete name`, where `name` is an unqualified identifier rather than a property access.
    ///
    /// A separate node from [`ExpressionKind::Delete`] because the operand is a *binding* and
    /// resolving one is the IR compiler's job, not the frontend's: the answer is `false` for
    /// anything a scope declared, and only for a name no scope declared does it become the
    /// global object's `[[Delete]]`.  Strict code cannot spell this at all -- the parser reports
    /// the early error -- so reaching here means the containing body is sloppy.
    DeleteBinding {
        name: String,
    },
    /// `delete <expression>` where the operand is not a reference at all -- `delete 0`,
    /// `delete f()`, `delete this`.
    ///
    /// The specification evaluates the operand, discards it and answers `true`; the operand is
    /// kept rather than dropped because it may have side effects that a program observes.
    DeleteValue {
        argument: Box<Expression>,
    },
    /// `delete object.property`.
    ///
    /// Not a unary operator over a member expression: the operand must not be *read*, and a
    /// unary node would say that it is.
    Delete {
        object: Box<Expression>,
        /// Boxed because an `ObjectKey` can hold an `Expression`, and a variant of
        /// `ExpressionKind` that owns one directly has no finite size.
        property: Box<ObjectKey>,
    },
    StaticMember {
        object: Box<Expression>,
        property: String,
        /// `?.`, which stops the whole enclosing [`ExpressionKind::Chain`] when the object is
        /// nullish rather than only this one access.
        optional: bool,
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
    /// `123n`, kept as the digits the source wrote.
    ///
    /// Not parsed here: a BigInt is arbitrary-precision, so there is no fixed-width field to put
    /// it in, and the decimal text is both exact and the shortest thing that is.
    BigInt(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Plus,
    Negate,
    Not,
    /// `typeof`. Note that `typeof undeclaredName` is legal and yields "undefined" rather than
    /// throwing, which is why it cannot be lowered as an ordinary operand evaluation.
    TypeOf,
    /// `~x`, which is `-(ToInt32(x) + 1)` and so is a numeric operator despite looking logical.
    BitwiseNot,
    /// `void x`: evaluate the operand for its effects, then discard it.
    Void,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    /// `key in object`. Presence, which a read-and-compare cannot express.
    In,
    /// `value instanceof constructor`: is the constructor's prototype anywhere on the value's
    /// prototype chain.
    InstanceOf,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Exponentiate,
    StrictEqual,
    StrictNotEqual,
    /// `==`, which compares across types after converting. Not `StrictEqual` with a coercion
    /// bolted on: the conversion rules are their own algorithm and `null == undefined` is true
    /// while neither converts to the other.
    LooseEqual,
    LooseNotEqual,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    /// `<<`, `>>` and `>>>`. All three truncate their operands to 32 bits first, which is why
    /// `1 << 31` is negative and `x >>> 0` is the idiom for reading a number as unsigned.
    ShiftLeft,
    ShiftRight,
    UnsignedShiftRight,
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
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
    UnsignedShiftRight,
    /// `&&=`, `||=` and `??=`. These do not compute and store: they *test* the current value and
    /// assign only if it is the wrong one, so the right-hand side may never be evaluated and the
    /// store may never happen. That is why they are not a `BinaryOperator` with an assignment
    /// wrapped round it.
    LogicalAnd,
    LogicalOr,
    Nullish,
}

impl AssignmentOperator {
    /// Whether this assigns conditionally, evaluating its right-hand side only when it must.
    pub fn is_short_circuiting(self) -> bool {
        matches!(self, Self::LogicalAnd | Self::LogicalOr | Self::Nullish)
    }
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

    /// Escape analysis pass for arguments to `isolate(expr)`.
    ///
    /// Checks that objects passed to `isolate()` do not escape via:
    /// - Local `let`/`const` alias bindings (e.g. `let alias = x; isolate(x);` or `isolate(x); let alias = x;`).
    /// - Capture in outer closures / nested functions.
    /// - Modification or reassignment after `isolate()`.
    /// - Container property assignment.
    ///
    /// Emits compile-time diagnostics into `self.diagnostics` naming any escaping alias.
    pub fn check_isolate_escapes(&mut self) {
        let mut diags = Vec::new();
        check_statements_escape(&self.body, &mut diags);
        self.diagnostics.extend(diags);
    }
}

fn is_isolate_call(callee: &Expression) -> bool {
    match &callee.kind {
        ExpressionKind::Identifier(name) => name == "isolate",
        ExpressionKind::StaticMember { property, .. } => property == "isolate",
        _ => false,
    }
}

fn collect_isolated_names(expr: &Expression, names: &mut Vec<(String, Span)>) {
    match &expr.kind {
        ExpressionKind::Identifier(name) => {
            names.push((name.clone(), expr.span));
        }
        ExpressionKind::Object { members } => {
            for member in members {
                if let ObjectMember::Property(prop) = member {
                    collect_isolated_names(&prop.value, names);
                }
            }
        }
        _ => {}
    }
}

fn collect_pattern_names(pat: &Pattern, names: &mut std::collections::HashSet<String>) {
    match pat {
        Pattern::Identifier(name) => {
            names.insert(name.clone());
        }
        Pattern::Member { .. } | Pattern::Computed { .. } | Pattern::PrivateMember { .. } => {}
        Pattern::Object { properties, rest } => {
            for prop in properties {
                collect_pattern_names(&prop.value, names);
            }
            if let Some(rest) = rest {
                collect_pattern_names(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for elem in elements.iter().flatten() {
                collect_pattern_names(elem, names);
            }
            if let Some(rest) = rest {
                collect_pattern_names(rest, names);
            }
        }
        Pattern::WithDefault { inner, .. } => collect_pattern_names(inner, names),
    }
}

fn collect_free_variables_in_function(
    func: &Function,
    set: &mut std::collections::HashSet<String>,
) {
    let mut bound = std::collections::HashSet::new();
    for param in &func.parameters {
        collect_pattern_names(&param.pattern, &mut bound);
    }
    if let Some(rest) = &func.rest {
        collect_pattern_names(rest, &mut bound);
    }
    if let Some(name) = &func.name {
        bound.insert(name.clone());
    }

    let mut used = std::collections::HashSet::new();
    for stmt in &func.body {
        collect_used_identifiers_in_statement(stmt, &mut used, &mut bound);
    }
    for u in used {
        if !bound.contains(&u) {
            set.insert(u);
        }
    }
}

fn collect_used_identifiers_in_statement(
    stmt: &Statement,
    used: &mut std::collections::HashSet<String>,
    bound: &mut std::collections::HashSet<String>,
) {
    match &stmt.kind {
        StatementKind::Variable { declarations, .. } => {
            for d in declarations {
                if let Some(init) = &d.initializer {
                    collect_used_identifiers_in_expression(init, used);
                }
                collect_pattern_names(&d.pattern, bound);
            }
        }
        StatementKind::Expression(e)
        | StatementKind::Throw(e)
        | StatementKind::ExportDefault(e) => {
            collect_used_identifiers_in_expression(e, used);
        }
        StatementKind::Block(stmts) => {
            for s in stmts {
                collect_used_identifiers_in_statement(s, used, bound);
            }
        }
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            collect_used_identifiers_in_expression(test, used);
            collect_used_identifiers_in_statement(consequent, used, bound);
            if let Some(alt) = alternate {
                collect_used_identifiers_in_statement(alt, used, bound);
            }
        }
        StatementKind::While { test, body } | StatementKind::DoWhile { body, test } => {
            collect_used_identifiers_in_expression(test, used);
            collect_used_identifiers_in_statement(body, used, bound);
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            if let Some(init) = init {
                collect_used_identifiers_in_statement(init, used, bound);
            }
            if let Some(test) = test {
                collect_used_identifiers_in_expression(test, used);
            }
            if let Some(update) = update {
                collect_used_identifiers_in_expression(update, used);
            }
            collect_used_identifiers_in_statement(body, used, bound);
        }
        StatementKind::ForIn {
            name, object, body, ..
        } => {
            bound.insert(name.clone());
            collect_used_identifiers_in_expression(object, used);
            collect_used_identifiers_in_statement(body, used, bound);
        }
        StatementKind::ForOf {
            pattern,
            iterable,
            body,
            ..
        } => {
            collect_pattern_names(pattern, bound);
            collect_used_identifiers_in_expression(iterable, used);
            collect_used_identifiers_in_statement(body, used, bound);
        }
        StatementKind::Function(f) => {
            if let Some(name) = &f.name {
                bound.insert(name.clone());
            }
            collect_free_variables_in_function(f, used);
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            for s in block {
                collect_used_identifiers_in_statement(s, used, bound);
            }
            if let Some(h) = handler {
                if let Some(b) = &h.binding {
                    bound.insert(b.clone());
                }
                for s in &h.body {
                    collect_used_identifiers_in_statement(s, used, bound);
                }
            }
            for s in finalizer {
                collect_used_identifiers_in_statement(s, used, bound);
            }
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            collect_used_identifiers_in_expression(discriminant, used);
            for case in cases {
                if let Some(test) = &case.test {
                    collect_used_identifiers_in_expression(test, used);
                }
                for s in &case.body {
                    collect_used_identifiers_in_statement(s, used, bound);
                }
            }
        }
        StatementKind::Return(Some(e)) => {
            collect_used_identifiers_in_expression(e, used);
        }
        _ => {}
    }
}

fn check_call_for_isolated(
    callee: &Expression,
    arguments: &[ListElement],
    idx: usize,
    isolated: &mut Vec<(String, Span, usize)>,
) {
    if !is_isolate_call(callee) {
        return;
    }
    if let Some(ListElement::Item(arg)) = arguments.first() {
        let mut names = Vec::new();
        collect_isolated_names(arg, &mut names);
        for (n, sp) in names {
            isolated.push((n, sp, idx));
        }
    }
}

fn collect_used_identifiers_in_expression(
    expr: &Expression,
    used: &mut std::collections::HashSet<String>,
) {
    match &expr.kind {
        ExpressionKind::Identifier(name) => {
            used.insert(name.clone());
        }
        ExpressionKind::Unary { argument, .. } => {
            collect_used_identifiers_in_expression(argument, used);
        }
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            collect_used_identifiers_in_expression(left, used);
            collect_used_identifiers_in_expression(right, used);
        }
        ExpressionKind::Assignment { target, value, .. } => {
            used.insert(target.clone());
            collect_used_identifiers_in_expression(value, used);
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            collect_used_identifiers_in_expression(test, used);
            collect_used_identifiers_in_expression(consequent, used);
            collect_used_identifiers_in_expression(alternate, used);
        }
        ExpressionKind::Call {
            callee, arguments, ..
        }
        | ExpressionKind::New {
            callee, arguments, ..
        } => {
            collect_used_identifiers_in_expression(callee, used);
            for arg in arguments {
                if let ListElement::Item(e) | ListElement::Spread(e) = arg {
                    collect_used_identifiers_in_expression(e, used);
                }
            }
        }
        ExpressionKind::Array { elements } => {
            for elem in elements {
                if let ListElement::Item(e) | ListElement::Spread(e) = elem {
                    collect_used_identifiers_in_expression(e, used);
                }
            }
        }
        ExpressionKind::Object { members } => {
            for member in members {
                match member {
                    ObjectMember::Property(p) => {
                        collect_used_identifiers_in_expression(&p.value, used);
                    }
                    ObjectMember::Spread(e) => {
                        collect_used_identifiers_in_expression(e, used);
                    }
                    ObjectMember::Accessor { function, .. } => {
                        collect_free_variables_in_function(function, used);
                    }
                }
            }
        }
        ExpressionKind::MemberAssignment { object, value, .. } => {
            collect_used_identifiers_in_expression(object, used);
            collect_used_identifiers_in_expression(value, used);
        }
        ExpressionKind::SuperMemberAssignment { value, .. } => {
            collect_used_identifiers_in_expression(value, used);
        }
        ExpressionKind::SuperComputedAssignment {
            property, value, ..
        } => {
            collect_used_identifiers_in_expression(property, used);
            collect_used_identifiers_in_expression(value, used);
        }
        ExpressionKind::StaticMember { object, .. }
        | ExpressionKind::Chain { expression: object } => {
            collect_used_identifiers_in_expression(object, used);
        }
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
            collect_used_identifiers_in_expression(object, used);
            collect_used_identifiers_in_expression(property, used);
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            collect_used_identifiers_in_expression(object, used);
            collect_used_identifiers_in_expression(property, used);
            collect_used_identifiers_in_expression(value, used);
        }
        ExpressionKind::Function(f) => {
            collect_free_variables_in_function(f, used);
        }
        _ => {}
    }
}

fn check_statements_escape(statements: &[Statement], diags: &mut Vec<Diagnostic>) {
    let mut aliases: Vec<(String, String)> = Vec::new();
    let mut isolated: Vec<(String, Span, usize)> = Vec::new();
    let mut captured_in_closures: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut container_escapes: Vec<(String, String)> = Vec::new();
    let mut mutations: Vec<(String, usize)> = Vec::new();

    for (idx, stmt) in statements.iter().enumerate() {
        match &stmt.kind {
            StatementKind::Variable { declarations, .. } => {
                for d in declarations {
                    let (Some(target), Some(init)) = (d.pattern.as_identifier(), &d.initializer)
                    else {
                        continue;
                    };
                    if let ExpressionKind::Identifier(src) = &init.kind {
                        aliases.push((src.clone(), target.to_owned()));
                    }
                    if let ExpressionKind::Call {
                        callee, arguments, ..
                    } = &init.kind
                    {
                        check_call_for_isolated(callee, arguments, idx, &mut isolated);
                    }
                }
            }
            StatementKind::Expression(expr) => {
                if let ExpressionKind::Call {
                    callee, arguments, ..
                } = &expr.kind
                {
                    check_call_for_isolated(callee, arguments, idx, &mut isolated);
                } else if let ExpressionKind::Assignment { target, value, .. } = &expr.kind {
                    if let ExpressionKind::Identifier(src) = &value.kind {
                        aliases.push((src.clone(), target.clone()));
                    }
                    mutations.push((target.clone(), idx));
                } else if let ExpressionKind::MemberAssignment { object, value, .. } = &expr.kind {
                    if let (ExpressionKind::Identifier(src), ExpressionKind::Identifier(cont)) =
                        (&value.kind, &object.kind)
                    {
                        container_escapes.push((src.clone(), cont.clone()));
                    }
                    if let ExpressionKind::Identifier(obj_name) = &object.kind {
                        mutations.push((obj_name.clone(), idx));
                    }
                }
            }
            StatementKind::Function(f) => {
                collect_free_variables_in_function(f, &mut captured_in_closures);
            }
            _ => {}
        }
    }

    for (name, span, isolate_idx) in isolated {
        for (orig, alias) in &aliases {
            if (orig == &name || alias == &name) && orig != alias {
                let escaping_alias = if orig == &name { alias } else { orig };
                diags.push(Diagnostic {
                    message: format!(
                        "isolate argument '{}' escapes via alias '{}'",
                        name, escaping_alias
                    ),
                    span: Some(span),
                });
            }
        }
        if captured_in_closures.contains(&name) {
            diags.push(Diagnostic {
                message: format!("isolate argument '{}' escapes into closure", name),
                span: Some(span),
            });
        }
        for (src, cont) in &container_escapes {
            if src == &name {
                diags.push(Diagnostic {
                    message: format!(
                        "isolate argument '{}' escapes via container assignment into '{}'",
                        name, cont
                    ),
                    span: Some(span),
                });
            }
        }
        for (mutated, mut_idx) in &mutations {
            if mutated == &name && *mut_idx > isolate_idx {
                diags.push(Diagnostic {
                    message: format!("isolate argument '{}' modified after isolate()", name),
                    span: Some(span),
                });
            }
        }
    }

    for stmt in statements {
        match &stmt.kind {
            StatementKind::Block(stmts) => check_statements_escape(stmts, diags),
            StatementKind::If {
                consequent,
                alternate,
                ..
            } => {
                check_statements_escape(std::slice::from_ref(consequent), diags);
                if let Some(alt) = alternate {
                    check_statements_escape(std::slice::from_ref(alt), diags);
                }
            }
            StatementKind::While { body, .. } | StatementKind::DoWhile { body, .. } => {
                check_statements_escape(std::slice::from_ref(body), diags);
            }
            StatementKind::For { body, .. }
            | StatementKind::ForIn { body, .. }
            | StatementKind::ForOf { body, .. } => {
                check_statements_escape(std::slice::from_ref(body), diags);
            }
            StatementKind::Function(f) => {
                check_statements_escape(&f.body, diags);
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                check_statements_escape(block, diags);
                if let Some(h) = handler {
                    check_statements_escape(&h.body, diags);
                }
                check_statements_escape(finalizer, diags);
            }
            StatementKind::Switch { cases, .. } => {
                for c in cases {
                    check_statements_escape(&c.body, diags);
                }
            }
            _ => {}
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

/// The module-scope local an anonymous or expression `export default` binds.
///
/// It carries characters an identifier cannot, so nothing a program writes can collide with it,
/// and it is the same name a module record uses for an anonymous default declaration -- which is
/// what lets `export default function () {}` and `export default 1 + 1` share one path.
pub const DEFAULT_EXPORT_LOCAL: &str = "*default*";

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
            source: "".into(),
            strict: false,
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

    #[test]
    fn isolate_escape_detection_flags_aliased_arguments() {
        // let orig = { a: 1 };
        // let alias = orig;
        // isolate(orig);
        let mut module = Module {
            source: Arc::from(""),
            source_kind: SourceKind::JavaScript,
            source_goal: SourceGoal::Module,
            strict: true,
            items: Vec::new(),
            body: vec![
                Statement {
                    kind: StatementKind::Variable {
                        kind: VariableKind::Let,
                        declarations: vec![VariableDeclarator {
                            pattern: Pattern::Identifier("orig".into()),
                            initializer: Some(Expression {
                                kind: ExpressionKind::Object {
                                    members: Vec::new(),
                                },
                                span: Span::default(),
                            }),
                            span: Span::default(),
                        }],
                    },
                    span: Span::default(),
                },
                Statement {
                    kind: StatementKind::Variable {
                        kind: VariableKind::Let,
                        declarations: vec![VariableDeclarator {
                            pattern: Pattern::Identifier("alias".into()),
                            initializer: Some(Expression {
                                kind: ExpressionKind::Identifier("orig".into()),
                                span: Span::default(),
                            }),
                            span: Span::default(),
                        }],
                    },
                    span: Span::default(),
                },
                Statement {
                    kind: StatementKind::Expression(Expression {
                        kind: ExpressionKind::Call {
                            callee: Box::new(Expression {
                                kind: ExpressionKind::Identifier("isolate".into()),
                                span: Span::default(),
                            }),
                            arguments: vec![ListElement::Item(Expression {
                                kind: ExpressionKind::Identifier("orig".into()),
                                span: Span::default(),
                            })],
                            optional: false,
                        },
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
            ],
            requested_modules: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            diagnostics: Vec::new(),
            panicked: false,
        };

        module.check_isolate_escapes();
        assert_eq!(module.diagnostics.len(), 1);
        assert!(
            module.diagnostics[0]
                .message
                .contains("escapes via alias 'alias'"),
            "diagnostic was: {:?}",
            module.diagnostics[0]
        );
    }

    #[test]
    fn isolate_clean_literal_produces_zero_diagnostics() {
        // isolate({ a: 1 });
        let mut module = Module {
            source: Arc::from(""),
            source_kind: SourceKind::JavaScript,
            source_goal: SourceGoal::Module,
            strict: true,
            items: Vec::new(),
            body: vec![Statement {
                kind: StatementKind::Expression(Expression {
                    kind: ExpressionKind::Call {
                        callee: Box::new(Expression {
                            kind: ExpressionKind::Identifier("isolate".into()),
                            span: Span::default(),
                        }),
                        arguments: vec![ListElement::Item(Expression {
                            kind: ExpressionKind::Object {
                                members: Vec::new(),
                            },
                            span: Span::default(),
                        })],
                        optional: false,
                    },
                    span: Span::default(),
                }),
                span: Span::default(),
            }],
            requested_modules: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            diagnostics: Vec::new(),
            panicked: false,
        };

        module.check_isolate_escapes();
        assert!(module.diagnostics.is_empty());
    }
}
