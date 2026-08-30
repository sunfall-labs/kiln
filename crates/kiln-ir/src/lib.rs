//! A small, parser-independent stack bytecode for executable Kiln modules.
//!
//! The frontend owns the parser-specific lowering into [`kiln_hir`].  This crate deliberately
//! consumes only that owned HIR and produces plain data, so an interpreter, serializer, or native
//! backend can be added without coupling it to Oxc (or to any other parser).

use std::collections::{HashMap, HashSet};

use kiln_hir::{
    AssignmentOperator, BinaryOperator, CatchClause, Class, ClassField, ExportName, ExportOrigin,
    Expression, ExpressionKind, Function, ImportName, ListElement, Literal, LogicalOperator,
    Module, ObjectKey, ObjectMember, Pattern, Span, Statement, StatementKind, SwitchCase,
    UnaryOperator, VariableKind,
};

/// Host operation identifiers are shared with the runtime through the stable ABI crate.
pub use kiln_host_abi::HostOp;

/// Version of the bytecode contract produced by this crate.
pub const BYTECODE_VERSION: u16 = 1;

/// A constant embedded in a [`Program`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constant {
    Undefined,
    Null,
    Boolean(bool),
    /// IEEE-754 `f64` bits; preserving bits makes program equality total (including NaN/-0).
    Number(u64),
    String(String),
}

/// Unary operations understood by the bytecode runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Plus,
    Negate,
    Not,
    TypeOf,
    BitwiseNot,
    Void,
}

/// Binary operations understood by the bytecode runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    In,
    InstanceOf,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Exponentiate,
    StrictEqual,
    StrictNotEqual,
    LooseEqual,
    LooseNotEqual,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
    UnsignedShiftRight,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

/// Stack bytecode instructions.
///
/// Conditional jumps consume their condition.  The compiler uses [`Instruction::Dup`] before a
/// short-circuit jump when the condition must remain as the expression's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    /// Push a constant-table entry.
    Const(u32),
    LoadLocal(u32),
    StoreLocal(u32),
    Pop,
    Dup,
    /// Duplicate the top two values, preserving their order.
    ///
    /// Needed by compound computed assignment, where the object and key must be read once and
    /// used twice. Doing it with two `Dup`s would reverse them.
    Dup2,
    Unary(UnaryOp),
    Binary(BinaryOp),
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfTruthy(u32),
    JumpIfNotNullish(u32),
    HostCall {
        op: HostOp,
        argc: u16,
    },
    /// Push a callable for `Program::functions[index]`.
    MakeFunction(u32),
    /// Call the value `argc` slots below the top of the stack, consuming it and its arguments
    /// and leaving the result.
    Call {
        argc: u16,
    },
    /// Build an array from `count` values on the stack, in source order.
    NewArray {
        count: u32,
    },
    /// Replace the iterable on top of the stack with an array of everything it yields.
    ///
    /// Array destructuring goes through the iterator protocol, so `const [a, b] = someSet` works.
    /// Materialising first costs a copy and buys one binding path for every iterable, rather than
    /// an indexed path for arrays and a protocol path for everything else.
    IterableToArray,
    /// Append every element of the iterable on top of the stack to the array beneath it.
    ///
    /// Spread is a separate instruction rather than a wider `NewArray` because the count it
    /// contributes is not known until it runs: `[...xs, y]` has one static element and however
    /// many `xs` yields.
    SpreadInto,
    /// Read `object[key]`. Stack is object, then key.
    GetComputed,
    /// Call `object.name(...)` with the object as receiver. Stack is object, then arguments.
    ///
    /// A method call cannot be a `GetProperty` followed by a `Call`: that discards the receiver,
    /// and a property read must yield the *unbound* function so that `a.f === a.f` holds. Keeping
    /// the receiver on the stack until the call is what lets both be true at once.
    CallMethod {
        name: u32,
        argc: u16,
    },
    /// `object[key](...)`, the computed form of `CallMethod`.
    CallComputed {
        argc: u16,
    },
    /// Write `object[key] = value`. Stack is object, key, value.
    SetComputed,
    /// Push the current `this`.
    LoadThis,
    /// Register an exception handler at `target`, active until `PopHandler`.
    PushHandler(u32),
    PopHandler,
    /// Throw the value on top of the stack.
    Throw,
    /// Suspend the running generator, handing the value on top of the stack to its caller.
    ///
    /// Leaves whatever the caller sends with the next `next(v)` in its place, which is why
    /// `yield` is an expression rather than a statement.
    Yield,
    /// Push this frame's `arguments` as an array.
    ///
    /// Built on demand rather than on every call: most functions never mention it, and
    /// allocating one per call would tax every call to serve the few that do.
    LoadArguments,
    /// Set an object's prototype. Stack is object, then prototype; the object is left.
    SetPrototype,
    /// Run a parent constructor against the current `this`, forwarding the arguments this frame
    /// received.
    ///
    /// A derived class with no constructor behaves as `constructor(...args) { super(...args) }`.
    /// Its arity is not known where it is synthesized, so the forwarding has to happen at run
    /// time from what the frame was actually given.
    SuperForward,
    /// Call a function against the current `this` rather than a fresh or absent one.
    ///
    /// Used by both halves of `super`: `super(...)` runs the parent constructor against the
    /// instance being built, and `super.m(...)` runs a parent method against the current
    /// receiver. Both need the same thing -- an existing `this` threaded into a call.
    SuperCall {
        argc: u16,
    },
    /// Push a built-in global.
    ///
    /// Built-ins are named by index rather than resolved by string at runtime because the
    /// compiler already knows which one it means: a global lookup that could fail at runtime
    /// would be a worse contract for no benefit while the set is fixed.
    LoadBuiltin(Builtin),
    /// `new callee(...)` with `argc` arguments above the callee on the stack.
    New {
        argc: u16,
    },
    /// Build an object from `count` key/value pairs on the stack, keys first in each pair and
    /// the whole run in source order.
    NewObject {
        count: u32,
    },
    /// Read a property, naming it by constant index. The object is on top of the stack.
    ///
    /// The name is a constant rather than an inline string so the constant table deduplicates
    /// it: `.length` appearing two hundred times costs one entry.
    GetProperty(u32),
    /// Write a property. Stack is object, then value.
    SetProperty(u32),
    /// Copy every own property of the source into the target object, symbol-keyed ones included.
    ///
    /// Stack: `[target, source] -> [target]`, the same shape `SpreadInto` uses for arrays. One
    /// instruction serves both `{ ...source }` and the rest of an object pattern, because both
    /// are "everything this value has, into a fresh object".
    SpreadObject,
    /// Remove an own property, leaving `true` -- which is what the `delete` operator evaluates
    /// to. Stack: `[target] -> [true]`.
    DeleteProperty(u32),
    /// Build a regular expression from its source and flags, both constant strings.
    ///
    /// Built per evaluation rather than interned: each literal produces a fresh object, and the
    /// object carries mutable state -- `lastIndex` -- that two evaluations must not share.
    NewRegExp {
        pattern: u32,
        flags: u32,
    },
    /// Push the marker that fills a hole in an array literal.
    ///
    /// Not `undefined`: a hole is *absent*, and `in`, `Object.keys` and every array method that
    /// walks elements can tell the two apart.
    PushHole,
    /// `delete target[key]`. Stack: `[target, key] -> [true]`.
    DeleteComputed,
    /// Read `object.#name`, where the name is a constant. Stack: `[object] -> [value]`.
    ///
    /// Own-only, and a miss is a TypeError rather than `undefined` -- which is the brand check:
    /// an object of another class simply does not carry the name.
    GetPrivate(u32),
    /// Write `object.#name = value`. Stack: `[object, value] -> [value]`.
    SetPrivate(u32),
    /// Define an accessor half on an object. Stack: `[target, key, function] -> [target]`.
    ///
    /// Two halves written separately describe one property, so this merges with whichever half
    /// is already there rather than replacing it.
    DefineAccessor {
        setter: bool,
    },
    /// Put the dead-zone marker in a slot, for a `let` or `const` whose declaration has not run.
    ///
    /// Reading it is a `ReferenceError`. Emitted only where a read could actually reach it --
    /// a captured binding or a module-scope one -- because a plain local can only be read by
    /// code the compiler already checked.
    MarkUninitialized(u32),
    /// Replace a value with an array of its own enumerable string keys, in enumeration order.
    ///
    /// A snapshot, which is what the language specifies: a key *added* during the loop may or may
    /// not be visited and this chooses not to. A key *deleted* during the loop must not be, which
    /// the loop rechecks rather than this instruction predicting.
    EnumerateKeys,
    /// Call with an argument list built at runtime, for a call containing a spread.
    ///
    /// Stack: `[this, callee, arguments] -> [result]`. A separate instruction rather than a flag
    /// on `Call` because the arity is no longer in the bytecode: it is the length of an array the
    /// program computed, which is exactly what a spread means.
    Apply,
    /// `new callee(...arguments)`. Stack: `[callee, arguments] -> [instance]`.
    ConstructSpread,
    /// Put a fresh cell in a local slot, so a nested function can capture the *binding* rather
    /// than a copy of its value.
    ///
    /// Emitted only for locals a nested function actually refers to. Everything else stays an
    /// ordinary slot, so the indirection is paid exactly where the semantics require it.
    NewCell(u32),
    /// Read through the cell in a local slot.
    LoadCell(u32),
    /// Write through the cell in a local slot, so every closure sharing it observes the write.
    StoreCell(u32),
    /// Read the cell captured at this closure's upvalue index.
    LoadUpvalue(u32),
    StoreUpvalue(u32),
    /// Read a module-scope slot from inside a call frame.
    ///
    /// Distinct from `LoadLocal` because a frame's locals are its own: module bindings outlive
    /// every call and are shared by all of them, which is what module scope means in JavaScript
    /// rather than an approximation of it. Keeping the two apart also means a frame can never
    /// reach another frame's locals by accident.
    LoadGlobal(u32),
    StoreGlobal(u32),
    Return,
}

/// A compiled module body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub version: u16,
    pub constants: Vec<Constant>,
    pub code: Vec<Instruction>,
    pub local_count: u32,
    /// Compiled function bodies, referenced by [`Instruction::MakeFunction`].
    ///
    /// Held on the program rather than in the constant table because a constant is a value and
    /// a function is not: it becomes one only when `MakeFunction` runs, and once closures land
    /// the same proto will produce different callables on different evaluations.
    pub functions: Vec<FunctionProto>,
}

/// One compiled function body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionProto {
    /// An `async` function: calling it starts the body and hands back a promise. Its body is
    /// compiled as a generator, because `await` suspends exactly as `yield` does -- the machine
    /// difference is only in who resumes it.
    pub asynchronous: bool,
    /// Diagnostics and stack traces only; calls go by index.
    pub name: Option<String>,
    /// An arrow function: it has no `this` of its own and captures the defining scope's.
    pub arrow: bool,
    /// A generator: calling it builds an object rather than running the body.
    pub generator: bool,
    pub parameter_count: u32,
    /// Includes the parameters, which occupy the first slots in declaration order.
    pub local_count: u32,
    pub code: Vec<Instruction>,
    /// Where each captured cell comes from, resolved against the frame creating the closure.
    pub upvalues: Vec<Upvalue>,
}

/// A global the compiler resolves by name and the runtime provides.
///
/// This is the native half of design.md §5, which wants built-ins written in JavaScript. Nothing
/// can bootstrap a JS built-in yet -- that needs objects, prototypes and a loader, and two of
/// those have only just arrived -- so the ones the corpus needs first are native. The set is
/// deliberately tiny and each addition should be argued rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    Error,
    Symbol,
    String,
    GlobalThis,
    Set,
    Map,
    Array,
    Object,
    Json,
    // Appended rather than inserted: the discriminant is hashed into the artifact cache key, so
    // renumbering the existing ones would invalidate every cached native build.
    Math,
    Number,
    Boolean,
    /// `NaN` and `Infinity` are values rather than callables, but they reach the program the same
    /// way -- as a name the compiler resolves to something the runtime supplies.
    NotANumber,
    Infinity,
    ParseInt,
    ParseFloat,
    IsNaN,
    IsFinite,
    TypeError,
    RangeError,
    ReferenceError,
    SyntaxError,
    SetTimeout,
    SetInterval,
    ClearTimeout,
    ClearInterval,
    QueueMicrotask,
    Promise,
    RegExp,
    Date,
}

impl Builtin {
    /// The global name that resolves to this built-in.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Error" => Some(Self::Error),
            "Symbol" => Some(Self::Symbol),
            "String" => Some(Self::String),
            "Set" => Some(Self::Set),
            "Map" => Some(Self::Map),
            "globalThis" => Some(Self::GlobalThis),
            "Array" => Some(Self::Array),
            "Object" => Some(Self::Object),
            "JSON" => Some(Self::Json),
            "Math" => Some(Self::Math),
            "Number" => Some(Self::Number),
            "Boolean" => Some(Self::Boolean),
            "NaN" => Some(Self::NotANumber),
            "Infinity" => Some(Self::Infinity),
            "parseInt" => Some(Self::ParseInt),
            "parseFloat" => Some(Self::ParseFloat),
            "isNaN" => Some(Self::IsNaN),
            "isFinite" => Some(Self::IsFinite),
            "TypeError" => Some(Self::TypeError),
            "RangeError" => Some(Self::RangeError),
            "ReferenceError" => Some(Self::ReferenceError),
            "SyntaxError" => Some(Self::SyntaxError),
            "setTimeout" => Some(Self::SetTimeout),
            "setInterval" => Some(Self::SetInterval),
            "clearTimeout" => Some(Self::ClearTimeout),
            "clearInterval" => Some(Self::ClearInterval),
            "queueMicrotask" => Some(Self::QueueMicrotask),
            "Promise" => Some(Self::Promise),
            "RegExp" => Some(Self::RegExp),
            "Date" => Some(Self::Date),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Symbol => "Symbol",
            Self::String => "String",
            Self::Set => "Set",
            Self::Map => "Map",
            Self::GlobalThis => "globalThis",
            Self::Array => "Array",
            Self::Object => "Object",
            Self::Json => "JSON",
            Self::Math => "Math",
            Self::Number => "Number",
            Self::Boolean => "Boolean",
            Self::NotANumber => "NaN",
            Self::Infinity => "Infinity",
            Self::ParseInt => "parseInt",
            Self::ParseFloat => "parseFloat",
            Self::IsNaN => "isNaN",
            Self::IsFinite => "isFinite",
            Self::TypeError => "TypeError",
            Self::RangeError => "RangeError",
            Self::ReferenceError => "ReferenceError",
            Self::SyntaxError => "SyntaxError",
            Self::SetTimeout => "setTimeout",
            Self::SetInterval => "setInterval",
            Self::ClearTimeout => "clearTimeout",
            Self::ClearInterval => "clearInterval",
            Self::QueueMicrotask => "queueMicrotask",
            Self::Promise => "Promise",
            Self::RegExp => "RegExp",
            Self::Date => "Date",
        }
    }
}

/// Where a closure's captured cell is found when the closure is created.
///
/// Resolved against the *creating* frame, not the calling one: a closure captures the bindings
/// that were live where it was written, which is what makes a returned closure keep working
/// after its enclosing call has returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upvalue {
    /// A cell in the enclosing frame's local slot.
    ParentLocal(u32),
    /// A cell the enclosing closure had already captured, threaded one level further in.
    ParentUpvalue(u32),
}

/// A runtime import lowered to a local slot. The graph linker initializes the slot from the
/// provider module before evaluation; cyclic graphs are rejected by this first implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    pub source: String,
    pub imported: String,
    pub local: String,
    pub local_slot: u32,
}

/// A provider-visible name backed by one module-scope local slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportBinding {
    pub exported: String,
    pub local_slot: u32,
}

/// Linkage-aware bytecode for one module. [`Program`] stays parser- and graph-independent so the
/// existing single-file interpreter and native backend APIs remain stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleProgram {
    pub program: Program,
    /// Runtime evaluation dependencies, including side-effect-only imports, in source order.
    pub dependencies: Vec<String>,
    pub imports: Vec<ImportBinding>,
    pub exports: Vec<ExportBinding>,
}

/// A source-located compiler failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileDiagnostic {
    pub message: String,
    pub span: Option<Span>,
}

/// Compile owned HIR into stack bytecode.
pub fn compile(module: &Module) -> Result<Program, Vec<CompileDiagnostic>> {
    let mut compiler = Compiler::new(module);
    // The module body has nested functions too, so its own block bindings can be captured.

    for requested in module.runtime_requested_modules() {
        if let Some(occurrence) = requested.occurrences.first() {
            compiler.error(
                Some(occurrence.specifier_span),
                format!(
                    "module dependency `{}` is not supported by this execution backend",
                    requested.specifier
                ),
            );
        }
    }
    for export in &module.exports {
        if export.type_only {
            continue;
        }
        match &export.origin {
            ExportOrigin::ReExport { source, .. }
            | ExportOrigin::Star { source }
            | ExportOrigin::NamespaceReExport { source } => compiler.error(
                Some(export.span),
                format!("re-export from `{source}` is not supported by this execution backend"),
            ),
            ExportOrigin::Unsupported { description, .. } => compiler.error(
                Some(export.span),
                format!("unsupported export: {description}"),
            ),
            ExportOrigin::Local { .. } | ExportOrigin::DefaultExpression => {}
        }
    }

    compiler.compile_body(module);
    compiler.finish()
}

/// Compile one module while preserving the scalar linkage needed by a graph linker.
///
/// Named and default value imports are supported. Namespace imports, star/namespace re-exports,
/// indirect re-exports, default expression exports, and cycles remain explicit unsupported forms.
pub fn compile_module(module: &Module) -> Result<ModuleProgram, Vec<CompileDiagnostic>> {
    let mut compiler = Compiler::new(module);
    let mut imports = Vec::new();

    for import in module.imports.iter().filter(|import| !import.type_only) {
        let imported = match &import.imported {
            ImportName::Name(name) => name.clone(),
            ImportName::Default => "default".to_owned(),
            ImportName::Namespace => {
                compiler.error(
                    Some(import.span),
                    format!("namespace import `{}` is not supported yet", import.local),
                );
                continue;
            }
        };
        if let Some(local_slot) = compiler.declare_import(&import.local, import.span) {
            imports.push(ImportBinding {
                source: import.source.clone(),
                imported,
                local: import.local.clone(),
                local_slot,
            });
        }
    }

    compiler.compile_body(module);

    let mut exports = Vec::new();
    for export in module.exports.iter().filter(|export| !export.type_only) {
        let Some(exported) = export.exported.as_ref().map(ExportName::as_str) else {
            if !matches!(export.origin, ExportOrigin::Star { .. }) {
                compiler.error(Some(export.span), "runtime export has no exported name");
            }
            continue;
        };
        match &export.origin {
            ExportOrigin::Local { local } => match compiler.resolve_module_local(local) {
                Some(local_slot) => exports.push(ExportBinding {
                    exported: exported.to_owned(),
                    local_slot,
                }),
                None => compiler.error(
                    Some(export.span),
                    format!("export `{exported}` refers to missing local `{local}`"),
                ),
            },
            ExportOrigin::DefaultExpression => compiler.error(
                Some(export.span),
                "default expression exports are not supported yet",
            ),
            ExportOrigin::ReExport { source, .. } => compiler.error(
                Some(export.span),
                format!("re-export from `{source}` is not supported yet"),
            ),
            ExportOrigin::Star { source } => compiler.error(
                Some(export.span),
                format!("star re-export from `{source}` is not supported yet"),
            ),
            ExportOrigin::NamespaceReExport { source } => compiler.error(
                Some(export.span),
                format!("namespace re-export from `{source}` is not supported yet"),
            ),
            ExportOrigin::Unsupported { description, .. } => compiler.error(
                Some(export.span),
                format!("unsupported export: {description}"),
            ),
        }
    }

    let mut dependencies = Vec::new();
    for request in module.runtime_requested_modules() {
        if !dependencies.contains(&request.specifier) {
            dependencies.push(request.specifier.clone());
        }
    }

    compiler.finish().map(|program| ModuleProgram {
        program,
        dependencies,
        imports,
        exports,
    })
}

/// Unpatched exits from one loop.
///
/// Collected rather than patched as they are emitted, because neither target is known until the
/// loop's shape is finished: `break` needs the address after the loop and `continue` needs the
/// update clause.
/// Something that has to run when control leaves the region it guards.
///
/// A `finally` block and closing a `for-of` iterator are the same idea -- run this on every way
/// out -- and every non-local exit already knows how to walk one stack of them. Keeping them in
/// one stack is what makes `break`, `continue` and `return` close an iterator without any of them
/// learning what an iterator is.
#[derive(Clone)]
enum Cleanup {
    /// A `finally` block, re-emitted at each exit path.
    Finally(Vec<Statement>),
    /// `if (it.return) it.return()` against a hidden `for-of` binding. Held as the binding rather
    /// than as statements because it has no source form to re-compile.
    CloseIterator { binding: String },
}

/// What a `break` or a `continue` is allowed to leave.
///
/// A `switch` catches `break` and is transparent to `continue`, so the two jumps do not always
/// choose the same enclosing construct and the stack has to record which is which.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum BreakableKind {
    #[default]
    Loop,
    Switch,
    /// A labelled statement that is not a loop. `break label` leaves it; `continue label` names
    /// something that cannot be continued and is refused.
    Labelled,
}

#[derive(Default)]
struct LoopContext {
    kind: BreakableKind,
    /// The label written on this construct, if any. `break outer` names one of these.
    label: Option<String>,
    breaks: Vec<usize>,
    continues: Vec<usize>,
    /// How many cleanups were pending when the loop began, so a `break` runs only those entered
    /// inside it.
    finalizer_depth: usize,
    /// How many exception handlers were active when the loop began. A `break` out of a `try`
    /// inside the body has to pop the ones it is jumping past, or they outlive their region and
    /// catch something that is not theirs.
    handler_depth: usize,
}

#[derive(Clone, Copy)]
struct Local {
    slot: u32,
    kind: VariableKind,
    imported: bool,
    /// False between the point a block's bindings are created and the point this one's
    /// declaration runs -- the temporal dead zone.
    ///
    /// Every name a block declares exists from the moment the block is entered, because a
    /// function declared at the top of it can be called immediately and its body may refer to a
    /// `const` written further down. Reading such a name *directly*, before its declaration, is
    /// the mistake this flag catches; reading it from inside a nested function is not, and
    /// resolves as an upvalue rather than a local, so the check never sees it.
    initialized: bool,
    /// The slot holds a cell rather than the value, because a nested function refers to this
    /// name. Reads and writes go through `LoadCell`/`StoreCell` so every closure sharing the
    /// binding sees the same mutations.
    celled: bool,
}

/// The per-function half of [`Compiler`], saved and restored around a nested function body.
///
/// Constants and diagnostics are deliberately not in here: they are program-wide, and splitting
/// them would mean merging them back with index fixups for no benefit.
struct FunctionState {
    code: Vec<Instruction>,
    scopes: Vec<HashMap<String, Local>>,
    local_count: u32,
    upvalues: Vec<Upvalue>,
    /// Names already captured, so a name referenced twice captures one cell rather than two.
    upvalue_names: HashMap<String, u32>,
    /// Names this function's nested functions refer to, so declarations know to box themselves.
    captured: HashSet<String>,
}

/// Where a name lives, which decides how it is read and whether it can be read at all.
enum Resolution {
    /// A binding in the function currently being compiled.
    Local(Local),
    /// A module-scope binding, reached from inside a call frame.
    Global(u32),
    /// A binding in an enclosing *function*. That is a closure capture, and compiling it as
    /// either of the above would read an unrelated slot -- silently, and with a plausible value
    /// in it. Refused until upvalues exist.
    /// A binding in an enclosing function, reached through this closure's captured cells.
    Upvalue(u32),
    Unresolved,
}

struct Compiler {
    constants: Vec<Constant>,
    functions: Vec<FunctionProto>,
    /// Enclosing function states, module body first. Empty while compiling the module body.
    enclosing: Vec<FunctionState>,
    upvalues: Vec<Upvalue>,
    upvalue_names: HashMap<String, u32>,
    captured: HashSet<String>,
    /// Loops currently being compiled, innermost last, holding the jumps their `break` and
    /// `continue` statements still need patched.
    loops: Vec<LoopContext>,
    /// Counter for compiler-introduced bindings, so two in one scope never collide.
    hidden_bindings: usize,
    /// `finally` blocks enclosing the code being compiled, innermost last. A non-local exit has
    /// to run each of them on its way out, so it needs their bodies rather than their addresses.
    finalizers: Vec<Cleanup>,
    /// Scratch used while re-emitting cleanups, so the stack is restored afterwards.
    pending_finalizers: Vec<Cleanup>,
    /// A label written immediately before the statement now being compiled, waiting for that
    /// statement to claim it. A loop takes it into its own context; anything else gets a
    /// labelled block around it.
    pending_label: Option<String>,
    /// Short-circuit jumps of the optional chains being compiled, innermost last.
    ///
    /// A stack rather than a field because a chain can appear inside another chain's arguments,
    /// and each `?.` has to leave the chain it was written in rather than the innermost one.
    chains: Vec<Vec<usize>>,
    /// Exception handlers active at this point in the emitted code. Tracked because a non-local
    /// exit jumps past `PopHandler` instructions that would otherwise have balanced them.
    handler_depth: usize,
    /// Cleanups pending when the current function began. A `return` unwinds to here and no
    /// further: a `finally` outside the function is not left by returning from it.
    function_finalizer_depth: usize,
    /// The hidden `super` bindings of the derived classes currently being compiled, innermost
    /// last. A stack rather than a field because class bodies nest.
    super_bindings: Vec<String>,
    /// Monotonic, so two derived classes in one scope never share a binding. The stack depth
    /// cannot serve: it returns to zero after each class, and the bindings are siblings in the
    /// same scope rather than nested.
    super_binding_count: usize,
    code: Vec<Instruction>,
    /// Lexical scopes of the function being compiled, innermost last. At module level the first
    /// entry is module scope itself.
    scopes: Vec<HashMap<String, Local>>,
    /// Slots are never recycled: bytecode can jump back into a block in a loop, and a stable
    /// slot keeps that execution independent of compiler scope bookkeeping.
    local_count: u32,
    diagnostics: Vec<CompileDiagnostic>,
}

impl Compiler {
    fn new(module: &Module) -> Self {
        let mut compiler = Self {
            constants: Vec::new(),
            code: Vec::new(),
            functions: Vec::new(),
            enclosing: Vec::new(),
            upvalues: Vec::new(),
            upvalue_names: HashMap::new(),
            // The module body has nested functions of its own, so its block bindings can be
            // captured. Computed here rather than in each entry point, because there are two --
            // `compile` and `compile_module` -- and one of them would have been forgotten.
            captured: names_captured_by_nested_functions(&module.body),
            loops: Vec::new(),
            hidden_bindings: 0,
            finalizers: Vec::new(),
            pending_finalizers: Vec::new(),
            function_finalizer_depth: 0,
            handler_depth: 0,
            pending_label: None,
            chains: Vec::new(),
            super_bindings: Vec::new(),
            super_binding_count: 0,
            scopes: vec![HashMap::new()],
            local_count: 0,
            diagnostics: Vec::new(),
        };
        if module.panicked {
            compiler.error(None, "frontend could not produce an executable module");
        }
        for diagnostic in &module.diagnostics {
            compiler.diagnostics.push(CompileDiagnostic {
                message: diagnostic.message.clone(),
                span: diagnostic.span,
            });
        }
        compiler
    }

    fn compile_body(&mut self, module: &Module) {
        self.statements(&module.body);
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::Return);
    }

    fn finish(self) -> Result<Program, Vec<CompileDiagnostic>> {
        if self.diagnostics.is_empty() {
            Ok(Program {
                version: BYTECODE_VERSION,
                constants: self.constants,
                code: self.code,
                local_count: self.local_count,
                functions: self.functions,
            })
        } else {
            Err(self.diagnostics)
        }
    }

    fn declare_import(&mut self, name: &str, span: Span) -> Option<u32> {
        if self.scopes[0].contains_key(name) {
            self.error(Some(span), format!("duplicate module binding `{name}`"));
            return None;
        }
        let slot = self.local_count;
        self.local_count += 1;
        self.scopes[0].insert(
            name.to_owned(),
            Local {
                slot,
                kind: VariableKind::Const,
                imported: true,
                // An import is bound before the module body runs, so it is never in a dead zone.
                initialized: true,
                celled: false,
            },
        );
        Some(slot)
    }

    fn resolve_module_local(&self, name: &str) -> Option<u32> {
        self.scopes[0].get(name).map(|local| local.slot)
    }

    fn error(&mut self, span: Option<Span>, message: impl Into<String>) {
        self.diagnostics.push(CompileDiagnostic {
            message: message.into(),
            span,
        });
    }

    fn statement(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::Variable { kind, declarations } => {
                for declaration in declarations {
                    // A plain name keeps the direct path, which knows about `var` redeclaration
                    // and temporal-dead-zone checks. A pattern evaluates its initialiser once
                    // and binds against the result.
                    match declaration.pattern.as_identifier() {
                        Some(name) => self.variable(
                            *kind,
                            name,
                            declaration.initializer.as_ref(),
                            declaration.span,
                        ),
                        None => {
                            match &declaration.initializer {
                                Some(initializer) => self.expression(initializer),
                                // `const { a } = undefined` throws in JavaScript; there is no
                                // throw from the compiler, so it binds undefined and each
                                // property read yields undefined.
                                None => self.push_constant(Constant::Undefined),
                            }
                            self.bind_pattern(*kind, &declaration.pattern, declaration.span);
                        }
                    }
                }
            }
            StatementKind::Expression(expression) => {
                self.expression(expression);
                self.code.push(Instruction::Pop);
            }
            StatementKind::Block(statements) => {
                self.scopes.push(HashMap::new());
                self.statements(statements);
                self.scopes.pop().expect("block scope was just pushed");
            }
            StatementKind::If {
                test,
                consequent,
                alternate,
            } => self.if_statement(test, consequent, alternate.as_deref()),
            StatementKind::While { test, body } => self.while_statement(test, body),
            StatementKind::DoWhile { body, test } => self.do_while_statement(body, test),
            StatementKind::Switch {
                discriminant,
                cases,
            } => self.switch_statement(discriminant, cases, statement.span),
            StatementKind::For {
                init,
                test,
                update,
                body,
            } => self.for_statement(
                init.as_deref(),
                test.as_ref(),
                update.as_ref(),
                body,
                statement.span,
            ),
            StatementKind::Labelled { label, body } => self.labelled_statement(label, body),
            StatementKind::ForIn {
                kind,
                name,
                object,
                body,
            } => self.for_in(*kind, name, object, body, statement.span),
            StatementKind::ForOf {
                kind,
                name,
                iterable,
                body,
            } => self.for_of(*kind, name, iterable, body, statement.span),
            StatementKind::Throw(value) => {
                self.expression(value);
                self.code.push(Instruction::Throw);
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => self.try_statement(block, handler.as_ref(), finalizer, statement.span),
            StatementKind::Break { label } => self.jump_out(label.as_deref(), true, statement.span),
            StatementKind::Continue { label } => {
                self.jump_out(label.as_deref(), false, statement.span)
            }
            StatementKind::Empty => {}
            StatementKind::Function(function) => {
                self.emit_function_declaration(function, statement.span)
            }
            StatementKind::Return(value) => self.return_statement(value.as_ref(), statement.span),
            StatementKind::Class(class) => self.class_declaration(class),
            StatementKind::ExportDefault(_) => self.error(
                Some(statement.span),
                "default expression exports are not supported yet",
            ),
            StatementKind::Unsupported { feature } => self.error(
                Some(statement.span),
                format!("unsupported statement: {feature}"),
            ),
        }
    }

    fn variable(
        &mut self,
        kind: VariableKind,
        name: &str,
        initializer: Option<&Expression>,
        span: Span,
    ) {
        self.variable_with(kind, name, initializer, span, false);
    }

    /// Bind `pattern` to the value on top of the stack, consuming it.
    fn bind_pattern(&mut self, kind: VariableKind, pattern: &Pattern, span: Span) {
        match pattern {
            Pattern::Identifier(name) => {
                if name.is_empty() {
                    // The frontend already reported why; binding an impossible name here would
                    // add a second, more confusing diagnostic.
                    self.code.push(Instruction::Pop);
                    return;
                }
                self.declare_bound(kind, name, span);
            }
            Pattern::WithDefault { inner, default } => {
                // A default applies only to `undefined`, not to every falsy value and not to
                // `null` -- which is why this is a nullish-style test against undefined alone
                // rather than a truthiness test.
                self.code.push(Instruction::Dup);
                self.push_constant(Constant::Undefined);
                self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
                let skip = self.emit_jump(Instruction::JumpIfFalse(0));
                self.code.push(Instruction::Pop);
                self.expression(default);
                self.patch_jump(skip);
                self.bind_pattern(kind, inner, span);
            }
            Pattern::Object { properties, rest } => {
                for property in properties {
                    self.code.push(Instruction::Dup);
                    let key = self.constant_index(Constant::String(property.key.clone()));
                    self.code.push(Instruction::GetProperty(key));
                    self.bind_pattern(kind, &property.value, span);
                }
                match rest {
                    // Copy everything, then remove what the named properties already took. The
                    // other direction -- copy all but a set of names -- would need the exclusion
                    // list at runtime; this needs only instructions that already earn their keep.
                    Some(name) => {
                        let source = format!("%rest{}", self.hidden_bindings);
                        self.hidden_bindings += 1;
                        self.scopes.push(HashMap::new());
                        self.declare_hidden(&source, span);

                        self.code.push(Instruction::NewObject { count: 0 });
                        self.load_hidden(&source);
                        self.code.push(Instruction::SpreadObject);
                        for property in properties {
                            let key = self.constant_index(Constant::String(property.key.clone()));
                            self.code.push(Instruction::Dup);
                            self.code.push(Instruction::DeleteProperty(key));
                            self.code.push(Instruction::Pop);
                        }
                        self.scopes.pop().expect("rest scope was just pushed");
                        self.declare_bound(kind, name, span);
                    }
                    None => self.code.push(Instruction::Pop),
                }
            }
            Pattern::Array { elements, rest } => {
                self.code.push(Instruction::IterableToArray);
                for (index, element) in elements.iter().enumerate() {
                    let Some(element) = element else {
                        continue; // a hole binds nothing
                    };
                    self.code.push(Instruction::Dup);
                    self.push_constant(Constant::Number((index as f64).to_bits()));
                    self.code.push(Instruction::GetComputed);
                    self.bind_pattern(kind, element, span);
                }
                if let Some(rest) = rest {
                    self.code.push(Instruction::Dup);
                    self.push_constant(Constant::Number((elements.len() as f64).to_bits()));
                    let slice = self.constant_index(Constant::String("slice".to_owned()));
                    self.code.push(Instruction::CallMethod {
                        name: slice,
                        argc: 1,
                    });
                    self.bind_pattern(kind, rest, span);
                }
                self.code.push(Instruction::Pop);
            }
        }
    }

    /// Declare a binding.
    ///
    /// `bound_from_stack` is for bindings the compiler introduces itself, where the value is
    /// already on the stack and the caller stores it: a `for-of` loop variable is `const` and
    /// initialised, but not by an expression this function can see. Without the distinction the
    /// const check would reject a binding that is in fact initialised.
    fn variable_with(
        &mut self,
        kind: VariableKind,
        name: &str,
        initializer: Option<&Expression>,
        span: Span,
        bound_from_stack: bool,
    ) {
        if name.is_empty() {
            self.error(Some(span), "destructuring bindings are not supported yet");
            return;
        }
        if kind != VariableKind::Var
            && initializer.is_some_and(|initializer| expression_references(initializer, name))
        {
            self.error(
                Some(span),
                format!("temporal-dead-zone access in initializer for `{name}` is not supported"),
            );
            return;
        }

        let scope_index = if kind == VariableKind::Var {
            0
        } else {
            self.scopes.len() - 1
        };
        if let Some(existing) = self.scopes[scope_index].get(name).copied() {
            // A `var` re-declaration is an assignment, and the hoisting pass has already created
            // the slot for every declaration in the block -- so reaching an existing binding is
            // the normal path now, and a *second* declaration of the same `let` is the error.
            // Two `var`s of one name are an assignment, not a redeclaration. Every other pairing
            // is an error -- including `var` after `let`, which is why the kinds of *both* are
            // consulted rather than only the one being declared now.
            let redeclaration = existing.initialized
                && !(kind == VariableKind::Var && existing.kind == VariableKind::Var);
            if redeclaration {
                self.error(
                    Some(span),
                    format!("redeclaration of local `{name}` is not supported"),
                );
                return;
            }
            if kind == VariableKind::Const && initializer.is_none() && !bound_from_stack {
                self.error(
                    Some(span),
                    format!("const local `{name}` requires an initializer"),
                );
            }
            self.initialize(name);
            if bound_from_stack {
                return;
            }
            if let Some(initializer) = initializer {
                self.expression(initializer);
            } else if kind != VariableKind::Var {
                // A `let` with no initializer is undefined; a `var` keeps what it already had,
                // which is what makes `var x = 1; var x;` leave 1 in place.
                self.push_constant(Constant::Undefined);
            } else {
                return;
            }
            self.code.push(if existing.celled {
                Instruction::StoreCell(existing.slot)
            } else {
                Instruction::StoreLocal(existing.slot)
            });
            return;
        }
        if kind == VariableKind::Const && initializer.is_none() && !bound_from_stack {
            self.error(
                Some(span),
                format!("const local `{name}` requires an initializer"),
            );
        }
        let slot = self.local_count;
        self.local_count += 1;
        // Boxed only when a nested function refers to this name and the name is not a module
        // *scope* binding. Module scope is already shared -- `LoadGlobal`/`StoreGlobal` reach the
        // one slot from every frame -- so boxing it would leave the cell itself in the slot and a
        // global read would hand back the cell rather than its contents. A block inside the
        // module body is not module scope, and a closure over it does need a cell.
        let in_module_scope = !self.compiling_function() && scope_index == 0;
        let celled = self.captured.contains(name) && !in_module_scope;
        self.scopes[scope_index].insert(
            name.to_owned(),
            Local {
                slot,
                kind,
                imported: false,
                initialized: true,
                celled,
            },
        );
        if celled {
            self.code.push(Instruction::NewCell(slot));
        }
        if bound_from_stack {
            // The value is already on the stack; the caller emits the store, because it may need
            // to know which instruction was used.
            return;
        }
        if let Some(initializer) = initializer {
            self.expression(initializer);
        } else {
            self.push_constant(Constant::Undefined);
        }
        self.code.push(if celled {
            Instruction::StoreCell(slot)
        } else {
            Instruction::StoreLocal(slot)
        });
    }

    /// Compile a run of statements sharing one scope, hoisting what the language hoists.
    ///
    /// `f(); function f() {}` is ordinary JavaScript, so a function declaration's value has to
    /// exist before the line that writes it. Emitting those first is not enough on its own: a
    /// hoisted body would then compile before the `const`s below it are in scope, and a closure
    /// legitimately sees those. So the block's *names* are all created first and only the
    /// function declarations' *values* are emitted early -- which is what the language does, and
    /// is where its temporal dead zone comes from.
    fn statements(&mut self, list: &[Statement]) {
        for statement in list {
            self.hoist_names(statement);
        }
        for statement in list {
            if let StatementKind::Function(function) = &statement.kind {
                self.emit_function_declaration(function, statement.span);
            }
        }
        for statement in list {
            // Emitted above; running it again would rebuild the closure and overwrite the
            // binding with an identical one.
            if matches!(statement.kind, StatementKind::Function(_)) {
                continue;
            }
            self.statement(statement);
        }
    }

    /// Create the bindings one statement declares, without running its initializer.
    fn hoist_names(&mut self, statement: &Statement) {
        match &statement.kind {
            // A function declaration is usable immediately; a class is not, which is the whole
            // difference between the two for hoisting purposes.
            StatementKind::Function(function) => {
                if let Some(name) = &function.name {
                    self.predeclare(VariableKind::Let, name, true);
                }
            }
            StatementKind::Class(class) => {
                if let Some(name) = &class.name {
                    self.predeclare(VariableKind::Let, name, false);
                }
            }
            StatementKind::Variable { kind, declarations } => {
                for declaration in declarations {
                    let mut names = Vec::new();
                    pattern_names_in_order(&declaration.pattern, &mut names);
                    for name in names {
                        // `var` is initialised to undefined when the scope is entered; `let` and
                        // `const` are not, and reading one before its declaration is an error.
                        self.predeclare(*kind, &name, *kind == VariableKind::Var);
                    }
                }
            }
            _ => {}
        }
    }

    /// Allocate a slot for a name the block declares, leaving its value for the declaration.
    ///
    /// Silent when the name is already there: a genuine redeclaration is reported by the
    /// declaration itself, where the span points at the second one rather than at the block.
    fn predeclare(&mut self, kind: VariableKind, name: &str, initialized: bool) {
        if name.is_empty() {
            return;
        }
        let scope_index = if kind == VariableKind::Var {
            0
        } else {
            self.scopes.len() - 1
        };
        if self.scopes[scope_index].contains_key(name) {
            return;
        }
        let slot = self.local_count;
        self.local_count += 1;
        let in_module_scope = !self.compiling_function() && scope_index == 0;
        let celled = self.captured.contains(name) && !in_module_scope;
        self.scopes[scope_index].insert(
            name.to_owned(),
            Local {
                slot,
                kind,
                imported: false,
                initialized,
                celled,
            },
        );
        // The cell has to exist before a hoisted function captures it, which is why this is here
        // rather than at the declaration.
        if celled {
            self.code.push(Instruction::NewCell(slot));
        }
        if initialized {
            // `var` reads as undefined before its declaration rather than throwing.
            self.push_constant(Constant::Undefined);
            self.code.push(if celled {
                Instruction::StoreCell(slot)
            } else {
                Instruction::StoreLocal(slot)
            });
        } else if celled || in_module_scope {
            // Only these can be read before the declaration without the compiler seeing it: a
            // captured binding through a cell or upvalue, and a module binding through a
            // function that was called too early. A plain local cannot, so it pays nothing.
            self.code.push(Instruction::MarkUninitialized(slot));
        }
    }

    /// Mark a name as past its dead zone, now that its declaration has run.
    fn initialize(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(local) = scope.get_mut(name) {
                local.initialized = true;
                return;
            }
        }
    }

    fn if_statement(
        &mut self,
        test: &Expression,
        consequent: &Statement,
        alternate: Option<&Statement>,
    ) {
        self.expression(test);
        let false_jump = self.emit_jump(Instruction::JumpIfFalse(0));
        self.statement(consequent);
        if let Some(alternate) = alternate {
            let end_jump = self.emit_jump(Instruction::Jump(0));
            self.patch_jump(false_jump);
            self.statement(alternate);
            self.patch_jump(end_jump);
        } else {
            self.patch_jump(false_jump);
        }
    }

    /// `try`/`catch`/`finally`.
    ///
    /// The cleanup block is emitted once per exit path rather than once with a dispatcher.
    /// A `finally` has to run on every way out -- falling off the end, `return`, `break`,
    /// `continue`, and an exception -- and duplicating the block is what lets each of those stay
    /// an ordinary jump. It costs code size in exchange for no pending-completion register and
    /// no second dispatch mechanism; if the size ever matters, the shape here is what a
    /// dispatcher would replace.
    fn try_statement(
        &mut self,
        block: &[Statement],
        handler: Option<&CatchClause>,
        finalizer: &[Statement],
        span: Span,
    ) {
        let has_finalizer = !finalizer.is_empty();
        if has_finalizer {
            self.finalizers.push(Cleanup::Finally(finalizer.to_vec()));
        }

        let handler_jump = self.emit_jump(Instruction::PushHandler(0));
        self.handler_depth += 1;

        self.scopes.push(HashMap::new());
        self.statements(block);
        self.scopes.pop().expect("try scope was just pushed");
        self.code.push(Instruction::PopHandler);
        self.handler_depth -= 1;

        // Normal completion runs the cleanup and skips the handler.
        if has_finalizer {
            self.finalizers.pop();
            self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
        }
        let done_jump = self.emit_jump(Instruction::Jump(0));

        // The handler is entered with the thrown value on the stack.
        // Entering the handler pops it at runtime, so the count is already back to what it was.
        self.patch_jump(handler_jump);
        if has_finalizer {
            self.finalizers.push(Cleanup::Finally(finalizer.to_vec()));
        }
        match handler {
            Some(clause) => {
                self.scopes.push(HashMap::new());
                match &clause.binding {
                    Some(name) => self.declare_bound(VariableKind::Let, name, span),
                    // `catch { }` discards the value rather than binding it.
                    None => self.code.push(Instruction::Pop),
                }
                self.statements(&clause.body);
                self.scopes.pop().expect("catch scope was just pushed");
                if has_finalizer {
                    self.finalizers.pop();
                    self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
                }
            }
            None => {
                // `try`/`finally` with no catch: run the cleanup, then let the exception
                // continue outwards. The value is still on the stack for `Throw` to take.
                if has_finalizer {
                    self.finalizers.pop();
                    self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
                }
                self.code.push(Instruction::Throw);
            }
        }

        self.patch_jump(done_jump);
    }

    /// Emit a cleanup block inline, with the enclosing cleanups temporarily out of scope.
    ///
    /// A `return` inside a `finally` must not re-run the same `finally`, which is what would
    /// happen if it were still on the stack while its own body compiled.
    fn emit_finalizer(&mut self, cleanup: &Cleanup) {
        match cleanup {
            Cleanup::Finally(statements) => {
                self.scopes.push(HashMap::new());
                self.statements(statements);
                self.scopes.pop().expect("finalizer scope was just pushed");
            }
            Cleanup::CloseIterator { binding } => self.emit_iterator_close(&binding.clone()),
        }
    }

    /// `if (it.return) it.return()` against a hidden `for-of` binding.
    ///
    /// The method is optional -- an array iterator has none -- so it is tested before it is
    /// called. The binding is loaded twice rather than duplicated on the stack because the test
    /// consumes what it reads, and a reload of a local is cheaper than the shuffling that
    /// keeping the receiver around would need.
    fn emit_iterator_close(&mut self, binding: &str) {
        let name = self.constant_index(Constant::String("return".to_owned()));
        self.load_hidden(binding);
        self.code.push(Instruction::GetProperty(name));
        let skip = self.emit_jump(Instruction::JumpIfFalse(0));
        self.load_hidden(binding);
        self.code.push(Instruction::CallMethod { name, argc: 0 });
        // The specification ignores the result unless it is not an object; nothing observes it
        // here, so it is discarded.
        self.code.push(Instruction::Pop);
        self.patch_jump(skip);
    }

    /// Run every cleanup between here and `depth`, innermost first.
    fn unwind_finalizers(&mut self, depth: usize) {
        while self.finalizers.len() > depth {
            let finalizer = self
                .finalizers
                .pop()
                .expect("checked by the loop condition");
            self.emit_finalizer(&finalizer);
            self.pending_finalizers.push(finalizer);
        }
        while let Some(finalizer) = self.pending_finalizers.pop() {
            self.finalizers.push(finalizer);
        }
    }

    /// `label: body`.
    ///
    /// A label on a loop names that loop, so it is handed to the loop's own context and nothing
    /// is emitted here. On anything else it names a block, which needs a context of its own for
    /// `break label` to have somewhere to land.
    fn labelled_statement(&mut self, label: &str, body: &Statement) {
        if matches!(
            body.kind,
            StatementKind::While { .. }
                | StatementKind::DoWhile { .. }
                | StatementKind::For { .. }
                | StatementKind::ForIn { .. }
                | StatementKind::ForOf { .. }
                | StatementKind::Switch { .. }
        ) {
            self.pending_label = Some(label.to_owned());
            self.statement(body);
            // Cleared in case the statement turned out not to claim it after all.
            self.pending_label = None;
            return;
        }

        self.loops.push(LoopContext {
            kind: BreakableKind::Labelled,
            label: Some(label.to_owned()),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this label pushed its own context");
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
    }

    /// `for (const name in object) body`.
    ///
    /// Walks a snapshot of the keys by index rather than through the iterator protocol, because
    /// there is no iterator involved: the keys are produced once and the loop steps through them.
    /// Each key is rechecked against the object before the body runs, so one the body deleted is
    /// skipped -- the reason this is not simply a `for-of` over `Object.keys(object)`.
    fn for_in(
        &mut self,
        kind: VariableKind,
        name: &str,
        object: &Expression,
        body: &Statement,
        span: Span,
    ) {
        let subject = format!("%in{}", self.hidden_bindings);
        let keys = format!("%keys{}", self.hidden_bindings);
        let cursor = format!("%at{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());
        self.expression(object);
        self.declare_hidden(&subject, span);
        self.load_hidden(&subject);
        self.code.push(Instruction::EnumerateKeys);
        self.declare_hidden(&keys, span);
        self.push_constant(Constant::Number(0f64.to_bits()));
        self.declare_hidden(&cursor, span);

        let start = self.code.len();
        self.load_hidden(&cursor);
        self.load_hidden(&keys);
        let length = self.constant_index(Constant::String("length".to_owned()));
        self.code.push(Instruction::GetProperty(length));
        self.code.push(Instruction::Binary(BinaryOp::LessThan));
        let exit_jump = self.emit_jump(Instruction::JumpIfFalse(0));

        // Read this step's key, then advance the cursor, so a `continue` in the body cannot
        // re-read it.
        let key = format!("%key{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.load_hidden(&keys);
        self.load_hidden(&cursor);
        self.code.push(Instruction::GetComputed);
        self.declare_hidden(&key, span);
        self.load_hidden(&cursor);
        self.push_constant(Constant::Number(1f64.to_bits()));
        self.code.push(Instruction::Binary(BinaryOp::Add));
        self.store_hidden(&cursor);

        // A key the body has already deleted is not visited.
        self.load_hidden(&key);
        self.load_hidden(&subject);
        self.code.push(Instruction::Binary(BinaryOp::In));
        let skip = self.emit_jump(Instruction::JumpIfFalse(0));

        // A fresh binding each iteration, so a closure made in the body captures that step's key.
        self.scopes.push(HashMap::new());
        self.load_hidden(&key);
        self.declare_bound(kind, name, span);

        self.loops.push(LoopContext {
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");
        self.scopes.pop().expect("iteration scope was just pushed");

        let continue_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, continue_target);
        }
        // A skipped key rejoins here, having already advanced the cursor.
        self.patch_jump(skip);
        self.code.push(Instruction::Jump(start as u32));

        self.patch_jump(exit_jump);
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
        self.scopes.pop().expect("loop scope was just pushed");
    }

    /// `for (const name of iterable) body`.
    ///
    /// Lowered to the protocol rather than to indexed access: the subject may be any iterable,
    /// and indexing an array directly would be wrong for everything else and would miss the
    /// re-read of `length` that mutation during iteration depends on.
    ///
    /// The iterator and each step's result live in hidden locals whose names carry a character an
    /// identifier cannot contain, so they can never collide with the program's own bindings.
    fn for_of(
        &mut self,
        kind: VariableKind,
        name: &str,
        iterable: &Expression,
        body: &Statement,
        span: Span,
    ) {
        let iterator_binding = format!("%iter{}", self.hidden_bindings);
        let result_binding = format!("%step{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());

        // `iterable[Symbol.iterator]()`, which is what makes any iterable work rather than only
        // an array.
        self.expression(iterable);
        let symbol = self.constant_index(Constant::String("@@iterator".to_owned()));
        self.code.push(Instruction::CallMethod {
            name: symbol,
            argc: 0,
        });
        self.declare_hidden(&iterator_binding, span);

        // One handler for the whole loop rather than one per iteration: the guarded region is
        // the same every time round, and pushing it once keeps `continue` an ordinary jump.
        let handler_jump = self.emit_jump(Instruction::PushHandler(0));

        let start = self.code.len();
        self.load_hidden(&iterator_binding);
        let next = self.constant_index(Constant::String("next".to_owned()));
        self.code.push(Instruction::CallMethod {
            name: next,
            argc: 0,
        });
        self.declare_hidden(&result_binding, span);

        self.load_hidden(&result_binding);
        let done = self.constant_index(Constant::String("done".to_owned()));
        self.code.push(Instruction::GetProperty(done));
        let exit_jump = self.emit_jump(Instruction::JumpIfTruthy(0));

        // A fresh binding each iteration, so a closure made in the body captures that step's
        // value rather than the last one.
        self.scopes.push(HashMap::new());
        self.load_hidden(&result_binding);
        let value = self.constant_index(Constant::String("value".to_owned()));
        self.code.push(Instruction::GetProperty(value));
        self.declare_bound(kind, name, span);

        // Leaving the loop early has to close the iterator. Registering that as a cleanup is
        // what gets `return` right for free; `break` and an exception are handled by the two
        // landing pads below, which is why the loop's own context is recorded *above* it.
        self.finalizers.push(Cleanup::CloseIterator {
            binding: iterator_binding.clone(),
        });
        self.handler_depth += 1;
        self.loops.push(LoopContext {
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");
        self.handler_depth -= 1;
        self.finalizers.pop().expect("the close was just pushed");
        self.scopes.pop().expect("iteration scope was just pushed");

        // `continue` stays inside the loop, so it neither closes the iterator nor pops the guard.
        let continue_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, continue_target);
        }
        self.code.push(Instruction::Jump(start as u32));

        // Running out of values is a normal completion: the iterator closes itself, and closing
        // it again here would call `return` on an exhausted iterator.
        self.patch_jump(exit_jump);
        self.code.push(Instruction::PopHandler);
        let normal_exit = self.emit_jump(Instruction::Jump(0));

        if !context.breaks.is_empty() {
            let break_pad = self.code.len() as u32;
            for jump in context.breaks {
                self.patch_jump_to(jump, break_pad);
            }
            self.code.push(Instruction::PopHandler);
            self.emit_iterator_close(&iterator_binding);
            let leave = self.emit_jump(Instruction::Jump(0));
            self.emit_catch_pad(handler_jump, &iterator_binding);
            self.patch_jump(leave);
        } else {
            self.emit_catch_pad(handler_jump, &iterator_binding);
        }
        self.patch_jump(normal_exit);
        self.scopes.pop().expect("loop scope was just pushed");
    }

    /// The landing pad an exception thrown inside a `for-of` body reaches: close the iterator,
    /// then let the exception carry on outwards. Entering it has already popped the handler.
    fn emit_catch_pad(&mut self, handler_jump: usize, binding: &str) {
        self.patch_jump(handler_jump);
        self.emit_iterator_close(binding);
        self.code.push(Instruction::Throw);
    }

    /// `yield* iterable`.
    ///
    /// Compiled as a loop over the iterator protocol rather than as one instruction. That is not
    /// an optimisation, it is the only way to get the semantics: each value has to be yielded
    /// *and suspended on*, and the value the caller sends must be handed to the delegate's own
    /// `next`. A single instruction cannot do that, because the run loop cannot suspend part-way
    /// through evaluating one -- an earlier attempt drained the iterable up front, which is
    /// wrong for a two-way protocol and never terminates on an infinite one.
    ///
    /// The shape is the specification's:
    ///
    /// ```text
    /// iterator = iterable[Symbol.iterator]()
    /// sent = undefined
    /// loop { step = iterator.next(sent); if step.done { break step.value } sent = yield step.value }
    /// ```
    fn yield_delegate(&mut self, value: Option<&Expression>, span: Span) {
        let iterator_binding = format!("%delegate{}", self.hidden_bindings);
        let sent_binding = format!("%sent{}", self.hidden_bindings);
        let step_binding = format!("%step{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());

        match value {
            Some(value) => self.expression(value),
            None => self.push_constant(Constant::Undefined),
        }
        let symbol = self.constant_index(Constant::String("@@iterator".to_owned()));
        self.code.push(Instruction::CallMethod {
            name: symbol,
            argc: 0,
        });
        self.declare_hidden(&iterator_binding, span);

        self.push_constant(Constant::Undefined);
        self.declare_hidden(&sent_binding, span);

        let start = self.code.len();
        self.load_hidden(&iterator_binding);
        self.load_hidden(&sent_binding);
        let next = self.constant_index(Constant::String("next".to_owned()));
        self.code.push(Instruction::CallMethod {
            name: next,
            argc: 1,
        });
        self.declare_hidden(&step_binding, span);

        self.load_hidden(&step_binding);
        let done = self.constant_index(Constant::String("done".to_owned()));
        self.code.push(Instruction::GetProperty(done));
        let finished = self.emit_jump(Instruction::JumpIfTruthy(0));

        // Not done: yield this value out, and keep what the caller sends back.
        self.load_hidden(&step_binding);
        let value_key = self.constant_index(Constant::String("value".to_owned()));
        self.code.push(Instruction::GetProperty(value_key));
        self.code.push(Instruction::Yield);
        self.store_hidden(&sent_binding);
        self.code.push(Instruction::Jump(start as u32));

        // Done: the delegate's own return value is what `yield*` evaluates to.
        self.patch_jump(finished);
        self.load_hidden(&step_binding);
        self.code.push(Instruction::GetProperty(value_key));

        self.scopes.pop().expect("delegate scope was just pushed");
    }

    fn store_hidden(&mut self, name: &str) {
        match self.resolve(name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::StoreCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::StoreLocal(local.slot)),
            _ => self.error(None, format!("`{name}` went out of scope")),
        }
    }

    /// Bind a hidden local to the value on top of the stack.
    ///
    /// Reuses the slot when the name is already bound in this scope, because a compiler-
    /// introduced binding inside a loop body is declared once and assigned on every pass.
    fn declare_hidden(&mut self, name: &str, span: Span) {
        if matches!(self.resolve(name), Resolution::Local(_)) {
            self.store_hidden(name);
            return;
        }
        self.declare_bound(VariableKind::Let, name, span);
    }

    /// Declare `name` and store the value on top of the stack into it.
    fn declare_bound(&mut self, kind: VariableKind, name: &str, span: Span) {
        self.variable_with(kind, name, None, span, true);
        match self.resolve(name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::StoreCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::StoreLocal(local.slot)),
            _ => self.error(Some(span), format!("could not bind `{name}`")),
        }
    }

    fn load_hidden(&mut self, name: &str) {
        match self.resolve(name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            _ => self.error(None, format!("`{name}` went out of scope")),
        }
    }

    /// `for (init; test; update) body`.
    ///
    /// The initialiser gets its own scope so a `let` there belongs to the loop rather than to
    /// the enclosing block, and `continue` jumps to the update rather than to the test -- which
    /// is the difference a `while` desugaring would silently lose.
    ///
    /// A `let` in the head is also re-bound on every pass, which is what makes
    /// `for (let i = 0; i < 3; i++) fs.push(() => i)` collect three closures over 0, 1 and 2
    /// rather than three over the value the loop ended on.
    fn for_statement(
        &mut self,
        init: Option<&Statement>,
        test: Option<&Expression>,
        update: Option<&Expression>,
        body: &Statement,
        span: Span,
    ) {
        let _ = span;
        self.scopes.push(HashMap::new());
        if let Some(init) = init {
            self.statement(init);
        }
        // Only a captured binding can tell one iteration's copy from another's, and only a
        // captured binding is boxed -- so the boxed `let`s of the head are exactly the set that
        // needs re-binding. Sorted by slot because the scope is a hash map and the bytecode is a
        // cache key: the same source must always produce the same instructions.
        let mut per_iteration: Vec<u32> = self
            .scopes
            .last()
            .expect("the loop scope was just pushed")
            .values()
            .filter(|local| local.celled && !matches!(local.kind, VariableKind::Var))
            .map(|local| local.slot)
            .collect();
        per_iteration.sort_unstable();

        let start = self.code.len();
        let exit_jump = test.map(|test| {
            self.expression(test);
            self.emit_jump(Instruction::JumpIfFalse(0))
        });

        self.loops.push(LoopContext {
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");

        // `continue` lands here rather than at the test, so the update still runs.
        let update_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, update_target);
        }
        // Re-bind before the update, not after: the update belongs to the *next* iteration, so
        // it must write the next iteration's cell and leave this one holding what the body saw.
        for slot in per_iteration {
            self.code.push(Instruction::LoadCell(slot));
            self.code.push(Instruction::NewCell(slot));
            self.code.push(Instruction::StoreCell(slot));
        }
        if let Some(update) = update {
            self.expression(update);
            self.code.push(Instruction::Pop);
        }
        self.code.push(Instruction::Jump(start as u32));

        if let Some(exit_jump) = exit_jump {
            self.patch_jump(exit_jump);
        }
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
        self.scopes.pop().expect("loop scope was just pushed");
    }

    /// `break` or `continue`, recorded for the enclosing loop to patch once its shape is known.
    fn jump_out(&mut self, label: Option<&str>, is_break: bool, span: Span) {
        // Only the cleanups entered since the target began: an outer `finally` is not left by
        // this jump, so running it here would run it twice.
        //
        // Unlabelled, `break` leaves the innermost breakable and `continue` steps over any
        // `switch` in the way, because a `switch` is not something a `continue` can continue.
        // Labelled, both look for that label instead -- which is the point of a label, and is
        // what lets one `break` leave several loops at once.
        let target = match label {
            None if is_break => self.loops.len().checked_sub(1),
            None => self
                .loops
                .iter()
                .rposition(|context| context.kind == BreakableKind::Loop),
            Some(label) => self.loops.iter().rposition(|context| {
                context.label.as_deref() == Some(label)
                    && (is_break || context.kind == BreakableKind::Loop)
            }),
        };
        if target.is_none()
            && let Some(label) = label
        {
            // The label is not there, or a `continue` named something that is not a loop.
            self.error(
                Some(span),
                format!("no enclosing loop or statement labelled `{label}`"),
            );
            return;
        }
        let (depth, handler_depth) = target.map_or((0, 0), |index| {
            let context = &self.loops[index];
            (context.finalizer_depth, context.handler_depth)
        });
        // Before the cleanups, not after: a `finally` reached by `break` must not still be
        // guarded by the handler of the `try` it belongs to, or an exception it raises would be
        // caught by its own `catch`.
        for _ in handler_depth..self.handler_depth {
            self.code.push(Instruction::PopHandler);
        }
        self.unwind_finalizers(depth);

        let jump = self.emit_jump(Instruction::Jump(0));
        let Some(context) = target.map(|index| &mut self.loops[index]) else {
            self.error(
                Some(span),
                if is_break {
                    "`break` outside of a loop"
                } else {
                    "`continue` outside of a loop"
                },
            );
            return;
        };
        if is_break {
            context.breaks.push(jump);
        } else {
            context.continues.push(jump);
        }
    }

    fn patch_jump_to(&mut self, jump: usize, target: u32) {
        match &mut self.code[jump] {
            Instruction::Jump(slot)
            | Instruction::JumpIfFalse(slot)
            | Instruction::JumpIfTruthy(slot)
            | Instruction::JumpIfNotNullish(slot)
            // A handler's target is patched the same way a jump's is: it is not known until the
            // handler body has been emitted.
            | Instruction::PushHandler(slot) => *slot = target,
            other => unreachable!("patched a {other:?}, which is not a jump"),
        }
    }

    fn while_statement(&mut self, test: &Expression, body: &Statement) {
        let loop_start = self.code.len();
        self.expression(test);
        let exit_jump = self.emit_jump(Instruction::JumpIfFalse(0));

        self.loops.push(LoopContext {
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");

        // `continue` re-tests, which for a `while` is the top of the loop.
        for jump in context.continues {
            self.patch_jump_to(jump, loop_start as u32);
        }
        self.code.push(Instruction::Jump(loop_start as u32));
        self.patch_jump(exit_jump);
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
    }

    /// `do body while (test)`, whose body always runs once before the test is reached.
    fn do_while_statement(&mut self, body: &Statement, test: &Expression) {
        let loop_start = self.code.len();

        self.loops.push(LoopContext {
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");

        // `continue` skips the rest of the body but still runs the test, so it lands here rather
        // than at the top -- the one place a do-while differs from a while for a jump.
        let test_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, test_target);
        }
        self.expression(test);
        let repeat = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.patch_jump_to(repeat, loop_start as u32);

        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
    }

    /// `switch (discriminant) { case … }`.
    ///
    /// Two passes over the arms, because a `switch` is not a set of alternatives: the tests are
    /// evaluated in order until one matches with `===`, and control then enters that arm's body
    /// and *keeps going* through the arms after it until a `break`. So the tests are emitted as a
    /// dispatch chain and the bodies as one straight run entered at different offsets.
    ///
    /// `default` is not special beyond being the arm with no test: it may sit anywhere among
    /// them, it is only reached once every test has failed, and it falls through like the rest.
    fn switch_statement(&mut self, discriminant: &Expression, cases: &[SwitchCase], span: Span) {
        // The discriminant is evaluated once, however many tests are compared against it.
        let subject = format!("%switch{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());
        self.expression(discriminant);
        self.declare_hidden(&subject, span);

        let mut entries = Vec::with_capacity(cases.len());
        let mut fallback = None;
        for case in cases {
            let Some(test) = &case.test else {
                // Emitted with the other bodies below; only its entry point is recorded here.
                fallback = Some(entries.len());
                entries.push(None);
                continue;
            };
            self.load_hidden(&subject);
            self.expression(test);
            self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
            entries.push(Some(self.emit_jump(Instruction::JumpIfTruthy(0))));
        }
        // No test matched, so control goes to `default` if there is one and past the whole
        // statement if there is not.
        let no_match = self.emit_jump(Instruction::Jump(0));

        self.loops.push(LoopContext {
            kind: BreakableKind::Switch,
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        // Every arm shares the switch's scope, so the hoisting pass spans all of them -- a
        // function declared in one case is callable from another, which is what one scope means.
        for case in cases {
            for statement in &case.body {
                self.hoist_names(statement);
            }
        }
        let mut starts = Vec::with_capacity(cases.len());
        for case in cases {
            starts.push(self.code.len() as u32);
            for statement in &case.body {
                self.statement(statement);
            }
        }
        let context = self
            .loops
            .pop()
            .expect("this switch pushed its own context");

        for (entry, start) in entries.iter().zip(&starts) {
            if let Some(jump) = entry {
                self.patch_jump_to(*jump, *start);
            }
        }
        match fallback.and_then(|index| starts.get(index)) {
            Some(start) => self.patch_jump_to(no_match, *start),
            None => self.patch_jump(no_match),
        }

        let exit = self.code.len() as u32;
        // A `default` in the middle still falls through, so this is the only jump that leaves
        // early -- and it is the same jump `break` uses.
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
        self.scopes.pop().expect("switch scope was just pushed");
    }

    fn expression(&mut self, expression: &Expression) {
        match &expression.kind {
            ExpressionKind::Literal(literal) => self.push_literal(literal),
            ExpressionKind::Identifier(name) => match self.resolve_or_capture(name) {
                // Reading a binding whose declaration has not run yet is the temporal dead zone.
                // Refused rather than read as undefined, which is what pre-declaring every name
                // in a block would otherwise turn a loud mistake into. A nested function reading
                // the same name resolves as an upvalue and never reaches this arm, which is
                // correct: it runs later, by which time the declaration has.
                Resolution::Local(local) if !local.initialized => self.error(
                    Some(expression.span),
                    format!("cannot access `{name}` before its declaration"),
                ),
                Resolution::Local(local) if local.celled => {
                    self.code.push(Instruction::LoadCell(local.slot))
                }
                Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
                Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
                Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
                Resolution::Unresolved if name == "this" => self.code.push(Instruction::LoadThis),
                // `arguments` is not a declared binding, so it only resolves when nothing else
                // bound the name -- a parameter or local called `arguments` shadows it, as it
                // does in JavaScript.
                Resolution::Unresolved if name == "arguments" && self.compiling_function() => {
                    self.code.push(Instruction::LoadArguments)
                }
                Resolution::Unresolved if name == "undefined" => {
                    self.push_constant(Constant::Undefined)
                }
                // A built-in is only reachable when nothing else bound the name, so a program
                // that declares its own `Error` shadows it exactly as it would any other global.
                Resolution::Unresolved => match Builtin::from_name(name) {
                    Some(builtin) => self.code.push(Instruction::LoadBuiltin(builtin)),
                    None => self.error(
                        Some(expression.span),
                        format!("unresolved identifier `{name}`"),
                    ),
                },
            },
            ExpressionKind::Unary { operator, argument } => {
                self.expression(argument);
                self.code.push(Instruction::Unary(match operator {
                    UnaryOperator::Plus => UnaryOp::Plus,
                    UnaryOperator::Negate => UnaryOp::Negate,
                    UnaryOperator::Not => UnaryOp::Not,
                    UnaryOperator::TypeOf => UnaryOp::TypeOf,
                    UnaryOperator::BitwiseNot => UnaryOp::BitwiseNot,
                    UnaryOperator::Void => UnaryOp::Void,
                }));
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                self.expression(left);
                self.expression(right);
                self.code.push(Instruction::Binary(match operator {
                    BinaryOperator::Add => BinaryOp::Add,
                    BinaryOperator::Subtract => BinaryOp::Subtract,
                    BinaryOperator::Multiply => BinaryOp::Multiply,
                    BinaryOperator::Divide => BinaryOp::Divide,
                    BinaryOperator::Remainder => BinaryOp::Remainder,
                    BinaryOperator::Exponentiate => BinaryOp::Exponentiate,
                    BinaryOperator::StrictEqual => BinaryOp::StrictEqual,
                    BinaryOperator::In => BinaryOp::In,
                    BinaryOperator::InstanceOf => BinaryOp::InstanceOf,
                    BinaryOperator::StrictNotEqual => BinaryOp::StrictNotEqual,
                    BinaryOperator::LooseEqual => BinaryOp::LooseEqual,
                    BinaryOperator::LooseNotEqual => BinaryOp::LooseNotEqual,
                    BinaryOperator::BitwiseAnd => BinaryOp::BitwiseAnd,
                    BinaryOperator::BitwiseOr => BinaryOp::BitwiseOr,
                    BinaryOperator::BitwiseXor => BinaryOp::BitwiseXor,
                    BinaryOperator::ShiftLeft => BinaryOp::ShiftLeft,
                    BinaryOperator::ShiftRight => BinaryOp::ShiftRight,
                    BinaryOperator::UnsignedShiftRight => BinaryOp::UnsignedShiftRight,
                    BinaryOperator::LessThan => BinaryOp::LessThan,
                    BinaryOperator::LessThanOrEqual => BinaryOp::LessThanOrEqual,
                    BinaryOperator::GreaterThan => BinaryOp::GreaterThan,
                    BinaryOperator::GreaterThanOrEqual => BinaryOp::GreaterThanOrEqual,
                }));
            }
            ExpressionKind::Logical {
                operator,
                left,
                right,
            } => self.logical(*operator, left, right),
            ExpressionKind::Assignment {
                operator,
                target,
                value,
            } => self.assignment(*operator, target, value, expression.span),
            ExpressionKind::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.expression(test);
                let false_jump = self.emit_jump(Instruction::JumpIfFalse(0));
                self.expression(consequent);
                let end_jump = self.emit_jump(Instruction::Jump(0));
                self.patch_jump(false_jump);
                self.expression(alternate);
                self.patch_jump(end_jump);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.call(callee, arguments, expression.span)
            }
            ExpressionKind::This => self.code.push(Instruction::LoadThis),
            ExpressionKind::Yield { delegate, value } if !*delegate => {
                match value {
                    Some(value) => self.expression(value),
                    None => self.push_constant(Constant::Undefined),
                }
                self.code.push(Instruction::Yield);
            }
            ExpressionKind::Yield { value, .. } => {
                self.yield_delegate(value.as_deref(), expression.span)
            }
            ExpressionKind::Class(class) => self.class_expression(class, expression.span),
            ExpressionKind::Update {
                increment,
                prefix,
                target,
            } => self.update(*increment, *prefix, target, expression.span),
            ExpressionKind::SuperCall { arguments } => {
                if arguments.len() > u16::MAX as usize {
                    self.error(Some(expression.span), "too many arguments to `super`");
                    return;
                }
                self.load_super();
                for argument in arguments {
                    self.expression(argument);
                }
                // The parent's return value is this expression's value, so `SuperCall` leaves
                // exactly one thing on the stack and needs no epilogue here.
                self.code.push(Instruction::SuperCall {
                    argc: arguments.len() as u16,
                });
            }
            ExpressionKind::SuperMember { property } => {
                // `super.m` resolves on the parent's prototype, not on `this`.
                self.load_super();
                let prototype = self.constant_index(Constant::String("prototype".to_owned()));
                self.code.push(Instruction::GetProperty(prototype));
                let name = self.constant_index(Constant::String(property.clone()));
                self.code.push(Instruction::GetProperty(name));
            }
            ExpressionKind::Function(function) => {
                if let Some(index) = self.compile_function(function, expression.span) {
                    self.code.push(Instruction::MakeFunction(index));
                }
            }
            ExpressionKind::Array { elements } => self.array_literal(elements, expression.span),
            ExpressionKind::Sequence { operands } => {
                let Some((last, rest)) = operands.split_last() else {
                    self.push_constant(Constant::Undefined);
                    return;
                };
                // Every operand is evaluated for its effects; only the last one is the value.
                for operand in rest {
                    self.expression(operand);
                    self.code.push(Instruction::Pop);
                }
                self.expression(last);
            }
            // `await` is a `yield` whose resumption the async driver supplies rather than the
            // program: the same suspension, resumed by a promise settling instead of a `next`.
            ExpressionKind::Await(value) => {
                if !self.compiling_function() {
                    // Top-level `await` needs the module itself to be resumable, which is a
                    // different shape from a function that suspends.
                    self.error(
                        Some(expression.span),
                        "top-level `await` is not supported yet",
                    );
                    self.push_constant(Constant::Undefined);
                    return;
                }
                self.expression(value);
                self.code.push(Instruction::Yield);
            }
            ExpressionKind::Chain { expression: inner } => {
                self.chains.push(Vec::new());
                self.expression(inner);
                let exits = self.chains.pop().expect("this chain pushed its own frame");
                // Every `?.` in the chain lands here with `undefined` already on the stack, so
                // the chain's value is one thing either way.
                for jump in exits {
                    self.patch_jump(jump);
                }
            }
            ExpressionKind::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, expression.span);
                self.expression(property);
                self.code.push(Instruction::GetComputed);
            }
            ExpressionKind::ComputedAssignment {
                operator,
                object,
                property,
                value,
            } => self.computed_assignment(*operator, object, property, value),
            ExpressionKind::New { callee, arguments } => {
                if arguments.len() > u16::MAX as usize {
                    self.error(Some(expression.span), "too many constructor arguments");
                    return;
                }
                if Self::has_spread(arguments) {
                    self.expression(callee);
                    self.array_literal(arguments, expression.span);
                    self.code.push(Instruction::ConstructSpread);
                    return;
                }
                self.expression(callee);
                for argument in arguments {
                    let ListElement::Item(value) = argument else {
                        unreachable!("just checked that there are no spreads");
                    };
                    self.expression(value);
                }
                self.code.push(Instruction::New {
                    argc: arguments.len() as u16,
                });
            }
            ExpressionKind::MemberAssignment {
                operator,
                object,
                property,
                value,
            } => self.member_assignment(*operator, object, property, value),
            ExpressionKind::PrivateAssignment {
                operator,
                object,
                name,
                value,
            } => self.private_assignment(*operator, object, name, value),
            ExpressionKind::Object { members } => self.object_literal(members, expression.span),
            // Concatenation, starting from a string so that every `+` after it is a string
            // append rather than a numeric add. `${1}${2}` is "12" for exactly that reason.
            ExpressionKind::Template {
                quasis,
                expressions,
            } => {
                let mut quasis = quasis.iter();
                self.push_constant(Constant::String(quasis.next().cloned().unwrap_or_default()));
                for (expression, quasi) in expressions.iter().zip(quasis) {
                    self.expression(expression);
                    self.code.push(Instruction::Binary(BinaryOp::Add));
                    // A run of text between two holes is skipped when it is empty, which is the
                    // common shape: `${a}${b}` has two empty quasis in the middle and the end.
                    if !quasi.is_empty() {
                        self.push_constant(Constant::String(quasi.clone()));
                        self.code.push(Instruction::Binary(BinaryOp::Add));
                    }
                }
            }
            ExpressionKind::RegExp { pattern, flags } => {
                let pattern = self.constant_index(Constant::String(pattern.clone()));
                let flags = self.constant_index(Constant::String(flags.clone()));
                self.code.push(Instruction::NewRegExp { pattern, flags });
            }
            ExpressionKind::PrivateMember { object, name } => {
                self.expression(object);
                let name = self.constant_index(Constant::String(private_key(name)));
                self.code.push(Instruction::GetPrivate(name));
            }
            ExpressionKind::Delete { object, property } => {
                self.expression(object);
                match property.as_ref() {
                    // `delete obj.#x` is a syntax error, so the frontend never builds one.
                    ObjectKey::Private(name) => self.error(
                        Some(expression.span),
                        format!("`delete` of the private name `#{name}`"),
                    ),
                    ObjectKey::Static(key) => {
                        let name = self.constant_index(Constant::String(key.clone()));
                        self.code.push(Instruction::DeleteProperty(name));
                    }
                    ObjectKey::Computed(key) => {
                        self.expression(key);
                        self.code.push(Instruction::DeleteComputed);
                    }
                }
            }
            ExpressionKind::StaticMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, expression.span);
                let name = self.constant_index(Constant::String(property.clone()));
                self.code.push(Instruction::GetProperty(name));
            }
            ExpressionKind::Unsupported { feature } => {
                self.error(
                    Some(expression.span),
                    format!("unsupported expression: {feature}"),
                );
                self.push_constant(Constant::Undefined);
            }
        }
    }

    /// Leave the enclosing optional chain with `undefined` when the value on top of the stack is
    /// nullish, or fall through with it untouched.
    ///
    /// Does nothing when the access is not optional, so every member read can call it and only
    /// the ones written with `?.` cost anything.
    fn short_circuit_if_nullish(&mut self, optional: bool, span: Span) {
        if !optional {
            return;
        }
        if self.chains.is_empty() {
            // The frontend only produces an optional access inside a chain, so this means the
            // two have drifted apart rather than that a program did something unusual.
            self.error(Some(span), "`?.` outside of an optional chain");
            return;
        }
        self.code.push(Instruction::Dup);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        // Nullish: drop the object and hand the whole chain `undefined`.
        self.code.push(Instruction::Pop);
        self.push_constant(Constant::Undefined);
        let exit = self.emit_jump(Instruction::Jump(0));
        self.chains
            .last_mut()
            .expect("checked just above")
            .push(exit);
        self.patch_jump(present);
    }

    /// An object literal.
    ///
    /// With no spread this is one `NewObject` over the whole literal, which is the common case
    /// and the cheap one. A spread splits the literal into segments: each run of properties
    /// becomes its own object and is merged into the accumulator in source order, so a property
    /// written after a spread overwrites what the spread contributed and one written before it
    /// is overwritten. Merging a segment uses the same instruction the spread does, rather than
    /// a second way to set a property on an object already on the stack.
    fn object_literal(&mut self, members: &[ObjectMember], span: Span) {
        if members.len() > u32::MAX as usize {
            self.error(Some(span), "object literal has too many properties");
            return;
        }
        let mut pending = 0u32;
        let mut accumulating = false;

        for member in members {
            match member {
                ObjectMember::Property(property) => {
                    match &property.key {
                        // Only a class body can declare one, so a literal never has one.
                        ObjectKey::Private(name) => self.error(
                            Some(span),
                            format!("private name `#{name}` in an object literal"),
                        ),
                        ObjectKey::Static(key) => {
                            self.push_constant(Constant::String(key.clone()));
                        }
                        // Evaluated in source order, between the members around it, because a
                        // computed key is an arbitrary expression and may have side effects.
                        ObjectKey::Computed(key) => self.expression(key),
                    }
                    self.expression(&property.value);
                    pending += 1;
                }
                ObjectMember::Spread(source) => {
                    // The first segment becomes the accumulator; later ones merge into it.
                    self.emit_object_segment(&mut pending, accumulating);
                    accumulating = true;
                    self.expression(source);
                    self.code.push(Instruction::SpreadObject);
                }
                ObjectMember::Accessor {
                    key,
                    kind,
                    function,
                } => {
                    // An accessor is defined on the object rather than pushed as a pair, so the
                    // pending run has to become an object first.
                    self.emit_object_segment(&mut pending, accumulating);
                    accumulating = true;
                    match key {
                        ObjectKey::Private(name) => self.error(
                            Some(span),
                            format!("private name `#{name}` in an object literal"),
                        ),
                        ObjectKey::Static(name) => {
                            self.push_constant(Constant::String(name.clone()));
                        }
                        ObjectKey::Computed(key) => self.expression(key),
                    }
                    let Some(index) = self.compile_function(function, span) else {
                        return;
                    };
                    self.code.push(Instruction::MakeFunction(index));
                    self.code.push(Instruction::DefineAccessor {
                        setter: matches!(kind, kiln_hir::MethodKind::Set),
                    });
                }
            }
        }
        self.emit_object_segment(&mut pending, accumulating);
    }

    /// Emit the pending run of key/value pairs as an object, merging it into the accumulator
    /// beneath when one exists.
    ///
    /// Skipped when the run is empty and something is already accumulating, so `{ ...a, ...b }`
    /// does not build two throwaway objects.
    fn emit_object_segment(&mut self, pending: &mut u32, merge: bool) {
        if merge && *pending == 0 {
            return;
        }
        self.code.push(Instruction::NewObject { count: *pending });
        *pending = 0;
        if merge {
            self.code.push(Instruction::SpreadObject);
        }
    }

    fn logical(&mut self, operator: LogicalOperator, left: &Expression, right: &Expression) {
        self.expression(left);
        self.code.push(Instruction::Dup);
        let jump = self.emit_jump(match operator {
            LogicalOperator::And => Instruction::JumpIfFalse(0),
            LogicalOperator::Or => Instruction::JumpIfTruthy(0),
            LogicalOperator::NullishCoalesce => Instruction::JumpIfNotNullish(0),
        });
        self.code.push(Instruction::Pop);
        self.expression(right);
        self.patch_jump(jump);
    }

    fn assignment(
        &mut self,
        operator: AssignmentOperator,
        target: &str,
        value: &Expression,
        span: Span,
    ) {
        // Whether the target is a frame local or a module slot decides the store instruction,
        // so it has to be carried alongside the slot rather than assumed.
        enum Place {
            Local(u32),
            Cell(u32),
            Upvalue(u32),
            Global(u32),
        }
        let (place, kind, imported) = match self.resolve_or_capture(target) {
            Resolution::Local(local) if local.celled => {
                (Place::Cell(local.slot), local.kind, local.imported)
            }
            Resolution::Local(local) => (Place::Local(local.slot), local.kind, local.imported),
            Resolution::Upvalue(index) => {
                // The owning declaration decides const-ness, and it was checked where the name
                // was captured; an upvalue carries no separate mutability of its own.
                (Place::Upvalue(index), VariableKind::Let, false)
            }
            Resolution::Global(slot) => {
                let module = &self.enclosing[0].scopes[0];
                let local = module
                    .get(target)
                    .copied()
                    .expect("Global resolution just found this name in module scope");
                (Place::Global(slot), local.kind, local.imported)
            }
            Resolution::Unresolved => {
                self.error(
                    Some(span),
                    format!("unresolved assignment target `{target}`"),
                );
                self.push_constant(Constant::Undefined);
                return;
            }
        };
        let (load, store) = match place {
            Place::Local(slot) => (Instruction::LoadLocal(slot), Instruction::StoreLocal(slot)),
            Place::Cell(slot) => (Instruction::LoadCell(slot), Instruction::StoreCell(slot)),
            Place::Upvalue(index) => (
                Instruction::LoadUpvalue(index),
                Instruction::StoreUpvalue(index),
            ),
            Place::Global(slot) => (
                Instruction::LoadGlobal(slot),
                Instruction::StoreGlobal(slot),
            ),
        };
        if imported {
            self.error(
                Some(span),
                format!("assignment to imported binding `{target}` is not allowed"),
            );
        } else if kind == VariableKind::Const {
            self.error(
                Some(span),
                format!("assignment to const local `{target}` is not supported"),
            );
        }
        if operator == AssignmentOperator::Assign {
            self.expression(value);
        } else {
            self.code.push(load);
            self.expression(value);
            let binary = match operator {
                AssignmentOperator::Add => BinaryOp::Add,
                AssignmentOperator::Subtract => BinaryOp::Subtract,
                AssignmentOperator::Multiply => BinaryOp::Multiply,
                AssignmentOperator::Divide => BinaryOp::Divide,
                AssignmentOperator::Remainder => BinaryOp::Remainder,
                AssignmentOperator::Exponentiate => BinaryOp::Exponentiate,
                AssignmentOperator::Assign => unreachable!(),
            };
            self.code.push(Instruction::Binary(binary));
        }
        self.code.push(Instruction::Dup);
        self.code.push(store);
    }

    /// Compile a class into a constructor and a prototype.
    ///
    /// The desugaring is not a shortcut -- a class really is a constructor function whose
    /// `prototype` holds the instance methods -- so it reuses the machinery `new` already needs
    /// rather than introducing a parallel object model for classes.
    /// A class expression leaves its constructor on the stack.
    fn class_expression(&mut self, class: &Class, span: Span) {
        // A class expression may be anonymous, and its name -- when it has one -- is visible only
        // inside its own body. Compiling it through a hidden binding gives methods a way to refer
        // to it without leaking the name into the enclosing scope.
        let binding = format!("%class{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());
        let mut named = class.clone();
        named.name = Some(binding.clone());
        self.class_declaration(&named);
        match self.resolve(&binding) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            _ => {
                self.error(Some(span), "could not bind class expression");
                self.push_constant(Constant::Undefined);
            }
        }
        self.scopes.pop().expect("class scope was just pushed");
    }

    fn class_declaration(&mut self, class: &Class) {
        let Some(name) = class.name.clone() else {
            self.error(Some(class.span), "a class declaration requires a name");
            return;
        };
        // A derived class evaluates its parent once, into a binding its members capture.
        //
        // The name carries a character an identifier cannot contain, so it can never collide
        // with one the program wrote, and a counter so that two derived classes in one scope get
        // separate bindings. `super` is syntax rather than a variable, but it resolves exactly
        // like a captured one -- giving it a real binding lets it reuse the closure machinery
        // instead of needing a parallel path through every scope.
        if let Some(superclass) = &class.superclass {
            let binding = format!("%super{}", self.super_binding_count);
            self.super_binding_count += 1;
            self.captured.insert(binding.clone());
            self.variable(VariableKind::Const, &binding, Some(superclass), class.span);
            self.super_bindings.push(binding);
        }

        // Bound before the body is compiled, so a method can refer to the class by name.
        self.variable(VariableKind::Let, &name, None, class.span);

        let constructor = match &class.constructor {
            Some(constructor) => constructor.clone(),
            // A class with no constructor gets an empty one, so `new` needs no special case for
            // classes that declared none.
            None => Function {
                rest: None,
                asynchronous: false,
                name: Some(name.clone()),
                arrow: false,
                generator: false,
                parameters: Vec::new(),
                body: Vec::new(),
                span: class.span,
            },
        };
        let synthesized_derived = class.constructor.is_none() && class.superclass.is_some();
        // A private instance method is installed on each *instance*, not on the prototype: that
        // is what carries the brand, and what makes `D.f()` on a sibling class a TypeError rather
        // than a call. Modelled as a field whose value is the function, so the constructor
        // prologue that already installs fields installs these too.
        let private_methods: Vec<ClassField> = class
            .methods
            .iter()
            .filter(|method| {
                !method.is_static
                    && matches!(method.key, ObjectKey::Private(_))
                    && matches!(method.kind, kiln_hir::MethodKind::Normal)
            })
            .map(|method| ClassField {
                key: method.key.clone(),
                value: Some(Expression {
                    kind: ExpressionKind::Function(Box::new(method.function.clone())),
                    span: method.function.span,
                }),
                is_static: false,
            })
            .collect();
        let instance_fields: Vec<&ClassField> = class
            .fields
            .iter()
            .filter(|field| !field.is_static)
            .chain(private_methods.iter())
            .collect();
        let Some(index) = self.compile_constructor(
            &constructor,
            class.span,
            // A *derived* class with no constructor is not empty: it forwards its arguments to
            // the parent. Without that, `class F extends Error {}` would construct an error with
            // no message.
            synthesized_derived,
            &instance_fields,
        ) else {
            return;
        };

        let Resolution::Local(local) = self.resolve(&name) else {
            self.error(Some(class.span), format!("could not bind class `{name}`"));
            return;
        };
        self.code.push(Instruction::MakeFunction(index));
        self.code.push(if local.celled {
            Instruction::StoreCell(local.slot)
        } else {
            Instruction::StoreLocal(local.slot)
        });

        let prototype = self.constant_index(Constant::String("prototype".to_owned()));

        // Link the two prototypes so instances inherit the parent's methods, and the two
        // constructors so static methods are inherited too.
        if class.superclass.is_some() {
            self.load_local_or_cell(local);
            self.code.push(Instruction::GetProperty(prototype));
            self.load_super();
            self.code.push(Instruction::GetProperty(prototype));
            self.code.push(Instruction::SetPrototype);
            self.code.push(Instruction::Pop);

            self.load_local_or_cell(local);
            self.load_super();
            self.code.push(Instruction::SetPrototype);
            self.code.push(Instruction::Pop);
        }

        // Static fields are ordinary properties of the constructor, assigned once here.
        for field in class.fields.iter().filter(|field| field.is_static) {
            self.load_local_or_cell(local);
            match &field.key {
                ObjectKey::Private(name) => {
                    let name = self.constant_index(Constant::String(private_key(name)));
                    self.emit_field_value(field);
                    self.code.push(Instruction::SetProperty(name));
                }
                ObjectKey::Static(key) => {
                    let name = self.constant_index(Constant::String(key.clone()));
                    self.emit_field_value(field);
                    self.code.push(Instruction::SetProperty(name));
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.emit_field_value(field);
                    self.code.push(Instruction::SetComputed);
                }
            }
            self.code.push(Instruction::Pop);
        }

        for method in &class.methods {
            let Some(body) = self.compile_function(&method.function, class.span) else {
                return;
            };

            // Static methods hang off the constructor; instance methods off its prototype.
            self.load_local_or_cell(local);
            if !method.is_static {
                self.code.push(Instruction::GetProperty(prototype));
            }

            let accessor = !matches!(method.kind, kiln_hir::MethodKind::Normal);
            match &method.key {
                // Installed on each instance by the constructor prologue instead.
                ObjectKey::Private(_) if !method.is_static && !accessor => {
                    self.code.push(Instruction::Pop);
                    continue;
                }
                ObjectKey::Private(name) => {
                    let name = self.constant_index(Constant::String(private_key(name)));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::SetProperty(name));
                }
                ObjectKey::Static(key) if !accessor => {
                    let name = self.constant_index(Constant::String(key.clone()));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::SetProperty(name));
                }
                ObjectKey::Static(key) => {
                    self.push_constant(Constant::String(key.clone()));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::DefineAccessor {
                        setter: matches!(method.kind, kiln_hir::MethodKind::Set),
                    });
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.code.push(Instruction::MakeFunction(body));
                    if accessor {
                        self.code.push(Instruction::DefineAccessor {
                            setter: matches!(method.kind, kiln_hir::MethodKind::Set),
                        });
                    } else {
                        self.code.push(Instruction::SetComputed);
                    }
                }
            }
            // Each definition leaves something; nothing consumes it.
            self.code.push(Instruction::Pop);
        }

        if class.superclass.is_some() {
            self.super_bindings.pop();
        }
    }

    /// Push the parent constructor of the class currently being compiled.
    fn load_super(&mut self) {
        let Some(binding) = self.super_bindings.last().cloned() else {
            self.error(None, "`super` outside of a derived class");
            self.push_constant(Constant::Undefined);
            return;
        };
        match self.resolve_or_capture(&binding) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            Resolution::Unresolved => {
                self.error(None, "`super` outside of a derived class");
                self.push_constant(Constant::Undefined);
            }
        }
    }

    /// A field with no initialiser is `undefined`, not absent: the property exists.
    fn emit_field_value(&mut self, field: &ClassField) {
        match &field.value {
            Some(value) => self.expression(value),
            None => self.push_constant(Constant::Undefined),
        }
    }

    fn load_local_or_cell(&mut self, local: Local) {
        self.code.push(if local.celled {
            Instruction::LoadCell(local.slot)
        } else {
            Instruction::LoadLocal(local.slot)
        });
    }

    /// Compile a function body into its own proto and bind the name to a callable.
    ///
    /// Declarations are not hoisted yet: the binding exists from this point on, so calling a
    /// function textually above its declaration is an unresolved identifier rather than a call
    /// to `undefined`. That is a smaller lie than hoisting half-way would be, and it is a
    /// diagnostic rather than a wrong answer.
    /// Compile a function declaration and store it in its already-created binding.
    ///
    /// The binding exists before the body is compiled, which is what makes a function able to
    /// call itself and to be called from above its own text: the body resolves its own name
    /// through the enclosing scope, and the slot it reads is filled by the `MakeFunction` here
    /// before any call can reach it.
    fn emit_function_declaration(&mut self, function: &Function, span: Span) {
        let Some(name) = function.name.clone() else {
            self.error(Some(span), "a function declaration requires a name");
            return;
        };
        // Normally created by the hoisting pass. A function declaration can also appear where a
        // single statement is expected -- `if (x) function f() {}` -- and there is no block to
        // have hoisted it.
        if !matches!(self.resolve(&name), Resolution::Local(_)) {
            self.predeclare(VariableKind::Let, &name, true);
        }

        let Some(index) = self.compile_function(function, span) else {
            return;
        };

        match self.resolve(&name) {
            Resolution::Local(local) => {
                self.code.push(Instruction::MakeFunction(index));
                self.code.push(if local.celled {
                    Instruction::StoreCell(local.slot)
                } else {
                    Instruction::StoreLocal(local.slot)
                });
            }
            // The binding was just created in the current function's innermost scope, so any
            // other resolution means the scope bookkeeping is inconsistent.
            _ => self.error(Some(span), format!("could not bind function `{name}`")),
        }
    }

    /// Compile one function body, returning its index in [`Program::functions`].
    ///
    /// The body is compiled against a fresh state: its own code buffer, its own slot numbering
    /// starting at the parameters, and its own scope stack. Module scope stays reachable through
    /// `enclosing[0]`; anything between is a capture and is refused by `resolve`.
    fn compile_function(&mut self, function: &Function, span: Span) -> Option<u32> {
        self.compile_function_with(function, span, false)
    }

    /// As [`Compiler::compile_function`], optionally prefixing a `super(...arguments)` forward.
    fn compile_function_with(
        &mut self,
        function: &Function,
        span: Span,
        forwards_to_super: bool,
    ) -> Option<u32> {
        self.compile_constructor(function, span, forwards_to_super, &[])
    }

    /// Compile a constructor, whose prologue initialises the class's instance fields.
    ///
    /// Fields run after `super()` and before the body, which is the order the language specifies
    /// and the only one that works: a field initialiser may read `this`, and in a derived class
    /// `this` does not exist until the parent has run.
    fn compile_constructor(
        &mut self,
        function: &Function,
        span: Span,
        forwards_to_super: bool,
        fields: &[&ClassField],
    ) -> Option<u32> {
        if function.parameters.len() > u32::MAX as usize {
            self.error(Some(span), "too many parameters");
            return None;
        }
        if self.functions.len() >= u32::MAX as usize {
            self.error(Some(span), "too many functions in one module");
            return None;
        }

        // Which of this function's own bindings its nested functions refer to. Computed before
        // the body is compiled because a declaration has to know, at the point it allocates its
        // slot, whether that slot holds a value or a cell.
        let captured = names_captured_by_this_function(function);

        // Every parameter owns a positional slot, whether or not it binds a name there. A plain
        // identifier binds the slot itself, which keeps the common call free of any prologue; a
        // pattern leaves the slot anonymous and is destructured out of it below, into ordinary
        // locals that behave like any other binding.
        let mut parameters = HashMap::with_capacity(function.parameters.len());
        let mut celled_parameters = Vec::new();
        let mut destructured = Vec::new();
        for (slot, parameter) in function.parameters.iter().enumerate() {
            let slot = slot as u32;
            let Pattern::Identifier(name) = &parameter.pattern else {
                destructured.push((slot, &parameter.pattern, parameter.span));
                continue;
            };
            if parameters.contains_key(name) {
                self.error(
                    Some(parameter.span),
                    format!("duplicate parameter `{name}`"),
                );
                return None;
            }
            let celled = captured.contains(name);
            if celled {
                celled_parameters.push(slot);
            }
            parameters.insert(
                name.clone(),
                Local {
                    slot,
                    kind: VariableKind::Let,
                    imported: false,
                    initialized: true,
                    celled,
                },
            );
        }

        let parameter_count = function.parameters.len() as u32;
        let outer_finalizer_depth =
            std::mem::replace(&mut self.function_finalizer_depth, self.finalizers.len());
        // A nested body cannot jump out to an enclosing loop, so it starts with no handlers of
        // its own to account for.
        let outer_handler_depth = std::mem::replace(&mut self.handler_depth, 0);
        let outer = FunctionState {
            code: std::mem::take(&mut self.code),
            scopes: std::mem::replace(&mut self.scopes, vec![parameters]),
            local_count: std::mem::replace(&mut self.local_count, parameter_count),
            upvalues: std::mem::take(&mut self.upvalues),
            upvalue_names: std::mem::take(&mut self.upvalue_names),
            captured: std::mem::replace(&mut self.captured, captured),
        };
        self.enclosing.push(outer);

        // A captured parameter arrives as a plain value in its slot, so the cell has to be
        // created and seeded before the body can read it.
        for slot in celled_parameters {
            self.code.push(Instruction::LoadLocal(slot));
            self.code.push(Instruction::NewCell(slot));
            self.code.push(Instruction::StoreCell(slot));
        }

        // Destructure the parameters that bind a pattern, in declaration order, before anything
        // in the body can read them. `bind_pattern` is the same code a `const { a } = x` uses, so
        // defaults, renaming, nesting and rest behave identically wherever they are written.
        for (slot, pattern, span) in destructured {
            self.code.push(Instruction::LoadLocal(slot));
            self.bind_pattern(VariableKind::Let, pattern, span);
        }

        // `...rest` is the arguments beyond the declared parameters. Taken from the arguments
        // object rather than from slots, because its length is decided by the caller.
        if let Some(rest) = &function.rest {
            self.code.push(Instruction::LoadArguments);
            self.push_constant(Constant::Number(f64::from(parameter_count).to_bits()));
            let slice = self.constant_index(Constant::String("slice".to_owned()));
            self.code.push(Instruction::CallMethod {
                name: slice,
                argc: 1,
            });
            self.bind_pattern(VariableKind::Let, rest, function.span);
        }

        if forwards_to_super {
            self.load_super();
            self.code.push(Instruction::SuperForward);
            self.code.push(Instruction::Pop);
        }
        for field in fields {
            self.code.push(Instruction::LoadThis);
            match &field.key {
                ObjectKey::Private(name) => {
                    let name = self.constant_index(Constant::String(private_key(name)));
                    self.emit_field_value(field);
                    self.code.push(Instruction::SetProperty(name));
                }
                ObjectKey::Static(key) => {
                    let name = self.constant_index(Constant::String(key.clone()));
                    self.emit_field_value(field);
                    self.code.push(Instruction::SetProperty(name));
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.emit_field_value(field);
                    self.code.push(Instruction::SetComputed);
                }
            }
            self.code.push(Instruction::Pop);
        }
        self.statements(&function.body);
        // Falling off the end of a function returns undefined. Emitted unconditionally rather
        // than only when the body cannot fall through: proving that needs flow analysis, and an
        // unreachable epilogue costs two instructions.
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::Return);

        let restored = self
            .enclosing
            .pop()
            .expect("this function pushed its own enclosing state");
        let proto = FunctionProto {
            name: function.name.clone(),
            arrow: function.arrow,
            asynchronous: function.asynchronous,
            // An async body suspends at every `await`, which is a `yield` in every way the
            // machine cares about -- so it is compiled as a generator and driven by the async
            // driver rather than by the program.
            generator: function.generator || function.asynchronous,
            parameter_count,
            local_count: std::mem::replace(&mut self.local_count, restored.local_count),
            code: std::mem::replace(&mut self.code, restored.code),
            upvalues: std::mem::replace(&mut self.upvalues, restored.upvalues),
        };
        self.scopes = restored.scopes;
        self.function_finalizer_depth = outer_finalizer_depth;
        self.handler_depth = outer_handler_depth;
        self.upvalue_names = restored.upvalue_names;
        self.captured = restored.captured;

        let index = self.functions.len() as u32;
        self.functions.push(proto);
        Some(index)
    }

    fn return_statement(&mut self, value: Option<&Expression>, span: Span) {
        if !self.compiling_function() {
            self.error(Some(span), "`return` outside of a function");
            return;
        }
        match value {
            Some(value) => self.expression(value),
            None => self.push_constant(Constant::Undefined),
        }
        if !self.finalizers.is_empty() {
            // The value is computed before the cleanups run -- that is the observable order --
            // but the cleanups may use the stack, so it is parked in a local across them.
            let slot = format!("%ret{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.scopes.push(HashMap::new());
            self.declare_bound(VariableKind::Let, &slot, span);
            self.unwind_finalizers(self.function_finalizer_depth);
            self.load_hidden(&slot);
            self.scopes.pop().expect("return scope was just pushed");
        }
        self.code.push(Instruction::Return);
    }

    /// `object.property = value`, and the compound forms.
    ///
    /// The object expression is evaluated once. `o().x += 1` must not call `o` twice, so the
    /// reference is duplicated on the stack rather than the expression being compiled twice --
    /// which would be the easy mistake and would double any side effect in the receiver.
    /// `x++` and friends.
    ///
    /// The two forms differ only in when the value is duplicated: a prefix update yields the new
    /// value, a postfix one the old. That is the whole reason this is not lowered to `x = x + 1`.
    fn update(&mut self, increment: bool, prefix: bool, target: &str, span: Span) {
        let (load, store) = match self.resolve_or_capture(target) {
            Resolution::Local(local) if local.celled => (
                Instruction::LoadCell(local.slot),
                Instruction::StoreCell(local.slot),
            ),
            Resolution::Local(local) => (
                Instruction::LoadLocal(local.slot),
                Instruction::StoreLocal(local.slot),
            ),
            Resolution::Upvalue(index) => (
                Instruction::LoadUpvalue(index),
                Instruction::StoreUpvalue(index),
            ),
            Resolution::Global(slot) => (
                Instruction::LoadGlobal(slot),
                Instruction::StoreGlobal(slot),
            ),
            Resolution::Unresolved => {
                self.error(Some(span), format!("unresolved identifier `{target}`"));
                self.push_constant(Constant::Undefined);
                return;
            }
        };

        self.code.push(load);
        if !prefix {
            // Keep the old value beneath the arithmetic; the store below consumes the new one.
            self.code.push(Instruction::Dup);
        }
        self.push_constant(Constant::Number(1.0f64.to_bits()));
        self.code.push(Instruction::Binary(if increment {
            BinaryOp::Add
        } else {
            BinaryOp::Subtract
        }));
        if prefix {
            self.code.push(Instruction::Dup);
        }
        self.code.push(store);
    }

    /// An array literal, which may contain spreads.
    ///
    /// Without a spread this is one `NewArray` over the elements. With one, the array is built
    /// incrementally: the run of literal elements before each spread becomes its own `NewArray`
    /// and the spread appends to it, because a spread's length is not known until it runs.
    fn array_literal(&mut self, elements: &[ListElement], span: Span) {
        if elements.len() > u32::MAX as usize {
            self.error(Some(span), "array literal has too many elements");
            return;
        }

        if !elements
            .iter()
            .any(|element| matches!(element, ListElement::Spread(_)))
        {
            for element in elements {
                match element {
                    ListElement::Item(value) => self.expression(value),
                    // A hole occupies its position without contributing a value, which is what
                    // makes it different from an element that happens to be undefined.
                    ListElement::Hole => self.code.push(Instruction::PushHole),
                    ListElement::Spread(_) => {
                        unreachable!("just checked that there are no spreads")
                    }
                }
            }
            self.code.push(Instruction::NewArray {
                count: elements.len() as u32,
            });
            return;
        }

        let mut pending = 0u32;
        let mut started = false;
        for element in elements {
            match element {
                ListElement::Item(value) => {
                    self.expression(value);
                    pending += 1;
                }
                ListElement::Hole => {
                    self.code.push(Instruction::PushHole);
                    pending += 1;
                }
                ListElement::Spread(value) => {
                    self.flush_array_run(&mut pending, &mut started);
                    self.expression(value);
                    self.code.push(Instruction::SpreadInto);
                }
            }
        }
        self.flush_array_run(&mut pending, &mut started);
    }

    /// Turn the pending literal elements into an array, or append them to the one being built.
    fn flush_array_run(&mut self, pending: &mut u32, started: &mut bool) {
        if !*started {
            self.code.push(Instruction::NewArray { count: *pending });
            *started = true;
            *pending = 0;
            return;
        }
        if *pending > 0 {
            self.code.push(Instruction::NewArray { count: *pending });
            self.code.push(Instruction::SpreadInto);
            *pending = 0;
        }
    }

    /// `object[key] = value`, and the compound forms.
    ///
    /// Both the object and the key are evaluated once, in that order, which is the order the
    /// language specifies and the one a reader assumes. A compound form has to re-read through
    /// the same pair rather than re-evaluating either, so both are duplicated on the stack.
    fn computed_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        property: &Expression,
        value: &Expression,
    ) {
        self.expression(object);
        self.expression(property);

        if operator != AssignmentOperator::Assign {
            self.code.push(Instruction::Dup2);
            self.code.push(Instruction::GetComputed);
            self.expression(value);
            self.code.push(Instruction::Binary(match operator {
                AssignmentOperator::Add => BinaryOp::Add,
                AssignmentOperator::Subtract => BinaryOp::Subtract,
                AssignmentOperator::Multiply => BinaryOp::Multiply,
                AssignmentOperator::Divide => BinaryOp::Divide,
                AssignmentOperator::Remainder => BinaryOp::Remainder,
                AssignmentOperator::Exponentiate => BinaryOp::Exponentiate,
                AssignmentOperator::Assign => unreachable!("handled above"),
            }));
        } else {
            self.expression(value);
        }

        self.code.push(Instruction::SetComputed);
    }

    /// `object.#name = value`, and the compound forms.
    ///
    /// The same shape as a member assignment with a different pair of instructions, which is the
    /// whole difference: the private ones check that the name is already there.
    fn private_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        name: &str,
        value: &Expression,
    ) {
        let name = self.constant_index(Constant::String(private_key(name)));
        self.expression(object);
        if operator != AssignmentOperator::Assign {
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::GetPrivate(name));
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }
        self.code.push(Instruction::SetPrivate(name));
    }

    fn member_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        property: &str,
        value: &Expression,
    ) {
        let name = self.constant_index(Constant::String(property.to_owned()));
        self.expression(object);

        if operator != AssignmentOperator::Assign {
            // Read the current value from the same object reference the write will use.
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::GetProperty(name));
            self.expression(value);
            self.code.push(Instruction::Binary(match operator {
                AssignmentOperator::Add => BinaryOp::Add,
                AssignmentOperator::Subtract => BinaryOp::Subtract,
                AssignmentOperator::Multiply => BinaryOp::Multiply,
                AssignmentOperator::Divide => BinaryOp::Divide,
                AssignmentOperator::Remainder => BinaryOp::Remainder,
                AssignmentOperator::Exponentiate => BinaryOp::Exponentiate,
                AssignmentOperator::Assign => unreachable!("handled above"),
            }));
        } else {
            self.expression(value);
        }

        // `SetProperty` leaves the assigned value, which is what an assignment expression
        // evaluates to.
        self.code.push(Instruction::SetProperty(name));
    }

    /// Whether an argument list contains a spread, and so needs its length computed at runtime.
    fn has_spread(arguments: &[ListElement]) -> bool {
        arguments
            .iter()
            .any(|argument| matches!(argument, ListElement::Spread(_)))
    }

    /// Emit the arguments of a call with no spread, in order, returning the count.
    fn emit_arguments(&mut self, arguments: &[ListElement]) -> u16 {
        for argument in arguments {
            let ListElement::Item(value) = argument else {
                unreachable!("callers check for spreads first");
            };
            self.expression(value);
        }
        arguments.len() as u16
    }

    fn call(&mut self, callee: &Expression, arguments: &[ListElement], span: Span) {
        let is_console_log = matches!(self.resolve("console"), Resolution::Unresolved)
            && matches!(
                &callee.kind,
                ExpressionKind::StaticMember { object, property, optional: false }
                    if property == "log"
                        && matches!(&object.kind, ExpressionKind::Identifier(name) if name == "console")
            );
        if arguments.len() > u16::MAX as usize {
            self.error(Some(span), "too many call arguments");
            return;
        }

        if is_console_log {
            // Still a compile-time shape rather than a value. `console` becomes an ordinary
            // object once there are objects to hold it; until then this is the only way to
            // reach stdout, and turning it into a real call would mean inventing a callable
            // with no object model underneath it.
            if Self::has_spread(arguments) {
                // `HostCall` carries its arity in the bytecode, and a spread does not have one
                // until it runs. Refused rather than flattened into a single array argument,
                // which would print `[1, 2]` where the program asked for `1 2`.
                self.error(Some(span), "spread into `console.log` is not supported yet");
                self.push_constant(Constant::Undefined);
                return;
            }
            let argc = self.emit_arguments(arguments);
            self.code.push(Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc,
            });
            return;
        }

        // A method call keeps its receiver rather than reading the function out first. With a
        // spread the receiver has to be kept explicitly instead, because `Apply` takes it as an
        // operand -- so each shape below has a fixed-arity form and a spread form.
        let spreading = Self::has_spread(arguments);
        match &callee.kind {
            // `super.m(...)` resolves the function on the parent's prototype but runs it against
            // the current `this` -- that is the whole point of it, and an ordinary call would
            // lose the receiver.
            ExpressionKind::SuperMember { .. } if !spreading => {
                self.expression(callee);
                let argc = self.emit_arguments(arguments);
                self.code.push(Instruction::SuperCall { argc });
                return;
            }
            ExpressionKind::SuperMember { .. } => {
                self.error(
                    Some(span),
                    "spread into a `super` call is not supported yet",
                );
                self.push_constant(Constant::Undefined);
                return;
            }
            // `this.#m()` is a method call: the private name is read off the receiver and the
            // receiver becomes `this`. Reached through `Apply` rather than a `CallPrivate`
            // instruction, because the shape `Apply` already takes -- receiver, callee,
            // arguments -- is exactly this one.
            ExpressionKind::PrivateMember { object, name } => {
                let name = self.constant_index(Constant::String(private_key(name)));
                self.expression(object);
                self.code.push(Instruction::Dup);
                self.code.push(Instruction::GetPrivate(name));
                self.array_literal(arguments, span);
                self.code.push(Instruction::Apply);
                return;
            }
            ExpressionKind::StaticMember {
                object,
                property,
                optional,
            } => {
                let name = self.constant_index(Constant::String(property.clone()));
                self.expression(object);
                // `a?.b()` short-circuits before the method is even read, so the receiver is the
                // thing tested and the call never happens.
                self.short_circuit_if_nullish(*optional, span);
                if spreading {
                    // The receiver stays beneath the method, which is what `Apply` expects.
                    self.code.push(Instruction::Dup);
                    self.code.push(Instruction::GetProperty(name));
                    self.array_literal(arguments, span);
                    self.code.push(Instruction::Apply);
                } else {
                    let argc = self.emit_arguments(arguments);
                    self.code.push(Instruction::CallMethod { name, argc });
                }
                return;
            }
            ExpressionKind::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                if spreading {
                    self.code.push(Instruction::Dup);
                    self.expression(property);
                    self.code.push(Instruction::GetComputed);
                    self.array_literal(arguments, span);
                    self.code.push(Instruction::Apply);
                } else {
                    self.expression(property);
                    let argc = self.emit_arguments(arguments);
                    self.code.push(Instruction::CallComputed { argc });
                }
                return;
            }
            _ => {}
        }

        // Everything else is a real call. The callee is evaluated first so it sits below its
        // arguments, which is both the evaluation order the language requires and the layout
        // the VM needs to find it at a fixed depth.
        if spreading {
            // A plain call has no receiver, and `Apply` takes one, so `undefined` stands in.
            self.push_constant(Constant::Undefined);
            self.expression(callee);
            self.array_literal(arguments, span);
            self.code.push(Instruction::Apply);
            return;
        }
        self.expression(callee);
        let argc = self.emit_arguments(arguments);
        self.code.push(Instruction::Call { argc });
    }

    /// Names visible to the function being compiled, and how each is reached.
    fn resolve(&self, name: &str) -> Resolution {
        if let Some(local) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return Resolution::Local(*local);
        }
        let Some(module) = self.enclosing.first() else {
            // Compiling the module body: its scopes were searched above, so there is nowhere
            // else a name could be.
            return Resolution::Unresolved;
        };
        if let Some(local) = module.scopes[0].get(name) {
            return Resolution::Global(local.slot);
        }
        Resolution::Unresolved
    }

    /// Resolve `name` and, if it lives in an enclosing function, capture it.
    ///
    /// Split from [`Compiler::resolve`] because capturing mutates the upvalue lists of every
    /// function between the binding and here, and a read-only lookup is wanted in places that
    /// must not have that side effect -- the `console` check in [`Compiler::call`] above all,
    /// which asks whether a name exists and would otherwise capture one by asking.
    fn resolve_or_capture(&mut self, name: &str) -> Resolution {
        if let Some(local) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return Resolution::Local(*local);
        }
        // The capture is tried before the global, because an enclosing function's binding
        // shadows a module binding of the same name. Asking `resolve` first would jump from
        // here to module scope and skip every function in between -- reading the wrong binding
        // whenever a name is reused, and reusing a name like `value` is not unusual. That was
        // invisible for as long as a module binding was only created at the line that declared
        // it, because the name did not exist yet when the inner function was compiled.
        if let Some(index) = self.capture(name) {
            return Resolution::Upvalue(index);
        }
        self.resolve(name)
    }

    /// Thread a binding from the function that owns it down to the one being compiled.
    ///
    /// Every function in between gains an upvalue too, because a closure can only reach cells
    /// its immediately enclosing frame holds -- a two-level capture is two hops, not one long
    /// one. Levels are indices into `enclosing` followed by the current function.
    fn capture(&mut self, name: &str) -> Option<u32> {
        // Scope 0 of level 0 is module scope, whose bindings are globals rather than captures --
        // but a *block* inside the module body is an ordinary scope, and a closure created there
        // captures from it like any other. Skipping level 0 entirely missed exactly that: a
        // `for-of` at module level whose body creates a closure over the loop variable.
        let (owner, local) = (0..self.enclosing.len()).rev().find_map(|level| {
            let skip = usize::from(level == 0);
            self.enclosing[level]
                .scopes
                .iter()
                .enumerate()
                .rev()
                .filter(|(index, _)| *index >= skip)
                .find_map(|(_, scope)| scope.get(name))
                .map(|local| (level, *local))
        })?;
        if !local.celled {
            // The capture analysis said no nested function referred to this name, yet here we
            // are. Compiling it anyway would read an uninitialised slot as though it were a
            // cell, so refuse rather than emit that.
            return None;
        }

        let mut source = Upvalue::ParentLocal(local.slot);
        for level in (owner + 1)..=self.enclosing.len() {
            let index = self.add_upvalue(level, name, source);
            source = Upvalue::ParentUpvalue(index);
        }
        // The last index added belongs to the function being compiled.
        self.upvalue_names.get(name).copied()
    }

    /// Add one upvalue to the function at `level`, reusing an existing capture of the same name.
    fn add_upvalue(&mut self, level: usize, name: &str, source: Upvalue) -> u32 {
        let (upvalues, names) = if level == self.enclosing.len() {
            (&mut self.upvalues, &mut self.upvalue_names)
        } else {
            let state = &mut self.enclosing[level];
            (&mut state.upvalues, &mut state.upvalue_names)
        };
        if let Some(existing) = names.get(name) {
            return *existing;
        }
        let index = upvalues.len() as u32;
        upvalues.push(source);
        names.insert(name.to_owned(), index);
        index
    }

    fn compiling_function(&self) -> bool {
        !self.enclosing.is_empty()
    }

    fn push_literal(&mut self, literal: &Literal) {
        self.push_constant(match literal {
            Literal::Undefined => Constant::Undefined,
            Literal::Null => Constant::Null,
            Literal::Boolean(value) => Constant::Boolean(*value),
            Literal::Number(value) => Constant::Number(*value),
            Literal::String(value) => Constant::String(value.clone()),
        });
    }

    fn push_constant(&mut self, constant: Constant) {
        let index = self.constant_index(constant);
        self.code.push(Instruction::Const(index));
    }

    /// Intern a constant, returning its table index.
    ///
    /// Property names repeat heavily -- every `.length` in a program is the same string -- and
    /// an instruction that names one by index rather than carrying it inline is what lets the
    /// table hold one copy.
    fn constant_index(&mut self, constant: Constant) -> u32 {
        if let Some(existing) = self
            .constants
            .iter()
            .position(|candidate| *candidate == constant)
        {
            return existing as u32;
        }
        let index = self.constants.len() as u32;
        self.constants.push(constant);
        index
    }

    fn emit_jump(&mut self, instruction: Instruction) -> usize {
        let index = self.code.len();
        self.code.push(instruction);
        index
    }

    fn patch_jump(&mut self, index: usize) {
        let target = self.code.len() as u32;
        match &mut self.code[index] {
            Instruction::Jump(offset)
            | Instruction::JumpIfFalse(offset)
            | Instruction::JumpIfTruthy(offset)
            | Instruction::JumpIfNotNullish(offset)
            // A handler's target is patched exactly like a jump's: neither is known until the
            // code it points past has been emitted.
            | Instruction::PushHandler(offset) => *offset = target,
            other => unreachable!("patch_jump called for {other:?}, which carries no target"),
        }
    }
}

/// Names that nested functions inside `body` refer to.
///
/// Deliberately an over-approximation: it collects every identifier appearing anywhere inside a
/// nested function, without modelling that function's own shadowing. A local that is boxed but
/// never actually captured still behaves correctly -- one pointer chase, no semantic difference
/// -- whereas a captured local that was *missed* would be read out of a dead frame. The analysis
/// is allowed to be wrong only in the direction that costs performance.
fn names_captured_by_nested_functions(body: &[Statement]) -> HashSet<String> {
    let mut captured = HashSet::new();
    for statement in body {
        collect_nested_function_names(statement, &mut captured, false);
    }
    captured
}

/// As [`names_captured_by_nested_functions`], but for a whole function rather than a body.
///
/// A parameter default is compiled in the function's own scope and may contain a nested function
/// of its own -- `function f(a, cb = () => a)` captures `a`, and nothing in the body says so. The
/// body-only scan leaves `a` unboxed, and the arrow then cannot reach it at all.
fn names_captured_by_this_function(function: &Function) -> HashSet<String> {
    let mut captured = names_captured_by_nested_functions(&function.body);
    for parameter in &function.parameters {
        collect_pattern_default_captures(&parameter.pattern, &mut captured);
    }
    if let Some(rest) = &function.rest {
        collect_pattern_default_captures(rest, &mut captured);
    }
    captured
}

/// Names that nested functions inside a pattern's defaults refer to.
fn collect_pattern_default_captures(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier(_) => {}
        Pattern::WithDefault { inner, default } => {
            collect_identifiers(default, names, false);
            collect_pattern_default_captures(inner, names);
        }
        Pattern::Object { properties, .. } => {
            for property in properties {
                collect_pattern_default_captures(&property.value, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                collect_pattern_default_captures(element, names);
            }
            if let Some(rest) = rest {
                collect_pattern_default_captures(rest, names);
            }
        }
    }
}

fn collect_nested_function_names(
    statement: &Statement,
    names: &mut HashSet<String>,
    inside_function: bool,
) {
    match &statement.kind {
        StatementKind::Function(function) => {
            for statement in &function.body {
                collect_nested_function_names(statement, names, true);
            }
        }
        // Every method body is a nested function, so its free names are capture candidates.
        StatementKind::Class(class) => {
            if let Some(superclass) = &class.superclass {
                collect_identifiers(superclass, names, inside_function);
            }
            collect_class_names(class, names);
        }
        StatementKind::Variable { declarations, .. } => {
            for declaration in declarations {
                if inside_function {
                    collect_pattern_names(&declaration.pattern, names);
                }
                if let Some(initializer) = &declaration.initializer {
                    collect_identifiers(initializer, names, inside_function);
                }
            }
        }
        StatementKind::Expression(expression) | StatementKind::ExportDefault(expression) => {
            collect_identifiers(expression, names, inside_function);
        }
        StatementKind::Return(value) => {
            if let Some(value) = value {
                collect_identifiers(value, names, inside_function);
            }
        }
        StatementKind::Block(body) => {
            for statement in body {
                collect_nested_function_names(statement, names, inside_function);
            }
        }
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            collect_identifiers(test, names, inside_function);
            collect_nested_function_names(consequent, names, inside_function);
            if let Some(alternate) = alternate {
                collect_nested_function_names(alternate, names, inside_function);
            }
        }
        StatementKind::While { test, body } => {
            collect_identifiers(test, names, inside_function);
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::Labelled { body, .. } => {
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::ForIn { object, body, .. } => {
            collect_identifiers(object, names, inside_function);
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::DoWhile { body, test } => {
            collect_nested_function_names(body, names, inside_function);
            collect_identifiers(test, names, inside_function);
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            collect_identifiers(discriminant, names, inside_function);
            for case in cases {
                if let Some(test) = &case.test {
                    collect_identifiers(test, names, inside_function);
                }
                for statement in &case.body {
                    collect_nested_function_names(statement, names, inside_function);
                }
            }
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            if let Some(init) = init {
                collect_nested_function_names(init, names, inside_function);
            }
            if let Some(test) = test {
                collect_identifiers(test, names, inside_function);
            }
            if let Some(update) = update {
                collect_identifiers(update, names, inside_function);
            }
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::ForOf { iterable, body, .. } => {
            collect_identifiers(iterable, names, inside_function);
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::Throw(value) => collect_identifiers(value, names, inside_function),
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            for statement in block.iter().chain(finalizer) {
                collect_nested_function_names(statement, names, inside_function);
            }
            if let Some(handler) = handler {
                for statement in &handler.body {
                    collect_nested_function_names(statement, names, inside_function);
                }
            }
        }
        StatementKind::Break { .. }
        | StatementKind::Continue { .. }
        | StatementKind::Empty
        | StatementKind::Unsupported { .. } => {}
    }
}

/// Walk an expression, recording identifiers when `collecting`.
///
/// It descends unconditionally and gates only the recording. That distinction matters: a function
/// *expression* nested in a `return` is a nested function whose free names are captures, and an
/// early return on `!collecting` would never reach it. That was a real bug -- `function f(n) {
/// return (m) => n + m; }` compiled with `n` unboxed, so the arrow could not resolve it.
/// Names a class's methods and field initialisers refer to.
///
/// Field initialisers count: they run inside the constructor, so a name one of them reads is
/// captured by the constructor exactly as a method body's would be.
fn collect_class_names(class: &Class, names: &mut HashSet<String>) {
    let bodies = class
        .constructor
        .iter()
        .chain(class.methods.iter().map(|method| &method.function));
    for function in bodies {
        for statement in &function.body {
            collect_nested_function_names(statement, names, true);
        }
    }
    for field in &class.fields {
        if let ObjectKey::Computed(key) = &field.key {
            collect_identifiers(key, names, true);
        }
        if let Some(value) = &field.value {
            collect_identifiers(value, names, true);
        }
    }
}

/// Every name a pattern binds.
/// The binary operation a compound assignment performs before storing.
fn compound_operator(operator: AssignmentOperator) -> BinaryOp {
    match operator {
        AssignmentOperator::Add => BinaryOp::Add,
        AssignmentOperator::Subtract => BinaryOp::Subtract,
        AssignmentOperator::Multiply => BinaryOp::Multiply,
        AssignmentOperator::Divide => BinaryOp::Divide,
        AssignmentOperator::Remainder => BinaryOp::Remainder,
        AssignmentOperator::Exponentiate => BinaryOp::Exponentiate,
        AssignmentOperator::Assign => unreachable!("a plain assignment performs no operation"),
    }
}

/// The reserved property name a private member is stored under.
///
/// Reserved rather than ordinary, so nothing that enumerates own string properties can see it and
/// no computed access can spell it -- `obj["#x"]` names a different property, as it should.
fn private_key(name: &str) -> String {
    format!("@@private:{name}")
}

/// The names a pattern binds, in source order.
///
/// Ordered, unlike [`collect_pattern_names`], because these decide slot numbering and the
/// bytecode is a cache key: the same source has to produce the same instructions, and a hash set
/// does not promise that.
fn pattern_names_in_order(pattern: &Pattern, names: &mut Vec<String>) {
    match pattern {
        Pattern::Identifier(name) => names.push(name.clone()),
        Pattern::WithDefault { inner, .. } => pattern_names_in_order(inner, names),
        Pattern::Object { properties, rest } => {
            for property in properties {
                pattern_names_in_order(&property.value, names);
            }
            if let Some(rest) = rest {
                names.push(rest.clone());
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                pattern_names_in_order(element, names);
            }
            if let Some(rest) = rest {
                pattern_names_in_order(rest, names);
            }
        }
    }
}

fn collect_pattern_names(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier(name) => {
            names.insert(name.clone());
        }
        Pattern::WithDefault { inner, .. } => collect_pattern_names(inner, names),
        Pattern::Object { properties, rest } => {
            for property in properties {
                collect_pattern_names(&property.value, names);
            }
            if let Some(rest) = rest {
                names.insert(rest.clone());
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                collect_pattern_names(element, names);
            }
            if let Some(rest) = rest {
                collect_pattern_names(rest, names);
            }
        }
    }
}

fn collect_identifiers(expression: &Expression, names: &mut HashSet<String>, collecting: bool) {
    match &expression.kind {
        // A nested function's body is always collected, whatever the enclosing flag says.
        ExpressionKind::Function(function) => {
            for statement in &function.body {
                collect_nested_function_names(statement, names, true);
            }
        }
        // A class expression's methods and field initialisers are nested functions too, and a
        // field initialiser closing over the enclosing scope is exactly what g03 exercises.
        ExpressionKind::Class(class) => {
            collect_class_names(class, names);
        }
        ExpressionKind::Identifier(name) => {
            if collecting {
                names.insert(name.clone());
            }
        }
        ExpressionKind::Update { target, .. } => {
            if collecting {
                names.insert(target.clone());
            }
        }
        ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::Unsupported { .. } => {}
        ExpressionKind::SuperCall { arguments } => {
            for argument in arguments {
                collect_identifiers(argument, names, collecting);
            }
        }
        ExpressionKind::Unary { argument, .. } => collect_identifiers(argument, names, collecting),
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            collect_identifiers(left, names, collecting);
            collect_identifiers(right, names, collecting);
        }
        ExpressionKind::Assignment { target, value, .. } => {
            if collecting {
                names.insert(target.clone());
            }
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            collect_identifiers(test, names, collecting);
            collect_identifiers(consequent, names, collecting);
            collect_identifiers(alternate, names, collecting);
        }
        ExpressionKind::Call { callee, arguments } | ExpressionKind::New { callee, arguments } => {
            collect_identifiers(callee, names, collecting);
            for argument in arguments {
                let (ListElement::Item(value) | ListElement::Spread(value)) = argument else {
                    continue;
                };
                collect_identifiers(value, names, collecting);
            }
        }
        ExpressionKind::Array { elements } => {
            for element in elements {
                let (ListElement::Item(value) | ListElement::Spread(value)) = element else {
                    continue;
                };
                collect_identifiers(value, names, collecting);
            }
        }
        ExpressionKind::Sequence { operands } => {
            for operand in operands {
                collect_identifiers(operand, names, collecting);
            }
        }
        ExpressionKind::Yield { value, .. } => {
            if let Some(value) = value {
                collect_identifiers(value, names, collecting);
            }
        }
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(property, names, collecting);
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(property, names, collecting);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::MemberAssignment { object, value, .. } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::Chain { expression } | ExpressionKind::Await(expression) => {
            collect_identifiers(expression, names, collecting);
        }
        ExpressionKind::RegExp { .. } => {}
        ExpressionKind::PrivateMember { object, .. } => {
            collect_identifiers(object, names, collecting);
        }
        ExpressionKind::PrivateAssignment { object, value, .. } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::Template { expressions, .. } => {
            for expression in expressions {
                collect_identifiers(expression, names, collecting);
            }
        }
        ExpressionKind::Delete { object, property } => {
            collect_identifiers(object, names, collecting);
            if let ObjectKey::Computed(key) = property.as_ref() {
                collect_identifiers(key, names, collecting);
            }
        }
        ExpressionKind::Object { members } => {
            for member in members {
                match member {
                    ObjectMember::Property(property) => {
                        if let ObjectKey::Computed(key) = &property.key {
                            collect_identifiers(key, names, collecting);
                        }
                        collect_identifiers(&property.value, names, collecting);
                    }
                    ObjectMember::Spread(source) => {
                        collect_identifiers(source, names, collecting);
                    }
                    ObjectMember::Accessor { key, function, .. } => {
                        if let ObjectKey::Computed(key) = key {
                            collect_identifiers(key, names, collecting);
                        }
                        // The body is a nested function, so every name in it is a candidate.
                        for statement in &function.body {
                            collect_nested_function_names(statement, names, true);
                        }
                    }
                }
            }
        }
        ExpressionKind::StaticMember { object, .. } => {
            collect_identifiers(object, names, collecting);
        }
    }
}

fn expression_references(expression: &Expression, name: &str) -> bool {
    match &expression.kind {
        ExpressionKind::Identifier(identifier) => identifier == name,
        ExpressionKind::Unary { argument, .. } => expression_references(argument, name),
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            expression_references(left, name) || expression_references(right, name)
        }
        ExpressionKind::Assignment { target, value, .. } => {
            target == name || expression_references(value, name)
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expression_references(test, name)
                || expression_references(consequent, name)
                || expression_references(alternate, name)
        }
        ExpressionKind::Call { callee, arguments } | ExpressionKind::New { callee, arguments } => {
            expression_references(callee, name)
                || arguments.iter().any(|argument| match argument {
                    ListElement::Item(value) | ListElement::Spread(value) => {
                        expression_references(value, name)
                    }
                    ListElement::Hole => false,
                })
        }
        ExpressionKind::Array { elements } => elements.iter().any(|element| match element {
            ListElement::Item(value) | ListElement::Spread(value) => {
                expression_references(value, name)
            }
            ListElement::Hole => false,
        }),
        ExpressionKind::Sequence { operands } => operands
            .iter()
            .any(|operand| expression_references(operand, name)),
        ExpressionKind::ComputedMember { object, property, .. } => {
            expression_references(object, name) || expression_references(property, name)
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            expression_references(object, name)
                || expression_references(property, name)
                || expression_references(value, name)
        }
        ExpressionKind::MemberAssignment { object, value, .. } => {
            expression_references(object, name) || expression_references(value, name)
        }
        ExpressionKind::Yield { value, .. } => value
            .as_ref()
            .is_some_and(|value| expression_references(value, name)),
        ExpressionKind::Class(_)
        | ExpressionKind::This
        | ExpressionKind::Function(_)
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::Unsupported { .. } => false,
        ExpressionKind::SuperCall { arguments } => arguments
            .iter()
            .any(|argument| expression_references(argument, name)),
        ExpressionKind::Chain { expression } | ExpressionKind::Await(expression) => {
            expression_references(expression, name)
        }
        ExpressionKind::RegExp { .. } => false,
        ExpressionKind::PrivateMember { object, .. } => expression_references(object, name),
        ExpressionKind::PrivateAssignment { object, value, .. } => {
            expression_references(object, name) || expression_references(value, name)
        }
        ExpressionKind::Template { expressions, .. } => expressions
            .iter()
            .any(|expression| expression_references(expression, name)),
        ExpressionKind::Delete { object, property } => {
            expression_references(object, name)
                || matches!(property.as_ref(), ObjectKey::Computed(key) if expression_references(key, name))
        }
        ExpressionKind::Object { members } => members.iter().any(|member| match member {
            ObjectMember::Property(property) => {
                expression_references(&property.value, name)
                    || matches!(&property.key, ObjectKey::Computed(key) if expression_references(key, name))
            }
            ObjectMember::Spread(source) => expression_references(source, name),
            ObjectMember::Accessor { key, .. } => {
                matches!(key, ObjectKey::Computed(key) if expression_references(key, name))
            }
        }),
        ExpressionKind::StaticMember { object, .. } => expression_references(object, name),
        ExpressionKind::Update { target, .. } => target == name,
        ExpressionKind::Literal(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(body: Vec<Statement>) -> Module {
        Module {
            source_kind: kiln_hir::SourceKind::JavaScript,
            source_goal: kiln_hir::SourceGoal::Module,
            items: Vec::new(),
            body,
            requested_modules: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            diagnostics: Vec::new(),
            panicked: false,
        }
    }

    fn number(value: f64) -> Expression {
        Expression {
            kind: ExpressionKind::Literal(Literal::Number(value.to_bits())),
            span: Span::default(),
        }
    }

    fn identifier(name: &str) -> Expression {
        Expression {
            kind: ExpressionKind::Identifier(name.into()),
            span: Span::default(),
        }
    }

    fn statement(kind: StatementKind) -> Statement {
        Statement {
            kind,
            span: Span::default(),
        }
    }

    fn function(name: &str, parameters: &[&str], body: Vec<Statement>) -> Statement {
        statement(StatementKind::Function(Box::new(kiln_hir::Function {
            asynchronous: false,
            name: Some(name.to_owned()),
            arrow: false,
            generator: false,
            parameters: parameters
                .iter()
                .map(|name| kiln_hir::Parameter {
                    pattern: kiln_hir::Pattern::Identifier((*name).to_owned()),
                    span: Span::default(),
                })
                .collect(),
            rest: None,
            body,
            span: Span::default(),
        })))
    }

    fn errors(body: Vec<Statement>) -> Vec<String> {
        compile(&module(body))
            .expect_err("expected the module to be rejected")
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    /// An arrow function taking the given parameters.
    fn arrow(parameters: &[&str], body: Vec<Statement>) -> Expression {
        Expression {
            kind: ExpressionKind::Function(Box::new(kiln_hir::Function {
                asynchronous: false,
                name: None,
                arrow: true,
                generator: false,
                parameters: parameters
                    .iter()
                    .map(|name| kiln_hir::Parameter {
                        pattern: kiln_hir::Pattern::Identifier((*name).to_owned()),
                        span: Span::default(),
                    })
                    .collect(),
                rest: None,
                body,
                span: Span::default(),
            })),
            span: Span::default(),
        }
    }

    /// The capture analysis has to descend into a function *expression* nested in a `return`.
    /// It did not at first -- it stopped as soon as it was not already collecting -- so
    /// `function f(n) { return (m) => n + m; }` compiled with `n` unboxed and the arrow could
    /// not resolve it. This is that program.
    #[test]
    fn an_arrow_returned_from_a_function_captures_its_parameter() {
        let program = compile(&module(vec![function(
            "make_adder",
            &["n"],
            vec![statement(StatementKind::Return(Some(arrow(
                &["m"],
                vec![statement(StatementKind::Return(Some(Expression {
                    kind: ExpressionKind::Binary {
                        operator: kiln_hir::BinaryOperator::Add,
                        left: Box::new(identifier("n")),
                        right: Box::new(identifier("m")),
                    },
                    span: Span::default(),
                })))],
            ))))],
        )]))
        .expect("compiles");

        let outer = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("make_adder"))
            .expect("outer proto");
        assert!(
            outer
                .code
                .iter()
                .any(|i| matches!(i, Instruction::NewCell(_))),
            "the captured parameter must be boxed"
        );
        let inner = program
            .functions
            .iter()
            .find(|proto| proto.name.is_none())
            .expect("the arrow has no name");
        assert!(inner.code.contains(&Instruction::LoadUpvalue(0)));
    }

    /// A computed key is an arbitrary expression evaluated in source order, so it must be
    /// compiled rather than folded into a constant.
    #[test]
    fn a_computed_object_key_is_evaluated_not_interned() {
        let program = compile(&module(vec![statement(StatementKind::Expression(
            Expression {
                kind: ExpressionKind::Object {
                    members: vec![
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("fixed".into()),
                            value: number(1.0),
                            span: Span::default(),
                        }),
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Computed(Expression {
                                kind: ExpressionKind::Binary {
                                    operator: kiln_hir::BinaryOperator::Add,
                                    left: Box::new(number(1.0)),
                                    right: Box::new(number(2.0)),
                                },
                                span: Span::default(),
                            }),
                            value: number(3.0),
                            span: Span::default(),
                        }),
                    ],
                },
                span: Span::default(),
            },
        ))]))
        .expect("compiles");
        assert!(
            program
                .code
                .iter()
                .any(|i| matches!(i, Instruction::Binary(_))),
            "the computed key is evaluated, not folded to a constant"
        );
    }

    #[test]
    fn a_compound_member_assignment_evaluates_its_receiver_once() {
        let program = compile(&module(vec![
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("o".into()),
                    initializer: Some(Expression {
                        kind: ExpressionKind::Object {
                            members: Vec::new(),
                        },
                        span: Span::default(),
                    }),
                    span: Span::default(),
                }],
            }),
            statement(StatementKind::Expression(Expression {
                kind: ExpressionKind::MemberAssignment {
                    operator: AssignmentOperator::Add,
                    object: Box::new(identifier("o")),
                    property: "count".into(),
                    value: Box::new(number(1.0)),
                },
                span: Span::default(),
            })),
        ]))
        .expect("compiles");

        let loads = program
            .code
            .iter()
            .filter(|i| matches!(i, Instruction::LoadLocal(_)))
            .count();
        assert_eq!(loads, 1, "the receiver is loaded once and duplicated");
        assert!(program.code.contains(&Instruction::Dup));
    }

    /// Property names repeat across a program, so they are interned rather than carried inline.
    #[test]
    fn repeated_property_names_share_one_constant() {
        let read = |name: &str| {
            statement(StatementKind::Expression(Expression {
                kind: ExpressionKind::StaticMember {
                    optional: false,
                    object: Box::new(identifier("o")),
                    property: name.into(),
                },
                span: Span::default(),
            }))
        };
        let program = compile(&module(vec![
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("o".into()),
                    initializer: Some(Expression {
                        kind: ExpressionKind::Object {
                            members: Vec::new(),
                        },
                        span: Span::default(),
                    }),
                    span: Span::default(),
                }],
            }),
            read("length"),
            read("length"),
            read("length"),
        ]))
        .expect("compiles");

        let names = program
            .constants
            .iter()
            .filter(|constant| **constant == Constant::String("length".to_owned()))
            .count();
        assert_eq!(names, 1, "three reads of .length share one constant");
    }

    #[test]
    fn a_captured_local_is_boxed_and_the_capture_points_at_its_slot() {
        let program = compile(&module(vec![function(
            "outer",
            &[],
            vec![
                statement(StatementKind::Variable {
                    kind: VariableKind::Let,
                    declarations: vec![kiln_hir::VariableDeclarator {
                        pattern: kiln_hir::Pattern::Identifier("captured".into()),
                        initializer: Some(number(0.0)),
                        span: Span::default(),
                    }],
                }),
                function(
                    "inner",
                    &[],
                    vec![statement(StatementKind::Return(Some(identifier(
                        "captured",
                    ))))],
                ),
            ],
        )]))
        .expect("compiles");

        let outer = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("outer"))
            .expect("outer proto");
        assert!(
            outer
                .code
                .iter()
                .any(|i| matches!(i, Instruction::NewCell(_))),
            "the captured local must be boxed"
        );

        let inner = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("inner"))
            .expect("inner proto");
        assert_eq!(inner.upvalues.len(), 1);
        assert!(matches!(inner.upvalues[0], Upvalue::ParentLocal(_)));
        assert!(inner.code.contains(&Instruction::LoadUpvalue(0)));
    }

    /// A local nothing captures must stay an ordinary slot: the indirection is paid where the
    /// semantics need it and nowhere else.
    #[test]
    fn an_uncaptured_local_is_not_boxed() {
        let program = compile(&module(vec![function(
            "plain",
            &[],
            vec![statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("local".into()),
                    initializer: Some(number(1.0)),
                    span: Span::default(),
                }],
            })],
        )]))
        .expect("compiles");
        assert!(
            !program.functions[0]
                .code
                .iter()
                .any(|i| matches!(i, Instruction::NewCell(_))),
            "nothing captures this local, so it must not be boxed"
        );
    }

    /// Two levels of nesting is two hops: the middle function captures from its parent, and the
    /// innermost captures from the middle. A single long hop would reach a frame that is gone.
    #[test]
    fn a_two_level_capture_threads_through_the_function_between() {
        let program = compile(&module(vec![function(
            "outer",
            &["x"],
            vec![function(
                "middle",
                &[],
                vec![function(
                    "inner",
                    &[],
                    vec![statement(StatementKind::Return(Some(identifier("x"))))],
                )],
            )],
        )]))
        .expect("compiles");

        let middle = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("middle"))
            .expect("middle proto");
        let inner = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("inner"))
            .expect("inner proto");

        assert!(
            matches!(middle.upvalues.as_slice(), [Upvalue::ParentLocal(_)]),
            "middle captures outer's boxed parameter directly"
        );
        assert!(
            matches!(inner.upvalues.as_slice(), [Upvalue::ParentUpvalue(0)]),
            "inner captures what middle had already captured"
        );
    }

    /// Writing through a capture has to be a write to the shared cell, not to a frame slot.
    #[test]
    fn assigning_to_a_captured_name_writes_through_the_cell() {
        let program = compile(&module(vec![function(
            "outer",
            &["seed"],
            vec![function(
                "bump",
                &[],
                vec![statement(StatementKind::Expression(Expression {
                    kind: ExpressionKind::Assignment {
                        operator: AssignmentOperator::Assign,
                        target: "seed".into(),
                        value: Box::new(number(1.0)),
                    },
                    span: Span::default(),
                }))],
            )],
        )]))
        .expect("compiles");
        let bump = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("bump"))
            .expect("bump proto");
        assert!(bump.code.contains(&Instruction::StoreUpvalue(0)));
    }

    #[test]
    fn a_function_body_compiles_into_its_own_proto() {
        let program = compile(&module(vec![function(
            "second",
            &["a", "b"],
            vec![statement(StatementKind::Return(Some(identifier("b"))))],
        )]))
        .expect("compiles");

        assert_eq!(program.functions.len(), 1);
        let proto = &program.functions[0];
        assert_eq!(proto.parameter_count, 2);
        assert_eq!(proto.local_count, 2, "parameters occupy the first slots");
        assert_eq!(proto.code.first(), Some(&Instruction::LoadLocal(1)));
        // Every body ends in a Return, so a caller can never fall off the end of one.
        assert_eq!(proto.code.last(), Some(&Instruction::Return));
    }

    /// The binding has to exist before the body is compiled, or a function cannot call itself.
    #[test]
    fn a_function_can_refer_to_itself() {
        let program = compile(&module(vec![function(
            "loop_forever",
            &[],
            vec![statement(StatementKind::Return(Some(Expression {
                kind: ExpressionKind::Call {
                    callee: Box::new(identifier("loop_forever")),
                    arguments: Vec::new(),
                },
                span: Span::default(),
            })))],
        )]))
        .expect("compiles");
        assert!(
            program.functions[0]
                .code
                .contains(&Instruction::Call { argc: 0 })
        );
    }

    /// Module bindings are read through a distinct instruction, because a frame's locals are
    /// its own and a module slot is shared by every frame.
    #[test]
    fn a_function_reads_module_scope_through_a_global_slot() {
        let program = compile(&module(vec![
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("shared".into()),
                    initializer: Some(number(1.0)),
                    span: Span::default(),
                }],
            }),
            function(
                "read",
                &[],
                vec![statement(StatementKind::Return(Some(identifier("shared"))))],
            ),
        ]))
        .expect("compiles");
        assert!(
            program.functions[0]
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
            "module reads must not compile to LoadLocal, which would read the frame's own slot"
        );
    }

    /// Step 1a refused captures outright; 1b compiles them. What must remain true is that a
    /// capture never becomes a `LoadLocal` on the enclosing frame's slot -- that would read
    /// whatever the callee's own frame happened to hold there, which is a plausible wrong
    /// answer rather than a diagnostic.
    #[test]
    fn a_capture_never_compiles_to_a_frame_local_read() {
        let program = compile(&module(vec![function(
            "outer",
            &["x"],
            vec![function(
                "inner",
                &[],
                vec![statement(StatementKind::Return(Some(identifier("x"))))],
            )],
        )]))
        .expect("compiles");
        let inner = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("inner"))
            .expect("inner proto");
        assert!(
            !inner
                .code
                .iter()
                .any(|i| matches!(i, Instruction::LoadLocal(_))),
            "inner has no locals of its own; reading x must go through an upvalue"
        );
        assert!(inner.code.contains(&Instruction::LoadUpvalue(0)));
    }

    #[test]
    fn return_outside_a_function_is_refused() {
        let messages = errors(vec![statement(StatementKind::Return(None))]);
        assert!(
            messages.iter().any(|m| m.contains("outside of a function")),
            "got: {messages:?}"
        );
    }

    #[test]
    fn duplicate_parameters_are_refused() {
        let messages = errors(vec![function("dup", &["a", "a"], Vec::new())]);
        assert!(
            messages.iter().any(|m| m.contains("duplicate parameter")),
            "got: {messages:?}"
        );
    }

    /// A bare `return;` and falling off the end both produce undefined, and both must leave a
    /// value on the stack: a caller unconditionally pops one.
    #[test]
    fn every_exit_from_a_function_yields_a_value() {
        let program = compile(&module(vec![function(
            "bare",
            &[],
            vec![statement(StatementKind::Return(None))],
        )]))
        .expect("compiles");
        let code = &program.functions[0].code;
        let returns = code
            .iter()
            .filter(|instruction| **instruction == Instruction::Return)
            .count();
        assert_eq!(returns, 2, "explicit return plus the fall-through epilogue");
        assert!(matches!(code.first(), Some(Instruction::Const(_))));
    }

    #[test]
    fn compiles_variables_and_arithmetic() {
        let body = vec![
            Statement {
                kind: StatementKind::Variable {
                    kind: VariableKind::Let,
                    declarations: vec![kiln_hir::VariableDeclarator {
                        pattern: kiln_hir::Pattern::Identifier("answer".into()),
                        initializer: Some(Expression {
                            kind: ExpressionKind::Binary {
                                operator: BinaryOperator::Add,
                                left: Box::new(number(40.0)),
                                right: Box::new(number(2.0)),
                            },
                            span: Span::default(),
                        }),
                        span: Span::default(),
                    }],
                },
                span: Span::default(),
            },
            Statement {
                kind: StatementKind::Expression(Expression {
                    kind: ExpressionKind::Assignment {
                        operator: AssignmentOperator::Add,
                        target: "answer".into(),
                        value: Box::new(number(1.0)),
                    },
                    span: Span::default(),
                }),
                span: Span::default(),
            },
        ];
        let program = compile(&module(body)).expect("valid HIR compiles");
        assert_eq!(program.version, BYTECODE_VERSION);
        assert_eq!(program.local_count, 1);
        assert!(
            program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Binary(BinaryOp::Add)))
        );
        assert!(
            program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::StoreLocal(0)))
        );
    }

    #[test]
    fn emits_short_circuit_jumps_and_host_call() {
        let call = Expression {
            kind: ExpressionKind::Call {
                callee: Box::new(Expression {
                    kind: ExpressionKind::StaticMember {
                        optional: false,
                        object: Box::new(identifier("console")),
                        property: "log".into(),
                    },
                    span: Span::default(),
                }),
                arguments: vec![kiln_hir::ListElement::Item(number(1.0))],
            },
            span: Span::default(),
        };
        let body = vec![Statement {
            kind: StatementKind::Expression(Expression {
                kind: ExpressionKind::Logical {
                    operator: LogicalOperator::And,
                    left: Box::new(Expression {
                        kind: ExpressionKind::Literal(Literal::Boolean(true)),
                        span: Span::default(),
                    }),
                    right: Box::new(call),
                },
                span: Span::default(),
            }),
            span: Span::default(),
        }];
        let program = compile(&module(body)).expect("valid HIR compiles");
        assert!(
            program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::JumpIfFalse(_)))
        );
        assert!(program.code.iter().any(|instruction| matches!(
            instruction,
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1
            }
        )));
    }

    #[test]
    fn rejects_unresolved_and_redeclared_locals() {
        let body = vec![
            Statement {
                kind: StatementKind::Expression(identifier("missing")),
                span: Span::new(2, 9),
            },
            Statement {
                kind: StatementKind::Variable {
                    kind: VariableKind::Let,
                    declarations: vec![kiln_hir::VariableDeclarator {
                        pattern: kiln_hir::Pattern::Identifier("x".into()),
                        initializer: None,
                        span: Span::new(10, 11),
                    }],
                },
                span: Span::default(),
            },
            Statement {
                kind: StatementKind::Variable {
                    kind: VariableKind::Var,
                    declarations: vec![kiln_hir::VariableDeclarator {
                        pattern: kiln_hir::Pattern::Identifier("x".into()),
                        initializer: None,
                        span: Span::new(12, 13),
                    }],
                },
                span: Span::default(),
            },
        ];
        let diagnostics = compile(&module(body)).expect_err("invalid HIR is rejected");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unresolved identifier"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("redeclaration"))
        );
    }

    #[test]
    fn rejects_module_dependencies() {
        let mut module = module(Vec::new());
        module.requested_modules.push(kiln_hir::RequestedModule {
            specifier: "./dep.js".into(),
            occurrences: vec![kiln_hir::ModuleRequestOccurrence {
                statement_span: Span::new(0, 9),
                specifier_span: Span::new(0, 9),
                type_only: false,
                kind: kiln_hir::ModuleRequestKind::Import,
            }],
        });
        let diagnostics = compile(&module).expect_err("dependencies are not executable yet");
        assert!(diagnostics[0].message.contains("module dependency"));
        assert_eq!(diagnostics[0].span, Some(Span::new(0, 9)));
    }

    #[test]
    fn linkage_compilation_maps_named_imports_and_local_exports_to_slots() {
        let mut module = module(vec![Statement {
            kind: StatementKind::Variable {
                kind: VariableKind::Const,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("answer".into()),
                    initializer: Some(Expression {
                        kind: ExpressionKind::Binary {
                            operator: BinaryOperator::Multiply,
                            left: Box::new(identifier("input")),
                            right: Box::new(number(2.0)),
                        },
                        span: Span::default(),
                    }),
                    span: Span::default(),
                }],
            },
            span: Span::default(),
        }]);
        module.requested_modules.push(kiln_hir::RequestedModule {
            specifier: "./dep.js".into(),
            occurrences: vec![kiln_hir::ModuleRequestOccurrence {
                statement_span: Span::new(0, 40),
                specifier_span: Span::new(23, 33),
                type_only: false,
                kind: kiln_hir::ModuleRequestKind::Import,
            }],
        });
        module.imports.push(kiln_hir::Import {
            source: "./dep.js".into(),
            imported: ImportName::Name("value".into()),
            local: "input".into(),
            type_only: false,
            span: Span::new(9, 14),
            statement_span: Span::new(0, 40),
        });
        module.exports.push(kiln_hir::Export {
            exported: Some(ExportName::Name("answer".into())),
            origin: ExportOrigin::Local {
                local: "answer".into(),
            },
            type_only: false,
            span: Span::default(),
            statement_span: Span::default(),
        });

        let linked = compile_module(&module).expect("named scalar linkage compiles");
        assert_eq!(linked.dependencies, ["./dep.js"]);
        assert_eq!(linked.imports[0].imported, "value");
        assert_eq!(linked.imports[0].local_slot, 0);
        assert_eq!(linked.exports[0].exported, "answer");
        assert_eq!(linked.exports[0].local_slot, 1);
        assert!(linked.program.code.contains(&Instruction::LoadLocal(0)));
    }

    #[test]
    fn linkage_compilation_rejects_assignment_to_import() {
        let mut module = module(vec![Statement {
            kind: StatementKind::Expression(Expression {
                kind: ExpressionKind::Assignment {
                    operator: AssignmentOperator::Assign,
                    target: "input".into(),
                    value: Box::new(number(1.0)),
                },
                span: Span::default(),
            }),
            span: Span::default(),
        }]);
        module.imports.push(kiln_hir::Import {
            source: "./dep.js".into(),
            imported: ImportName::Name("value".into()),
            local: "input".into(),
            type_only: false,
            span: Span::default(),
            statement_span: Span::default(),
        });

        let diagnostics = compile_module(&module).expect_err("imports are immutable bindings");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("imported binding"))
        );
    }

    #[test]
    fn an_object_literal_without_a_spread_stays_one_instruction() {
        // The segment machinery only earns its cost when a spread is present; the common literal
        // must not pay for it with a merge per property.
        let program = compile(&module(vec![statement(StatementKind::Expression(
            Expression {
                kind: ExpressionKind::Object {
                    members: vec![
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("a".into()),
                            value: number(1.0),
                            span: Span::default(),
                        }),
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("b".into()),
                            value: number(2.0),
                            span: Span::default(),
                        }),
                    ],
                },
                span: Span::default(),
            },
        ))]))
        .expect("compiles");
        assert_eq!(
            program
                .code
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::NewObject { .. }))
                .count(),
            1
        );
        assert!(!program.code.contains(&Instruction::SpreadObject));
    }

    #[test]
    fn a_spread_splits_the_literal_into_merged_segments() {
        // `{ a: 1, ...source, b: 2 }`: the leading run, the spread, and the trailing run each
        // merge in source order, which is what makes the later property win.
        let program = compile(&module(vec![statement(StatementKind::Expression(
            Expression {
                kind: ExpressionKind::Object {
                    members: vec![
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("a".into()),
                            value: number(1.0),
                            span: Span::default(),
                        }),
                        kiln_hir::ObjectMember::Spread(number(0.0)),
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("b".into()),
                            value: number(2.0),
                            span: Span::default(),
                        }),
                    ],
                },
                span: Span::default(),
            },
        ))]))
        .expect("compiles");
        let objects = program
            .code
            .iter()
            .filter(|instruction| matches!(instruction, Instruction::NewObject { .. }))
            .count();
        let merges = program
            .code
            .iter()
            .filter(|instruction| matches!(instruction, Instruction::SpreadObject))
            .count();
        assert_eq!((objects, merges), (2, 2));
    }

    #[test]
    fn consecutive_spreads_do_not_build_throwaway_objects() {
        // `{ ...a, ...b }` has no properties of its own, so only the accumulator is allocated.
        let program = compile(&module(vec![statement(StatementKind::Expression(
            Expression {
                kind: ExpressionKind::Object {
                    members: vec![
                        kiln_hir::ObjectMember::Spread(number(0.0)),
                        kiln_hir::ObjectMember::Spread(number(1.0)),
                    ],
                },
                span: Span::default(),
            },
        ))]))
        .expect("compiles");
        assert_eq!(
            program
                .code
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::NewObject { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn a_captured_let_in_a_for_head_is_rebound_each_iteration() {
        // `for (let i = 0; ...; i++) fs.push(() => i)` must collect closures over 0, 1, 2 -- not
        // three over the value the loop ended on. The re-binding is a fresh cell per pass, and it
        // has to sit before the update so the update writes the *next* iteration's cell.
        let program = compile(&module(vec![statement(StatementKind::For {
            init: Some(Box::new(statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("i".into()),
                    initializer: Some(number(0.0)),
                    span: Span::default(),
                }],
            }))),
            test: None,
            update: None,
            body: Box::new(statement(StatementKind::Expression(arrow(
                &[],
                vec![statement(StatementKind::Return(Some(Expression {
                    kind: ExpressionKind::Identifier("i".into()),
                    span: Span::default(),
                })))],
            )))),
        })]))
        .expect("compiles");
        assert!(
            program.code.windows(3).any(|window| matches!(
                window,
                [
                    Instruction::LoadCell(a),
                    Instruction::NewCell(b),
                    Instruction::StoreCell(c)
                ] if a == b && b == c
            )),
            "expected a per-iteration re-binding, got {:?}",
            program.code
        );
    }

    #[test]
    fn an_uncaptured_let_in_a_for_head_is_not_rebound() {
        // Nothing can observe the difference without a closure, so nothing should pay for it.
        let program = compile(&module(vec![statement(StatementKind::For {
            init: Some(Box::new(statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("i".into()),
                    initializer: Some(number(0.0)),
                    span: Span::default(),
                }],
            }))),
            test: None,
            update: None,
            body: Box::new(statement(StatementKind::Empty)),
        })]))
        .expect("compiles");
        assert!(
            !program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::NewCell(_))),
            "an unboxed loop variable needs no per-iteration cell"
        );
    }

    #[test]
    fn a_switch_evaluates_its_discriminant_once() {
        // Once, however many arms are compared against it -- so it lands in a hidden binding
        // rather than being re-emitted per test.
        let program = compile(&module(vec![statement(StatementKind::Switch {
            discriminant: number(1.0),
            cases: vec![
                kiln_hir::SwitchCase {
                    test: Some(number(1.0)),
                    body: Vec::new(),
                },
                kiln_hir::SwitchCase {
                    test: Some(number(2.0)),
                    body: Vec::new(),
                },
            ],
        })]))
        .expect("compiles");
        assert_eq!(
            program
                .code
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::StoreLocal(_)))
                .count(),
            1,
            "the discriminant is stored once: {:?}",
            program.code
        );
    }

    #[test]
    fn continue_steps_over_a_switch_to_reach_its_loop() {
        // A `switch` catches `break` and is transparent to `continue`, so the two jumps do not
        // always choose the same enclosing construct.
        let program = compile(&module(vec![statement(StatementKind::While {
            test: number(1.0),
            body: Box::new(statement(StatementKind::Switch {
                discriminant: number(1.0),
                cases: vec![kiln_hir::SwitchCase {
                    test: Some(number(1.0)),
                    body: vec![statement(StatementKind::Continue { label: None })],
                }],
            })),
        })]));
        assert!(
            program.is_ok(),
            "`continue` inside a switch belongs to the loop around it"
        );
    }

    #[test]
    fn a_continue_with_no_loop_at_all_is_still_refused() {
        let messages = errors(vec![statement(StatementKind::Switch {
            discriminant: number(1.0),
            cases: vec![kiln_hir::SwitchCase {
                test: Some(number(1.0)),
                body: vec![statement(StatementKind::Continue { label: None })],
            }],
        })]);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("`continue` outside of a loop")),
            "{messages:?}"
        );
    }

    #[test]
    fn a_template_concatenates_from_a_string() {
        // Starting from a string is what makes every `+` after it an append rather than a
        // numeric add, so `${1}${2}` is "12".
        let program = compile(&module(vec![statement(StatementKind::Expression(
            Expression {
                kind: ExpressionKind::Template {
                    quasis: vec![String::new(), String::new(), String::new()],
                    expressions: vec![number(1.0), number(2.0)],
                },
                span: Span::default(),
            },
        ))]))
        .expect("compiles");
        assert!(matches!(program.code.first(), Some(Instruction::Const(_))));
        assert_eq!(
            program
                .code
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::Binary(BinaryOp::Add)))
                .count(),
            2,
            "one append per hole, and none for the empty text between them"
        );
    }

    #[test]
    fn a_call_without_a_spread_keeps_its_arity_in_the_bytecode() {
        // The fixed-arity path is the common one and must not pay for the general one: no array
        // is built and no `Apply` is emitted.
        let program = compile(&module(vec![
            function("f", &["x"], vec![]),
            statement(StatementKind::Expression(Expression {
                kind: ExpressionKind::Call {
                    callee: Box::new(Expression {
                        kind: ExpressionKind::Identifier("f".into()),
                        span: Span::default(),
                    }),
                    arguments: vec![kiln_hir::ListElement::Item(number(1.0))],
                },
                span: Span::default(),
            })),
        ]))
        .expect("compiles");
        assert!(program.code.contains(&Instruction::Call { argc: 1 }));
        assert!(!program.code.contains(&Instruction::Apply));
    }

    #[test]
    fn a_call_with_a_spread_builds_its_arguments_at_runtime() {
        // A spread's length is not known until it runs, so the arity leaves the bytecode and
        // becomes the length of an array the program computes.
        let program = compile(&module(vec![
            function("f", &["x"], vec![]),
            statement(StatementKind::Expression(Expression {
                kind: ExpressionKind::Call {
                    callee: Box::new(Expression {
                        kind: ExpressionKind::Identifier("f".into()),
                        span: Span::default(),
                    }),
                    arguments: vec![kiln_hir::ListElement::Spread(number(1.0))],
                },
                span: Span::default(),
            })),
        ]))
        .expect("compiles");
        assert!(program.code.contains(&Instruction::Apply));
        assert!(
            !program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Call { .. }))
        );
    }

    #[test]
    fn a_function_declaration_is_emitted_before_the_statements_around_it() {
        // `f(); function f() {}` is ordinary JavaScript, so the value has to exist before the
        // line that writes it.
        let program = compile(&module(vec![
            statement(StatementKind::Expression(Expression {
                kind: ExpressionKind::Call {
                    callee: Box::new(identifier("f")),
                    arguments: Vec::new(),
                },
                span: Span::default(),
            })),
            function("f", &[], vec![]),
        ]))
        .expect("compiles");
        let make = program
            .code
            .iter()
            .position(|instruction| matches!(instruction, Instruction::MakeFunction(_)))
            .expect("the function is built");
        let call = program
            .code
            .iter()
            .position(|instruction| matches!(instruction, Instruction::Call { .. }))
            .expect("the call is emitted");
        assert!(make < call, "the binding is filled before it is read");
    }

    #[test]
    fn a_direct_read_before_a_declaration_is_refused() {
        // Pre-declaring every name in a block would otherwise turn this from a diagnostic into a
        // silent `undefined`, which is the trade the dead-zone flag exists to avoid.
        let messages = errors(vec![
            statement(StatementKind::Expression(identifier("x"))),
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("x".into()),
                    initializer: Some(number(1.0)),
                    span: Span::default(),
                }],
            }),
        ]);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("before its declaration")),
            "{messages:?}"
        );
    }

    #[test]
    fn a_var_read_before_its_declaration_is_undefined_rather_than_refused() {
        // The one kind that is initialised on entry, which is the whole difference.
        let program = compile(&module(vec![
            statement(StatementKind::Expression(identifier("x"))),
            statement(StatementKind::Variable {
                kind: VariableKind::Var,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("x".into()),
                    initializer: Some(number(1.0)),
                    span: Span::default(),
                }],
            }),
        ]));
        assert!(program.is_ok(), "{program:?}");
    }

    #[test]
    fn an_enclosing_functions_binding_shadows_a_module_binding() {
        // Resolution used to go from the inner function straight to module scope, skipping every
        // function in between -- so an inner parameter sharing a name with a module binding read
        // the wrong one. Invisible until a module binding existed before the line declaring it.
        let program = compile(&module(vec![
            statement(StatementKind::Function(Box::new(kiln_hir::Function {
                asynchronous: false,
                name: Some("outer".into()),
                arrow: false,
                generator: false,
                parameters: vec![kiln_hir::Parameter {
                    pattern: kiln_hir::Pattern::Identifier("shared".into()),
                    span: Span::default(),
                }],
                rest: None,
                body: vec![statement(StatementKind::Return(Some(arrow(
                    &[],
                    vec![statement(StatementKind::Return(Some(identifier("shared"))))],
                ))))],
                span: Span::default(),
            }))),
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("shared".into()),
                    initializer: Some(number(1.0)),
                    span: Span::default(),
                }],
            }),
        ]))
        .expect("compiles");
        // The arrow reads the parameter it captured, not the module slot.
        let arrow_body = &program.functions[0].code;
        assert!(
            arrow_body
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadUpvalue(_))),
            "expected a capture, got {arrow_body:?}"
        );
        assert!(
            !arrow_body
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_)))
        );
    }
}
