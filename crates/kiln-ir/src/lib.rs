//! A small, parser-independent stack bytecode for executable Kiln modules.
//!
//! The frontend owns the parser-specific lowering into [`kiln_hir`].  This crate deliberately
//! consumes only that owned HIR and produces plain data, so an interpreter, serializer, or native
//! backend can be added without coupling it to Oxc (or to any other parser).

use std::collections::{HashMap, HashSet};

use kiln_hir::{
    AssignmentOperator, BinaryOperator, CatchClause, Class, ClassField, ExportName, ExportOrigin,
    Expression, ExpressionKind, Function, ImportName, ListElement, Literal, LogicalOperator,
    Module, ObjectKey, ObjectMember, Pattern, PatternKey, Span, Statement, StatementKind,
    SwitchCase, UnaryOperator, VariableKind,
};

/// Host operation identifiers are shared with the runtime through the stable ABI crate.
pub use kiln_host_abi::HostOp;

/// Version of the bytecode contract produced by this crate.
pub const BYTECODE_VERSION: u16 = 2;

/// A constant embedded in a [`Program`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constant {
    Undefined,
    Null,
    Boolean(bool),
    /// IEEE-754 `f64` bits; preserving bits makes program equality total (including NaN/-0).
    Number(u64),
    String(String),
    /// A BigInt literal's decimal digits, with no sign: `-1n` is a negation of `1n`.
    ///
    /// Held as text because the value has no bound. Two programs are equal when their digits
    /// are, which keeps program equality total without a numeric type in the IR.
    BigInt(String),
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
    /// `ToNumeric`: the conversion `x++` performs before it adds anything.
    ///
    /// No source text spells it. It exists because `+x` is *not* it: `+` narrows to a number and
    /// is a TypeError on a BigInt, while an update leaves a BigInt a BigInt. Lowering `x++` to
    /// `+x` then `x + 1` therefore turned every BigInt it touched into a number -- twice over,
    /// since the `1` would have mixed the two types even if the conversion had not.
    ToNumeric,
    /// `ToNumeric(x)` plus one, in the type the conversion produced.
    Increment,
    /// `ToNumeric(x)` minus one, in the type the conversion produced.
    Decrement,
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
    /// `eval(...)` written as that exact name, with nothing shadowing it.
    ///
    /// Its own instruction rather than a call to a value, because the eval'd source is compiled
    /// against the *caller's* scope and no value can carry that: the site's [`EvalSite`] is the
    /// only record of which names are visible here and where they live. Boxed so that the one
    /// instruction that needs a whole scope description does not widen every other one.
    DirectEval(Box<EvalSite>),
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
    /// Computed write carrying the strictness of the body that contains it.
    SetComputedStrict {
        strict: bool,
    },
    /// Push the current `this`.
    LoadThis,
    /// `new.target`. Follows the same rule as `this`: an ordinary call has none, a `new` call
    /// has the constructor it named, and an arrow uses the one from where it was written.
    LoadNewTarget,
    /// Suspend on a promise and resume with what it settles to.
    ///
    /// Distinct from [`Instruction::Yield`], which it is identical to in an async *function*
    /// where every suspension is an await. An async *generator* suspends for two different
    /// reasons -- a value for its consumer, and a promise it is waiting on -- and the driver has
    /// to tell them apart to know whether the consumer's step is finished.
    Await,
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
    /// The call's own arguments from index n onwards, as an array: a rest parameter.
    ///
    /// Distinct from `LoadArguments` because an arrow function has no `arguments` of its own --
    /// it sees the enclosing function's -- but it does have its own rest parameter. Slicing the
    /// `arguments` object would give an arrow the wrong list, which is exactly the bug this
    /// exists to prevent.
    RestArguments(u32),
    /// Set an object's prototype. Stack is object, then prototype; the object is left.
    SetPrototype,
    /// `ToObject`, for the head of a `with`. Stack: `[value] -> [object]`.
    ///
    /// A primitive is boxed and `null` or `undefined` is a TypeError, which is what makes
    /// `with (null) {}` throw before its body runs rather than at the first name inside it.
    ToObject,
    /// Whether the object on top has the named binding, for a `with`. Stack: `[object] -> [bool]`.
    ///
    /// Not `Binary(In)`: an object environment record also consults `Symbol.unscopables`, which
    /// is the mechanism that let `Array.prototype.flat` be added without `with (array) { flat }`
    /// changing meaning in code written before it existed.
    WithHas(u32),
    /// `import(specifier)`. Stack: `[specifier] -> [promise]`.
    ///
    /// The promise is handed back before anything is loaded, and the load happens on the job
    /// queue: `import()` is asynchronous even when the module is already in memory, and a
    /// program that observed it settling synchronously would be seeing this engine rather than
    /// the language.
    DynamicImport,
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
    /// Named write carrying the strictness of the body that contains it.
    SetPropertyStrict {
        name: u32,
        strict: bool,
    },
    /// Copy every own property of the source into the target object, symbol-keyed ones included.
    ///
    /// Stack: `[target, source] -> [target]`, the same shape `SpreadInto` uses for arrays. One
    /// instruction serves both `{ ...source }` and the rest of an object pattern, because both
    /// are "everything this value has, into a fresh object".
    SpreadObject,
    /// Remove an own property, leaving `true` -- which is what the `delete` operator evaluates
    /// to. Stack: `[target] -> [true]`.
    DeleteProperty(u32),
    /// Named delete carrying the strictness of the body that contains it.
    DeletePropertyStrict {
        name: u32,
        strict: bool,
    },
    /// Build a regular expression from its source and flags, both constant strings.
    ///
    /// Built per evaluation rather than interned: each literal produces a fresh object, and the
    /// object carries mutable state -- `lastIndex` -- that two evaluations must not share.
    NewRegExp {
        pattern: u32,
        flags: u32,
    },
    /// Throw a `ReferenceError` naming an identifier that resolved to nothing.
    ///
    /// Emitted rather than refused at compile time, because a reference in a branch that never
    /// runs is not an error in JavaScript, and real bundles are full of them: feature detection
    /// mentions globals the host may not have. The name is kept so the message can say which.
    ThrowReference(u32),
    /// Push the marker that fills a hole in an array literal.
    ///
    /// Not `undefined`: a hole is *absent*, and `in`, `Object.keys` and every array method that
    /// walks elements can tell the two apart.
    PushHole,
    /// `delete target[key]`. Stack: `[target, key] -> [true]`.
    DeleteComputed,
    /// Computed delete carrying the strictness of the body that contains it.
    DeleteComputedStrict {
        strict: bool,
    },
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
    ///
    /// `enumerable` is what separates the two syntaxes that reach here: `{ get x() {} }` is an
    /// ordinary property of an object literal and is enumerated, and `class C { get x() {} }` is
    /// part of the class rather than of any instance and is not.
    DefineAccessor {
        setter: bool,
        enumerable: bool,
    },
    /// Define a method on an object. Stack: `[target, key, function] -> [target]`.
    ///
    /// `SetProperty` with the key on the stack would be the same thing but for the attributes:
    /// an assignment creates an enumerated property, and a class's methods are not enumerated --
    /// `for (const k in new C())` visits fields and nothing else. An assignment cannot express
    /// that, which is why lowering a class body needs an instruction of its own where an object
    /// literal does not.
    DefineMethod,
    /// Put the dead-zone marker in a slot, for a `let` or `const` whose declaration has not run.
    ///
    /// Reading it is a `ReferenceError`. Emitted for lexical bindings as they enter scope, which
    /// matters for captured bindings and for control-flow joins such as switch cases where the
    /// compiler's straight-line initialization state cannot describe every incoming path.
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
    /// `super(...arguments)`. Stack: `[parent, arguments] -> [result]`.
    ///
    /// Separate from `Apply` because `super()` initialises the *existing* instance and keeps the
    /// `new.target` of the call building it, and separate from `SuperCall` because the arity is
    /// the length of an array the program computed rather than a number in the bytecode.
    SuperApply,
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
    /// `async function*`: both of the above at once.
    ///
    /// `generator` is set for an async function too -- its body suspends the same way -- so the
    /// pair cannot say which of the three shapes this is. This is the third bit.
    pub async_generator: bool,
    pub parameter_count: u32,
    /// What `f.length` reports: the parameters before the first one with an initializer, and
    /// before a rest.
    ///
    /// Not the same number as `parameter_count`, which is how many slots the frame has.
    /// `function (a, b = 1, c) {}` has three slots and a `length` of one: the language reports
    /// how many arguments a caller must supply, and everything from the first default onwards is
    /// optional.
    pub declared_length: u32,
    /// Includes the parameters, which occupy the first slots in declaration order.
    pub local_count: u32,
    /// Where parameter binding ends and the body begins: `code[..prologue_end]` is this
    /// function's FunctionDeclarationInstantiation -- cells for captured parameters, pattern
    /// destructuring, defaults, and the rest parameter.
    ///
    /// An ordinary call runs the two halves together and never needs the seam. A generator does:
    /// the language runs instantiation during `[[Call]]` and only *then* builds the generator
    /// object, so `function* g([{x}]) {}` called as `g([null])` throws at the call rather than at
    /// the first `next()`. Recording the offset is what lets the interpreter run the first half
    /// eagerly and start the body at the second.
    pub prologue_end: u32,
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
    EncodeUri,
    EncodeUriComponent,
    DecodeUri,
    DecodeUriComponent,
    ParseFloat,
    IsNaN,
    IsFinite,
    TypeError,
    RangeError,
    ReferenceError,
    SyntaxError,
    UriError,
    SetTimeout,
    SetInterval,
    ClearTimeout,
    ClearInterval,
    QueueMicrotask,
    Promise,
    RegExp,
    Date,
    WeakMap,
    WeakSet,
    Performance,
    Reflect,
    BigInt,
    /// `process`, in the minimal shape a bundle actually reads.
    Process,
    /// Node's global byte container. The interpreter installs the Node-compatible surface only;
    /// native Node execution remains unadvertised until it shares that runtime adapter.
    Buffer,
    /// WHATWG URL, used by both web-compatible programs and Node servers.
    Url,
    /// `Function`, reached for its `prototype` and its `toString` rather than to construct.
    Function,
    Proxy,
    Escape,
    Unescape,
    ArrayBuffer,
    Int8Array,
    Uint8Array,
    Uint8ClampedArray,
    Int16Array,
    Uint16Array,
    Int32Array,
    Uint32Array,
    Float32Array,
    Float64Array,
    /// `eval` reached as a *value* rather than by a direct call: `(0, eval)(src)`,
    /// `globalThis.eval(src)`, or a reference stashed and called later. Every one of those is an
    /// indirect eval, which runs in global scope; a direct call is [`Instruction::DirectEval`]
    /// instead, because only the compiler knows what a caller's scope contains.
    Eval,
    /// Explicit resource management's two names. Appended, like everything after `Json`: the
    /// discriminant is hashed into the artifact cache key, so inserting one in the middle would
    /// invalidate every cached native build.
    DisposableStack,
    SuppressedError,
    /// The asynchronous half of the same pair. Appended for the reason above: the discriminant
    /// is part of the artifact cache key, so a new name goes at the end.
    AsyncDisposableStack,
    /// The error kind nothing throws any more. Appended for the same reason.
    EvalError,
    /// The shared-memory namespace. Appended for the same reason.
    Atomics,
    /// WHATWG UTF-8 decoder. Appended so existing artifact discriminants remain stable.
    TextDecoder,
    /// Node's check-phase task primitives. Appended to preserve existing artifact discriminants.
    SetImmediate,
    ClearImmediate,
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
            "encodeURI" => Some(Self::EncodeUri),
            "encodeURIComponent" => Some(Self::EncodeUriComponent),
            "decodeURI" => Some(Self::DecodeUri),
            "decodeURIComponent" => Some(Self::DecodeUriComponent),
            "parseFloat" => Some(Self::ParseFloat),
            "isNaN" => Some(Self::IsNaN),
            "isFinite" => Some(Self::IsFinite),
            "TypeError" => Some(Self::TypeError),
            "RangeError" => Some(Self::RangeError),
            "ReferenceError" => Some(Self::ReferenceError),
            "SyntaxError" => Some(Self::SyntaxError),
            "URIError" => Some(Self::UriError),
            "setTimeout" => Some(Self::SetTimeout),
            "setInterval" => Some(Self::SetInterval),
            "clearTimeout" => Some(Self::ClearTimeout),
            "clearInterval" => Some(Self::ClearInterval),
            "queueMicrotask" => Some(Self::QueueMicrotask),
            "setImmediate" => Some(Self::SetImmediate),
            "clearImmediate" => Some(Self::ClearImmediate),
            "Promise" => Some(Self::Promise),
            "RegExp" => Some(Self::RegExp),
            "Date" => Some(Self::Date),
            "WeakMap" => Some(Self::WeakMap),
            "WeakSet" => Some(Self::WeakSet),
            "performance" => Some(Self::Performance),
            "Reflect" => Some(Self::Reflect),
            "BigInt" => Some(Self::BigInt),
            "process" => Some(Self::Process),
            "Buffer" => Some(Self::Buffer),
            "URL" => Some(Self::Url),
            // node's alias for the global object. `globalThis` exists to unify it with the
            // browser's `self`, and code written before that still tests for this one.
            "global" => Some(Self::GlobalThis),
            "Function" => Some(Self::Function),
            "Proxy" => Some(Self::Proxy),
            "escape" => Some(Self::Escape),
            "unescape" => Some(Self::Unescape),
            "ArrayBuffer" => Some(Self::ArrayBuffer),
            "Int8Array" => Some(Self::Int8Array),
            "Uint8Array" => Some(Self::Uint8Array),
            "Uint8ClampedArray" => Some(Self::Uint8ClampedArray),
            "Int16Array" => Some(Self::Int16Array),
            "Uint16Array" => Some(Self::Uint16Array),
            "Int32Array" => Some(Self::Int32Array),
            "Uint32Array" => Some(Self::Uint32Array),
            "Float32Array" => Some(Self::Float32Array),
            "Float64Array" => Some(Self::Float64Array),
            "eval" => Some(Self::Eval),
            "DisposableStack" => Some(Self::DisposableStack),
            "SuppressedError" => Some(Self::SuppressedError),
            "AsyncDisposableStack" => Some(Self::AsyncDisposableStack),
            "EvalError" => Some(Self::EvalError),
            "Atomics" => Some(Self::Atomics),
            "TextDecoder" => Some(Self::TextDecoder),

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
            Self::EncodeUri => "encodeURI",
            Self::EncodeUriComponent => "encodeURIComponent",
            Self::DecodeUri => "decodeURI",
            Self::DecodeUriComponent => "decodeURIComponent",
            Self::ParseFloat => "parseFloat",
            Self::IsNaN => "isNaN",
            Self::IsFinite => "isFinite",
            Self::TypeError => "TypeError",
            Self::RangeError => "RangeError",
            Self::ReferenceError => "ReferenceError",
            Self::SyntaxError => "SyntaxError",
            Self::UriError => "URIError",
            Self::SetTimeout => "setTimeout",
            Self::SetInterval => "setInterval",
            Self::ClearTimeout => "clearTimeout",
            Self::ClearInterval => "clearInterval",
            Self::QueueMicrotask => "queueMicrotask",
            Self::SetImmediate => "setImmediate",
            Self::ClearImmediate => "clearImmediate",
            Self::Promise => "Promise",
            Self::RegExp => "RegExp",
            Self::Date => "Date",
            Self::WeakMap => "WeakMap",
            Self::WeakSet => "WeakSet",
            Self::Performance => "performance",
            Self::Reflect => "Reflect",
            Self::BigInt => "BigInt",
            Self::Process => "process",
            Self::Buffer => "Buffer",
            Self::Url => "URL",
            Self::Function => "Function",
            Self::Proxy => "Proxy",
            Self::Escape => "escape",
            Self::Unescape => "unescape",
            Self::ArrayBuffer => "ArrayBuffer",
            Self::Int8Array => "Int8Array",
            Self::Uint8Array => "Uint8Array",
            Self::Uint8ClampedArray => "Uint8ClampedArray",
            Self::Int16Array => "Int16Array",
            Self::Uint16Array => "Uint16Array",
            Self::Int32Array => "Int32Array",
            Self::Uint32Array => "Uint32Array",
            Self::Float32Array => "Float32Array",
            Self::Float64Array => "Float64Array",
            Self::Eval => "eval",
            Self::DisposableStack => "DisposableStack",
            Self::SuppressedError => "SuppressedError",
            Self::AsyncDisposableStack => "AsyncDisposableStack",
            Self::EvalError => "EvalError",
            Self::Atomics => "Atomics",
            Self::TextDecoder => "TextDecoder",
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

/// Where a binding visible at a direct `eval` call site lives in the running frame.
///
/// Always a cell, never a raw slot: the eval'd source may write the binding, and the caller has
/// to see that write. A cell is the only storage two independently compiled programs can share,
/// which is why a function containing a direct eval boxes every binding in scope
/// ([`Compiler::cell_everything`]) rather than only the ones a nested function names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalAccess {
    /// A cell in the calling frame's own local slot.
    Cell(u32),
    /// A cell the calling closure had already captured.
    Upvalue(u32),
}

/// How the caller declared a binding a direct `eval` can see.
///
/// Not just "may it be written": `var x` inside a sloppy eval *is* the caller's binding when the
/// caller declared a `var`, and is a `SyntaxError` when the caller declared a `let`. One flag
/// cannot answer both questions, and answering the second wrong turns a redeclaration the
/// language allows into a refusal.
///
/// A kind of its own rather than `kiln_hir::VariableKind`, because this crosses into the
/// interpreter and ADR 0007 keeps HIR out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalDeclaration {
    Var,
    Let,
    Const,
}

/// One name a direct `eval` can see, and how the machine reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalBinding {
    pub name: String,
    pub access: EvalAccess,
    pub declaration: EvalDeclaration,
}

/// Everything a direct `eval(...)` call needs that only the compiler knows.
///
/// A direct eval evaluates in its *caller's* scope, and the caller's names are gone by run time:
/// the compiler resolved them to slot numbers. So the call site carries them. Held inline in the
/// instruction rather than in a side table on [`Program`], because a direct eval is rare and a
/// table would cost every program a field it never uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalSite {
    /// The names in scope where the call is written, innermost binding first so a shadowed outer
    /// name never wins.
    pub bindings: Vec<EvalBinding>,
    /// Whether the calling code is strict, which decides whether the eval'd source gets its own
    /// scope for `var` declarations or shares the caller's bindings.
    pub strict: bool,
    /// Arguments on the stack above the call, or `None` when a spread put them in one array.
    pub argc: Option<u16>,
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

/// The `imported` name an `import * as ns` binding carries.
///
/// A sentinel rather than a flag on [`ImportBinding`]: the linker already looks a name up in the
/// provider's exports, and a name no export can have keeps that one lookup doing all the work.
pub const NAMESPACE_IMPORT: &str = "*namespace*";

/// The prefix of the export name a `export * from "./m"` compiles to, followed by the specifier.
///
/// A module's export table is fixed when it compiles and the provider's is not, so `export *`
/// cannot be listed as names here. It is recorded as one export whose name says where to look,
/// and the linker -- which does have both tables -- expands it. The prefix carries characters no
/// exported name can, so it can never be mistaken for one.
pub const STAR_REEXPORT_PREFIX: &str = "*star*";

/// The prefix of the local name `export { x } from "./m"` binds its borrowed value to.
///
/// A re-export is desugared into an import the module never names plus an export of what that
/// import brought in, and the import needs a local to land in. The name carries characters no
/// identifier can, so no source can read the slot -- which is what lets a linker resolving the
/// export through to its origin leave the slot itself unfilled.
pub const REEXPORT_LOCAL_PREFIX: &str = "*reexport";

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

/// What a pattern walk does when it reaches a name.
///
/// Taking a value apart is the same work whether the names are being declared or assigned; only
/// this differs, so it is a parameter rather than a second copy of the walk.
#[derive(Debug, Clone, Copy)]
enum PatternLeaf {
    Declare(VariableKind),
    Assign,
}

/// Where a pattern walk reads the value it is taking apart.
///
/// Straight off the stack whenever nothing else has to push there, which keeps `{ a, b } = o`
/// free of a temporary. A target that stores through a property reference, and a computed key,
/// both evaluate expressions *between* the source arriving and the value being read out of it;
/// those pushes would bury the source, so it spills into a hidden binding instead.
enum PatternSource {
    Stack,
    Hidden(String),
}

/// A pattern property's key, once it has been evaluated.
///
/// A computed key is held in a hidden binding rather than kept as an expression, because a rest
/// has to exclude the key that was actually produced -- re-running the expression would evaluate
/// it twice and could produce a different key the second time.
enum SourceKey {
    Static(u32),
    Computed(String),
}

/// A source-located compiler failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileDiagnostic {
    pub message: String,
    pub span: Option<Span>,
}

/// What the caller knows about a compilation that the HIR does not say.
///
/// Strictness is the whole of it today. A directive prologue is not an HIR statement -- the
/// frontend drops it, as Oxc keeps directives beside the body rather than in it -- so the only
/// place that knows a Script was run under `"use strict"` is whoever assembled the source.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompileOptions {
    pub strict: bool,
}

/// Which compiled bodies run as strict code.
///
/// Beside the [`Program`] rather than inside it, because it answers a question only the
/// interpreter asks -- whether a refused write throws or is discarded -- while a `Program` is
/// bytecode two backends read. `functions[i]` describes `Program::functions[i]`, in the same
/// order, and is the *effective* strictness: a body inside strict code is strict whether or not
/// it said so.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Strictness {
    /// The top-level body: the module body, the script, or the eval program.
    pub body: bool,
    pub functions: Vec<bool>,
}

impl Strictness {
    /// Nothing is strict -- the answer for a caller that compiled without asking.
    #[must_use]
    pub fn sloppy(program: &Program) -> Self {
        Self {
            body: false,
            functions: vec![false; program.functions.len()],
        }
    }

    /// Everything is strict, which is what a Module goal means: strict whatever its text says,
    /// and so is every function written in it.
    #[must_use]
    pub fn always(program: &Program) -> Self {
        Self {
            body: true,
            functions: vec![true; program.functions.len()],
        }
    }

    /// Whether `Program::functions[index]` is strict. Out of range answers sloppy rather than
    /// panicking: a function value that crossed a module boundary is indexed against its own
    /// program, and a mismatch is a bug to survive rather than to abort a program over.
    #[must_use]
    pub fn function(&self, index: u32) -> bool {
        self.functions.get(index as usize).copied().unwrap_or(false)
    }
}

/// Compile owned HIR into stack bytecode.
pub fn compile(module: &Module) -> Result<Program, Vec<CompileDiagnostic>> {
    compile_with_options(module, CompileOptions::default())
}

/// Compile owned HIR into stack bytecode, saying what the source text alone cannot.
pub fn compile_with_options(
    module: &Module,
    options: CompileOptions,
) -> Result<Program, Vec<CompileDiagnostic>> {
    Ok(compile_with_strictness(module, options)?.0)
}

/// [`compile_with_options`], also reporting which bodies came out strict.
///
/// A second entry point rather than a wider return type, because only the interpreter needs the
/// answer and every other caller of `compile` would have to thread a value it never reads.
pub fn compile_with_strictness(
    module: &Module,
    options: CompileOptions,
) -> Result<(Program, Strictness), Vec<CompileDiagnostic>> {
    let mut compiler = Compiler::new(module);
    // Either the caller knows -- a Test262 strict variant, a `Function` body -- or the source
    // opened with the directive itself.
    compiler.strict = options.strict || module.strict;
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
    compiler.finish_with_strictness()
}

/// One binding an eval program borrows from the frame that called it.
///
/// The cell itself travels at run time; only its name and whether it may be written are the
/// compiler's business. Order matters: the *i*th entry binds slot *i* of the produced program,
/// which is the whole contract between [`compile_eval`] and the machine that installs the cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalOuter {
    pub name: String,
    pub declaration: EvalDeclaration,
}

/// What a run-time `eval` compilation needs beyond the source it was handed.
#[derive(Debug, Clone, Default)]
pub struct EvalOptions {
    /// The calling frame's visible bindings, innermost first. Empty for an indirect eval, which
    /// sees global scope and nothing else.
    pub outer: Vec<EvalOuter>,
    /// Strict either because the caller is, or because the eval'd source says so itself.
    pub strict: bool,
}

/// Compile source that arrived at run time, as `eval` and the `Function` constructor do.
///
/// Two things separate this from [`compile`]. The program *completes with a value* -- `eval("1")`
/// is 1 -- where a module body always completes with `undefined`. And its scope 0 may already
/// hold bindings: a direct eval reads and writes the caller's, through cells the machine installs
/// in slots 0..`outer.len()` before the first instruction runs.
pub fn compile_eval(
    module: &Module,
    options: &EvalOptions,
) -> Result<Program, Vec<CompileDiagnostic>> {
    Ok(compile_eval_with_strictness(module, options)?.0)
}

/// [`compile_eval`], also reporting which bodies came out strict.
pub fn compile_eval_with_strictness(
    module: &Module,
    options: &EvalOptions,
) -> Result<(Program, Strictness), Vec<CompileDiagnostic>> {
    let mut compiler = Compiler::new(module);
    compiler.strict = options.strict || module.strict;
    compiler.eval_program = true;
    // Unconditional, unlike an ordinary program, and it is what makes the eval'd code reachable
    // from itself. A module-scope binding that is *not* boxed is read from a nested function
    // through `LoadGlobal`, which finds the frame of whichever module is running -- and for an
    // eval that is the caller's module, holding entirely unrelated slots. Boxing every top-level
    // binding means every such read is a capture instead, and `LoadGlobal` is never emitted.
    compiler.cell_everything = true;
    for outer in &options.outer {
        let slot = compiler.local_count;
        compiler.local_count += 1;
        compiler.outer_names.insert(outer.name.clone());
        compiler.scopes[0].insert(
            outer.name.clone(),
            Local {
                slot,
                kind: match outer.declaration {
                    EvalDeclaration::Var => VariableKind::Var,
                    EvalDeclaration::Let => VariableKind::Let,
                    EvalDeclaration::Const => VariableKind::Const,
                },
                imported: false,
                initialized: true,
                // The caller's cell is installed in the slot before the body runs, so there is
                // nothing for a prologue to box and every access goes straight through it.
                celled: true,
            },
        );
    }
    compiler.compile_eval_body(module);
    compiler.finish_with_strictness()
}

/// Compile one module while preserving the scalar linkage needed by a graph linker.
///
/// Named and default value imports are supported. Namespace imports, star/namespace re-exports,
/// indirect re-exports, default expression exports, and cycles remain explicit unsupported forms.
pub fn compile_module(module: &Module) -> Result<ModuleProgram, Vec<CompileDiagnostic>> {
    let mut compiler = Compiler::new(module);
    // A module is strict whatever its text says, so nothing has to pass it in.
    compiler.strict = true;
    compiler.module_body = true;
    let mut imports = Vec::new();

    for import in module.imports.iter().filter(|import| !import.type_only) {
        let imported = match &import.imported {
            ImportName::Name(name) => name.clone(),
            ImportName::Default => "default".to_owned(),
            // `import * as ns` asks for every export at once. The name carries characters an
            // identifier cannot, so it can never collide with a real export.
            ImportName::Namespace => NAMESPACE_IMPORT.to_owned(),
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

    // `export { x } from "./m"` is an import the module never names plus an export of what it
    // brought in. Desugaring it here rather than teaching the linker a third kind of export
    // means the import path and the local-export path each stay the only one of their kind.
    //
    // The value is read once, when the provider finishes, which is where every other export is
    // read too. A genuinely live re-export -- one that shows a later reassignment in the
    // provider -- needs exported cells rather than exported values, and is a separate change.
    let mut reexport_slots: HashMap<usize, u32> = HashMap::new();
    let mut star_exports: Vec<ExportBinding> = Vec::new();
    for (index, export) in module.exports.iter().enumerate() {
        if export.type_only {
            continue;
        }
        let (source, imported) = match &export.origin {
            ExportOrigin::ReExport { source, imported } => (
                source,
                match imported {
                    ImportName::Name(name) => name.clone(),
                    ImportName::Default => "default".to_owned(),
                    ImportName::Namespace => NAMESPACE_IMPORT.to_owned(),
                },
            ),
            ExportOrigin::NamespaceReExport { source } | ExportOrigin::Star { source } => {
                (source, NAMESPACE_IMPORT.to_owned())
            }
            _ => continue,
        };
        // A name no program can write, and one per re-export, so two of them from the same
        // module do not share a slot.
        let local = format!("{REEXPORT_LOCAL_PREFIX}{index}*");
        let Some(local_slot) = compiler.declare_import(&local, export.span) else {
            continue;
        };
        // `export *` names nothing, so the main loop below -- which needs an exported name --
        // never sees it. Its export entry is made here instead, naming the source.
        if let ExportOrigin::Star { source } = &export.origin {
            star_exports.push(ExportBinding {
                exported: format!("{STAR_REEXPORT_PREFIX}{source}"),
                local_slot,
            });
        } else {
            reexport_slots.insert(index, local_slot);
        }
        imports.push(ImportBinding {
            source: source.clone(),
            imported,
            local,
            local_slot,
        });
    }

    // An exported binding that can *change* is boxed, so the importer holds this module's cell
    // rather than a copy of the value it had when the module finished. One that cannot change --
    // a `const`, a function declaration, a class -- is left as a plain slot: a snapshot and a
    // live view of it are indistinguishable, and boxing it would cost the native backend, which
    // has no cells, a module it can otherwise compile.
    let mutable = mutable_module_bindings(&module.body);
    for export in &module.exports {
        if let ExportOrigin::Local { local } = &export.origin
            && mutable.contains(local)
        {
            compiler.captured.insert(local.clone());
        }
    }
    compiler.compile_body(module);

    let mut exports = star_exports;
    for (index, export) in module
        .exports
        .iter()
        .enumerate()
        .filter(|(_, export)| !export.type_only)
    {
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
            ExportOrigin::DefaultExpression => {
                match compiler.resolve_module_local(kiln_hir::DEFAULT_EXPORT_LOCAL) {
                    Some(local_slot) => exports.push(ExportBinding {
                        exported: exported.to_owned(),
                        local_slot,
                    }),
                    None => {
                        compiler.error(Some(export.span), "`export default` has no value to export")
                    }
                }
            }
            ExportOrigin::ReExport { .. } | ExportOrigin::NamespaceReExport { .. } => {
                if let Some(local_slot) = reexport_slots.get(&index) {
                    exports.push(ExportBinding {
                        exported: exported.to_owned(),
                        local_slot: *local_slot,
                    });
                }
            }
            // Handled above: a star export has no exported name, so control never reaches here.
            ExportOrigin::Star { .. } => {}
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

/// One parameter's share of `FunctionDeclarationInstantiation`, in declaration order.
///
/// The list is built while the parameters are being declared and emitted afterwards, because
/// the two need different information and the order between them is the same: left to right,
/// which is the order a default expression can observe.
enum ParameterStep<'a> {
    /// A pattern, destructured out of its positional slot.
    Destructure(u32, &'a Pattern, Span),
    /// An identifier with a default, assigned from its positional slot.
    Defaulted(u32, bool, &'a Pattern, Span, &'a str),
    /// A plain identifier: the call already put it in its slot, and nothing needs emitting.
    Plain(&'a str),
}

/// Compile a CommonJS body with Node's wrapper bindings installed as module-scope cells.
///
/// The graph supplies the five values in this exact order. They are ordinary mutable bindings:
/// `module.exports = value` must be observable after evaluation, and nested functions capture
/// `require`/`exports` through the same cell mechanism as any other module-scope name.
pub fn compile_commonjs(module: &Module) -> Result<ModuleProgram, Vec<CompileDiagnostic>> {
    const WRAPPER: [&str; 5] = ["exports", "require", "module", "__filename", "__dirname"];
    let mut compiler = Compiler::new(module);
    // A CommonJS module's values routinely leave their defining module through `module.exports`
    // and are invoked by importers. Module-slot instructions are caller-relative in the VM, so
    // no CommonJS function may depend on one: its defining bindings have to travel as cells.
    // Boxing the whole top level makes that invariant structural rather than dependent on a
    // syntax-shape capture scan keeping pace with every form emitted by npm packages (notably
    // destructured parameter defaults in path-to-regexp).
    compiler.cell_everything = true;
    let mut imports = Vec::with_capacity(WRAPPER.len());
    for name in WRAPPER {
        let slot = compiler.local_count;
        compiler.local_count += 1;
        compiler.outer_names.insert(name.to_owned());
        compiler.scopes[0].insert(
            name.to_owned(),
            Local {
                slot,
                // Node supplies these as function parameters. A `var` declaration of a
                // parameter is legal and reuses that binding (`var module = ...` appears in
                // real packages); modelling them as lexical `let`s rejects valid CommonJS.
                kind: VariableKind::Var,
                imported: false,
                initialized: true,
                celled: true,
            },
        );
        imports.push(ImportBinding {
            source: "*commonjs-wrapper*".into(),
            imported: name.into(),
            local: name.into(),
            local_slot: slot,
        });
    }
    compiler.compile_body(module);
    compiler.finish().map(|program| ModuleProgram {
        program,
        dependencies: Vec::new(),
        imports,
        exports: Vec::new(),
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
    /// The `using` declarations of one scope, disposed in reverse declaration order.
    ///
    /// Held as bindings rather than as a runtime list because which declarations a scope
    /// contains is a fact about its text: the *number* of resources is static even though
    /// whether each one was reached is not, and a pair of hidden bindings per declaration says
    /// both at once -- an empty method binding is a declaration this pass never got to.
    DisposeResources(Vec<ResourceSlot>),
}

/// Where one `using` declaration keeps what has to be disposed of, and how.
#[derive(Clone)]
struct ResourceSlot {
    /// The resource itself, which is the receiver its disposal is called against.
    value: String,
    /// The method, read once where the declaration is. Reading it again at the end of the scope
    /// would run an accessor twice and could find a different function the second time.
    method: String,
    /// `await using`: the disposal is awaited.
    awaiting: bool,
}

/// The `using` declarations of the scope being compiled, and how many have been reached.
struct ResourceScope {
    slots: Vec<ResourceSlot>,
    /// Declarations are handed their slots in source order, which is the order the disposal
    /// walks backwards.
    next: usize,
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
    /// `const` written further down. The runtime marker catches a direct read before the
    /// declaration; a nested function resolves the same name as an upvalue and checks the shared
    /// cell when it eventually runs.
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
    /// Whether every binding of this function is boxed, because a direct `eval` can see them.
    cell_everything: bool,
}

/// Where a name lives, which decides how it is read and whether it can be read at all.
/// One active `with`, and where it sits so a binding written inside it can shadow it.
#[derive(Clone, Debug)]
struct WithScope {
    /// The hidden binding holding the object. It is celled, because a function created inside
    /// the body still resolves its names against the object when it is called later.
    name: String,
    /// How many functions deep the `with` itself is, so a nested function can tell that its own
    /// parameters and locals are inside the object environment rather than outside it.
    functions: usize,
    /// How many scopes were open when the body started. A binding declared at or beyond this is
    /// nested *within* the object environment record and therefore shadows it.
    scopes: usize,
}

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
    /// Whether `functions[i]` was compiled as strict code, in the same order.
    ///
    /// Beside the protos rather than on them, because a proto is bytecode the C backend reads
    /// too and this is the interpreter's answer to one question: whether a refused write throws
    /// or is discarded. See [`Strictness`].
    strict_functions: Vec<bool>,
    /// Enclosing function states, module body first. Empty while compiling the module body.
    enclosing: Vec<FunctionState>,
    upvalues: Vec<Upvalue>,
    upvalue_names: HashMap<String, u32>,
    captured: HashSet<String>,
    /// Whether the code being compiled is strict.
    ///
    /// Recorded rather than derived, because a directive prologue does not survive lowering: the
    /// goal and the harness variant are what say so, and they are the caller's to pass in. Only
    /// `eval` reads it -- a direct eval in strict code gets its own scope for its declarations,
    /// and in sloppy code shares the caller's.
    strict: bool,
    /// Whether this compilation is an `eval` program rather than a file.
    ///
    /// The one thing it changes is the temporal dead zone. Both files and eval code preserve a
    /// reachable dead-zone read for runtime, because the language promises a `ReferenceError` --
    /// `assert.throws(ReferenceError, () => eval("typeof x; let x;"))` -- rather than rejecting
    /// source that may never reach the read. Every eval binding is boxed, so the runtime check is
    /// already there to raise it.
    eval_program: bool,
    /// Whether this compilation is a *module* body rather than a script or an `eval`.
    ///
    /// Only `await` reads it. A module body may await, and doing so makes its evaluation
    /// asynchronous -- the body stops and the host resumes it; a script has nowhere to put that
    /// suspension, and `await` there is a plain identifier or a syntax error depending on the
    /// context, never a suspension.
    module_body: bool,
    /// Whether the code being compiled is one parameter's default, with later parameters still
    /// unbound.
    ///
    /// The parameter list is its own scope, and every name in it exists -- uninitialised --
    /// from the moment the call begins. So `function f(x = x) {}` compiles and only *calling*
    /// it fails, and the failure is a `ReferenceError`, not a rejection of the source. The flag
    /// is what tells a dead-zone read here apart from one in a body, where the same runtime check
    /// must remain reachable so a surrounding `try` can catch it.
    parameter_dead_zone: bool,
    /// Names of the calling frame's bindings borrowed into scope 0 of an eval program.
    ///
    /// Tracked apart from the scope itself because a declaration in the eval'd source has to
    /// tell a borrowed binding from one of its own: a sloppy `var` takes the caller's over, and
    /// everything else shadows it.
    outer_names: HashSet<String>,
    /// Box every binding of the function being compiled, not only the ones a nested function
    /// names.
    ///
    /// Set when a direct `eval` appears anywhere inside this function, including inside a
    /// function nested in it. The eval'd source is compiled separately and reaches the caller's
    /// bindings through shared cells, so a binding it might name has to *be* a cell -- and which
    /// names it will use is unknowable until it runs. Over-approximating costs one pointer chase
    /// per access in a function that already contains an eval, and buys the guarantee that every
    /// name at the call site can be handed over.
    cell_everything: bool,
    /// Loops currently being compiled, innermost last, holding the jumps their `break` and
    /// `continue` statements still need patched.
    loops: Vec<LoopContext>,
    /// Counter for compiler-introduced bindings, so two in one scope never collide.
    hidden_bindings: usize,
    /// Whether the function being compiled is an async generator.
    ///
    /// `yield*` is the only thing that reads it, and it needs it because the two generator kinds
    /// delegate to *different protocols*: an async `yield*` asks for `@@asyncIterator` and awaits
    /// each step, and reaching for `@@iterator` instead is observable -- a program that defines
    /// both sees the wrong one run, and one that defines only a throwing `@@iterator` getter sees
    /// an exception the specification says can never happen.
    async_generator: bool,
    /// The scopes containing `using` declarations that are being compiled, innermost last.
    ///
    /// A stack because a block with resources can be written inside another, and each
    /// declaration belongs to the scope it was written in rather than to the innermost one open
    /// at the time it runs.
    resource_scopes: Vec<ResourceScope>,
    /// The `with` statements whose bodies are being compiled, outermost first.
    ///
    /// Every name mentioned inside one is looked for on these objects before the bindings around
    /// it, and only code lexically inside a `with` pays for that: the resolution everywhere else
    /// is the static one it has always been.
    with_scopes: Vec<WithScope>,
    /// `finally` blocks enclosing the code being compiled, innermost last. A non-local exit has
    /// to run each of them on its way out, so it needs their bodies rather than their addresses.
    finalizers: Vec<Cleanup>,
    /// The class whose private names are in scope, innermost last.
    ///
    /// A stack because a class can be declared inside another's method, and each has its own
    /// private names.
    private_scopes: Vec<u32>,
    /// The next unused class identity for `private_scopes`.
    next_private_scope: u32,
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
    fn set_property_instruction(&self, name: u32) -> Instruction {
        Instruction::SetPropertyStrict {
            name,
            strict: self.strict,
        }
    }

    fn set_computed_instruction(&self) -> Instruction {
        Instruction::SetComputedStrict {
            strict: self.strict,
        }
    }

    fn delete_property_instruction(&self, name: u32) -> Instruction {
        Instruction::DeletePropertyStrict {
            name,
            strict: self.strict,
        }
    }

    fn delete_computed_instruction(&self) -> Instruction {
        Instruction::DeleteComputedStrict {
            strict: self.strict,
        }
    }

    fn new(module: &Module) -> Self {
        let mut compiler = Self {
            constants: Vec::new(),
            code: Vec::new(),
            functions: Vec::new(),
            strict_functions: Vec::new(),
            enclosing: Vec::new(),
            upvalues: Vec::new(),
            upvalue_names: HashMap::new(),
            // The module body has nested functions of its own, so its block bindings can be
            // captured. Computed here rather than in each entry point, because there are two --
            // `compile` and `compile_module` -- and one of them would have been forgotten.
            captured: names_captured_by_nested_functions(&module.body),
            strict: false,
            eval_program: false,
            module_body: false,
            parameter_dead_zone: false,
            outer_names: HashSet::new(),
            // Module scope is boxed too when the module contains a direct eval: a script's
            // top-level bindings are exactly the ones an eval written there can see.
            cell_everything: body_contains_direct_eval(&module.body),
            loops: Vec::new(),
            hidden_bindings: 0,
            resource_scopes: Vec::new(),
            async_generator: false,
            with_scopes: Vec::new(),
            private_scopes: Vec::new(),
            next_private_scope: 0,
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
        self.plain_statements(&module.body);
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::Return);
    }

    /// The body of an eval program, which returns its completion value.
    ///
    /// `eval("1 + 1")` is 2, and a module body is not: it always completes with `undefined`, so
    /// the ordinary path throws every statement's value away. The completion value is the last
    /// *expression* statement's, held in a slot of its own so that a statement which produces
    /// nothing -- a declaration, a loop -- leaves the previous one standing, which is what the
    /// specification's completion records do.
    ///
    /// Only the top level is tracked. A value produced inside an `if` or a block is a completion
    /// value too, and reporting `undefined` for one is a gap rather than a wrong answer: nothing
    /// else can claim to be the result.
    fn compile_eval_body(&mut self, module: &Module) {
        let completion = self.local_count;
        self.local_count += 1;
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::StoreLocal(completion));
        // The same three passes `statements` makes -- names, then hoisted function values, then
        // the rest -- with the expression statements diverted into the completion slot.
        for statement in &module.body {
            self.hoist_names(statement);
        }
        for statement in &module.body {
            if let StatementKind::Function(function) = &statement.kind {
                self.emit_function_declaration(function, statement.span);
            }
        }
        for statement in &module.body {
            match &statement.kind {
                StatementKind::Function(_) => {}
                StatementKind::Expression(expression) => {
                    self.expression(expression);
                    self.code.push(Instruction::StoreLocal(completion));
                }
                _ => self.statement(statement),
            }
        }
        self.code.push(Instruction::LoadLocal(completion));
        self.code.push(Instruction::Return);
    }

    fn finish(self) -> Result<Program, Vec<CompileDiagnostic>> {
        Ok(self.finish_with_strictness()?.0)
    }

    fn finish_with_strictness(self) -> Result<(Program, Strictness), Vec<CompileDiagnostic>> {
        if self.diagnostics.is_empty() {
            Ok((
                Program {
                    version: BYTECODE_VERSION,
                    constants: self.constants,
                    code: self.code,
                    local_count: self.local_count,
                    functions: self.functions,
                },
                Strictness {
                    body: self.strict,
                    functions: self.strict_functions,
                },
            ))
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
                // *Always* a cell. An import is a live view of the exporter's binding, not a
                // copy of its value at the moment the exporter finished -- `export let count`
                // that a function later increments has to read as the new number here. The cell
                // is the exporter's own, installed by the host before the first instruction, so
                // there is nothing for the module prologue to box.
                celled: true,
            },
        );
        Some(slot)
    }

    /// The reserved key a private field is stored under, in the class being compiled.
    ///
    /// Scoped to the class rather than to the name: `class A { #v } class B { #v }` declare two
    /// different fields, and `a.#v` on a `B` is a TypeError rather than a read of B's. One key
    /// per name would have made those the same slot, which is a wrong answer -- and the one
    /// `#v in b` would have reported as a match.
    fn private_key(&self, name: &str) -> String {
        private_key(self.private_scopes.last().copied().unwrap_or(0), name)
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
                        Some(name) => {
                            self.variable(
                                *kind,
                                name,
                                declaration.initializer.as_ref(),
                                declaration.span,
                            );
                            // `using` binds exactly as `const` does; the registration is the
                            // whole of what it adds, and it runs after the binding so that a
                            // disposal method reached through the binding sees it initialised.
                            if let Some(awaiting) = kind.disposal() {
                                self.register_resource(name, awaiting, declaration.span);
                            }
                        }
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
            StatementKind::With { object, body } => {
                self.with_statement(object, body, statement.span);
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
                pattern,
                iterable,
                body,
                awaiting,
            } => self.for_of(*kind, pattern, iterable, body, *awaiting, statement.span),
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
            // `export default <expression>` has no name in the source, so it binds the reserved
            // one the module record already uses for an anonymous default. Everything after it
            // -- the export table, the linker -- then sees an ordinary local export.
            StatementKind::ExportDefault(expression) => self.variable(
                VariableKind::Const,
                kiln_hir::DEFAULT_EXPORT_LOCAL,
                Some(expression),
                statement.span,
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
        self.destructure(PatternLeaf::Declare(kind), pattern, span);
    }

    /// Assign `pattern` from the value on top of the stack, consuming it.
    ///
    /// Every rule about how a pattern takes a value apart -- defaults applying only to
    /// `undefined`, a hole skipping an index, a rest copying what the named keys did not take --
    /// is the same whether the names are being declared or assigned. Only the leaf differs.
    fn assign_pattern(&mut self, pattern: &Pattern, span: Span) {
        self.destructure(PatternLeaf::Assign, pattern, span);
    }

    /// Take the value on top of the stack apart, consuming it.
    ///
    /// A leaf that stores through a property reference expects the receiver -- and the key, for a
    /// computed one -- to be sitting under that value already: [`Compiler::destructure_element`]
    /// puts them there before the value exists, because that is the order the language evaluates
    /// them in. Only that method reaches such a leaf, which is why nothing here re-derives them.
    /// `RequireObjectCoercible` on the value an object pattern is about to take apart.
    ///
    /// Part of the pattern rather than a side effect of reading it: `var {} = null` throws even
    /// though the pattern names nothing to read. A pattern *with* properties only appeared to
    /// obey the rule, because reading its first key off `null` threw for an unrelated reason --
    /// which left `var {} = null`, `({} = null)` and `var { ...rest } = null` accepting a value
    /// no object pattern may take.
    fn require_object_coercible(&mut self) {
        self.code.push(Instruction::Dup);
        let coercible = self.emit_jump(Instruction::JumpIfNotNullish(0));
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(
            "Cannot destructure 'null' or 'undefined'".to_owned(),
        ));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
        self.patch_jump(coercible);
    }

    fn destructure(&mut self, leaf: PatternLeaf, pattern: &Pattern, span: Span) {
        match pattern {
            Pattern::Identifier(name) => {
                if name.is_empty() {
                    // The frontend already reported why; binding an impossible name here would
                    // add a second, more confusing diagnostic.
                    self.code.push(Instruction::Pop);
                    return;
                }
                match leaf {
                    PatternLeaf::Declare(kind) => self.declare_bound(kind, name, span),
                    PatternLeaf::Assign => self.store_existing(name, span),
                }
            }
            Pattern::Member { property, .. } => {
                let name = self.constant_index(Constant::String(property.clone()));
                self.store_through_reference(leaf, self.set_property_instruction(name), span);
            }
            Pattern::Computed { .. } => {
                self.store_through_reference(leaf, self.set_computed_instruction(), span);
            }
            Pattern::PrivateMember { name, .. } => {
                let name = self.constant_index(Constant::String(self.private_key(name)));
                self.store_through_reference(leaf, Instruction::SetPrivate(name), span);
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
                self.destructure(leaf, inner, span);
            }
            Pattern::Object { properties, rest } => {
                self.require_object_coercible();
                let spilled = rest.is_some()
                    || properties.iter().any(|property| {
                        stores_through_reference(&property.value)
                            || matches!(property.key, PatternKey::Computed(_))
                    });
                let source = self.pattern_source(spilled, span);

                let mut taken = Vec::with_capacity(properties.len());
                for property in properties {
                    // A computed key is evaluated where the source wrote it: before the target's
                    // reference and before the read, and exactly once even when a rest below
                    // needs the key again.
                    let key = match &property.key {
                        PatternKey::Static(name) => {
                            SourceKey::Static(self.constant_index(Constant::String(name.clone())))
                        }
                        PatternKey::Computed(expression) => {
                            self.expression(expression);
                            let held = format!("%key{}", self.hidden_bindings);
                            self.hidden_bindings += 1;
                            self.declare_hidden(&held, span);
                            SourceKey::Computed(held)
                        }
                    };
                    self.destructure_element(leaf, &property.value, span, |compiler| {
                        compiler.load_pattern_source(&source);
                        compiler.read_key(&key);
                    });
                    taken.push(key);
                }

                // Copy everything, then remove what the named properties already took. The other
                // direction -- copy all but a set of names -- would need the exclusion list at
                // runtime; this needs only instructions that already earn their keep.
                if let Some(rest) = rest {
                    self.destructure_element(leaf, rest, span, |compiler| {
                        compiler.code.push(Instruction::NewObject { count: 0 });
                        compiler.load_pattern_source(&source);
                        compiler.code.push(Instruction::SpreadObject);
                        for key in &taken {
                            compiler.code.push(Instruction::Dup);
                            match key {
                                SourceKey::Static(index) => {
                                    compiler
                                        .code
                                        .push(compiler.delete_property_instruction(*index));
                                }
                                SourceKey::Computed(held) => {
                                    compiler.load_hidden(held);
                                    compiler.code.push(compiler.delete_computed_instruction());
                                }
                            }
                            compiler.code.push(Instruction::Pop);
                        }
                    });
                }
                self.finish_pattern_source(&source);
            }
            Pattern::Array { elements, rest } => {
                self.destructure_array(leaf, elements, rest.as_deref(), span);
            }
        }
    }

    /// Take an array pattern apart by stepping its iterator, one element at a time.
    ///
    /// The shorter lowering -- materialise the iterable into an array and read it by index --
    /// gets three separate things wrong, and each of them is something a program can observe:
    ///
    /// * `const [a] = naturals()` must take exactly one step. Draining first never returns.
    /// * An iterator the pattern did not exhaust must be *closed*, by calling its `return`. A
    ///   generator on the other side of one runs its `finally` blocks because of that call.
    /// * How many times `next` ran is visible to whoever wrote the iterator, so `[] = iter` --
    ///   which takes nothing -- must not step it at all.
    ///
    /// So the protocol is walked here, in the same shape `for-of` walks it and for the same
    /// reason: no single instruction can stop part-way through an iterable.
    ///
    /// `done` tracks the specification's `iteratorRecord.[[Done]]`, and the flag is what decides
    /// whether the close happens. It is set *before* each `next` and cleared only once a value
    /// is in hand, because an iterator that threw is finished rather than merely unfinished --
    /// closing one that has already failed would call `return` on it, which the language does
    /// not do.
    fn destructure_array(
        &mut self,
        leaf: PatternLeaf,
        elements: &[Option<Pattern>],
        rest: Option<&Pattern>,
        span: Span,
    ) {
        let index = self.hidden_bindings;
        self.hidden_bindings += 1;
        let iterator = format!("%iterated{index}");
        let done = format!("%drained{index}");
        let step = format!("%stepped{index}");
        let next_method = format!("%nextof{index}");

        // `value[Symbol.iterator]()`, which is what makes `const [a] = new Set([1])` bind 1: a
        // pattern reads what the value yields, not what its indices hold.
        let symbol = self.constant_index(Constant::String("@@iterator".to_owned()));
        self.code.push(Instruction::CallMethod {
            name: symbol,
            argc: 0,
        });
        self.declare_hidden(&iterator, span);
        // The rest of the iterator record. `GetIteratorFromMethod` reads `next` once, before any
        // element is taken, and every step calls what it captured -- which is why `const [] = it`
        // reads `next` even though it steps nothing, and why a three-element pattern reads it
        // once rather than three times.
        let next = self.constant_index(Constant::String("next".to_owned()));
        self.load_hidden(&iterator);
        self.code.push(Instruction::GetProperty(next));
        self.declare_hidden(&next_method, span);
        self.push_constant(Constant::Boolean(false));
        self.declare_hidden(&done, span);
        // The slot the step results land in, bound once here so each element can store into it.
        self.push_constant(Constant::Undefined);
        self.declare_hidden(&step, span);

        // Everything from here to the close is guarded: an exception out of a default, a nested
        // pattern or a target's own evaluation has to close the iterator on its way past.
        let handler_jump = self.emit_jump(Instruction::PushHandler(0));

        for element in elements {
            match element {
                // A hole steps the iterator and reads nothing -- not even `value`, which is why
                // it is not the same as binding a name no one looks at.
                None => self.emit_iterator_step(&iterator, &next_method, &done, &step, false),
                Some(element) => {
                    self.destructure_element(leaf, element, span, |compiler| {
                        compiler.emit_iterator_step(&iterator, &next_method, &done, &step, true);
                    });
                }
            }
        }
        if let Some(rest) = rest {
            self.destructure_element(leaf, rest, span, |compiler| {
                compiler.emit_iterator_drain(&iterator, &next_method, &done, &step, span);
            });
        }

        self.code.push(Instruction::PopHandler);
        // A pattern that consumed every value has an iterator that closed itself; calling
        // `return` on an exhausted iterator is a step the language does not take.
        self.load_hidden(&done);
        let closed = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.emit_iterator_close_checked(&iterator, span);
        self.patch_jump(closed);
        let finished = self.emit_jump(Instruction::Jump(0));

        // The landing pad. Entering it has already popped the handler and put the thrown value
        // on the stack. The close here is the forgiving flavour: an exception already on its way
        // out is not replaced by whatever the cleanup thought of the iterator.
        self.patch_jump(handler_jump);
        self.load_hidden(&done);
        let raise = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.emit_iterator_close_quietly(&iterator, span);
        self.patch_jump(raise);
        self.code.push(Instruction::Throw);

        self.patch_jump(finished);
    }

    /// One `IteratorStep`, leaving the value it produced when `want_value` is set.
    ///
    /// An exhausted iterator yields `undefined` rather than stopping the walk: `var [a, b] = [1]`
    /// binds `b` to undefined, and a default on it applies.
    fn emit_iterator_step(
        &mut self,
        iterator: &str,
        next_method: &str,
        done: &str,
        step: &str,
        want_value: bool,
    ) {
        self.load_hidden(done);
        let exhausted = self.emit_jump(Instruction::JumpIfTruthy(0));
        // Marked done *before* the call, and cleared only after a value is in hand: a `next` or
        // a `value` getter that throws leaves the iterator failed, and a failed one is never
        // closed.
        self.push_constant(Constant::Boolean(true));
        self.store_hidden(done);
        self.load_hidden(iterator);
        self.load_hidden(next_method);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        self.store_hidden(step);
        self.load_hidden(step);
        let finished_key = self.constant_index(Constant::String("done".to_owned()));
        self.code.push(Instruction::GetProperty(finished_key));
        let finished = self.emit_jump(Instruction::JumpIfTruthy(0));
        if want_value {
            self.load_hidden(step);
            let value_key = self.constant_index(Constant::String("value".to_owned()));
            self.code.push(Instruction::GetProperty(value_key));
        }
        self.push_constant(Constant::Boolean(false));
        self.store_hidden(done);
        let ready = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(exhausted);
        self.patch_jump(finished);
        if want_value {
            self.push_constant(Constant::Undefined);
        }
        self.patch_jump(ready);
    }

    /// `...rest`: everything the iterator has left, as a fresh array.
    ///
    /// Written with an index and `SetComputed` rather than with `push`, because
    /// `Array.prototype.push` is a property a program can replace and the language defines this
    /// as a plain data property on a fresh array.
    fn emit_iterator_drain(
        &mut self,
        iterator: &str,
        next_method: &str,
        done: &str,
        step: &str,
        span: Span,
    ) {
        let index = self.hidden_bindings;
        self.hidden_bindings += 1;
        let collected = format!("%collected{index}");
        let count = format!("%counted{index}");
        self.code.push(Instruction::NewArray { count: 0 });
        self.declare_hidden(&collected, span);
        self.push_constant(Constant::Number(0.0f64.to_bits()));
        self.declare_hidden(&count, span);

        let start = self.code.len();
        self.load_hidden(done);
        let drained = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.push_constant(Constant::Boolean(true));
        self.store_hidden(done);
        self.load_hidden(iterator);
        self.load_hidden(next_method);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        self.store_hidden(step);
        self.load_hidden(step);
        let finished_key = self.constant_index(Constant::String("done".to_owned()));
        self.code.push(Instruction::GetProperty(finished_key));
        let ended = self.emit_jump(Instruction::JumpIfTruthy(0));

        self.load_hidden(&collected);
        self.load_hidden(&count);
        self.load_hidden(step);
        let value_key = self.constant_index(Constant::String("value".to_owned()));
        self.code.push(Instruction::GetProperty(value_key));
        // The value survived, so the iterator is unfinished again.
        self.push_constant(Constant::Boolean(false));
        self.store_hidden(done);
        self.code.push(self.set_computed_instruction());
        self.code.push(Instruction::Pop);
        self.load_hidden(&count);
        self.push_constant(Constant::Number(1.0f64.to_bits()));
        self.code.push(Instruction::Binary(BinaryOp::Add));
        self.store_hidden(&count);
        self.code.push(Instruction::Jump(start as u32));

        self.patch_jump(drained);
        self.patch_jump(ended);
        self.load_hidden(&collected);
    }

    /// `IteratorClose` on a normal completion, which is the strict flavour of it.
    ///
    /// A `return` that throws propagates, and one that answers a non-object is a `TypeError`:
    /// nothing is on its way out yet, so this failure is the program's outcome. The pad that
    /// catches an exception uses [`Compiler::emit_iterator_close`] instead, which discards both.
    fn emit_iterator_close_checked(&mut self, binding: &str, span: Span) {
        let absent = self.emit_iterator_return(binding, span);
        self.require_object("Iterator result of `return` is not an object");
        self.code.push(Instruction::Pop);
        self.patch_jump(absent);
    }

    /// `IteratorClose` on an abrupt completion, which is the forgiving flavour of it.
    ///
    /// The iterator is given its chance to clean up and anything it has to say about that is
    /// dropped -- its result unread, and an exception out of its `return` swallowed. A value is
    /// already travelling outwards, and the language keeps the one the program threw rather than
    /// replacing it with whatever the cleanup thought.
    fn emit_iterator_close_quietly(&mut self, binding: &str, span: Span) {
        let guard = self.emit_jump(Instruction::PushHandler(0));
        let absent = self.emit_iterator_return(binding, span);
        self.code.push(Instruction::Pop);
        self.patch_jump(absent);
        self.code.push(Instruction::PopHandler);
        let closed = self.emit_jump(Instruction::Jump(0));
        // Entering here truncated the stack back to where the guard went up, so the only thing
        // above the exception being carried out is the one the `return` raised.
        self.patch_jump(guard);
        self.code.push(Instruction::Pop);
        self.patch_jump(closed);
    }

    /// Call the iterator's `return`, leaving its result on the stack.
    ///
    /// Returns the jump taken when there is no `return` to call, which the caller patches to
    /// wherever it lands after dealing with the result: an absent `return` leaves nothing.
    ///
    /// Applied rather than called by name, because the language reads `return` once and a second
    /// read would run an accessor twice. The empty argument array is also what `return` sees as
    /// its own `arguments`, whose length one test asserts.
    fn emit_iterator_return(&mut self, binding: &str, span: Span) -> usize {
        let name = self.constant_index(Constant::String("return".to_owned()));
        let held = format!("%closing{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.load_hidden(binding);
        self.code.push(Instruction::GetProperty(name));
        self.declare_hidden(&held, span);
        self.load_hidden(&held);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        let absent = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(present);
        self.load_hidden(binding);
        self.load_hidden(&held);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        absent
    }

    /// Throw a `TypeError` unless the value on top of the stack is an object, leaving it either
    /// way. `typeof` rather than a test of its own, because the two answers that mean "object"
    /// are exactly `"object"` -- excluding `null`, which shares the answer -- and `"function"`.
    fn require_object(&mut self, message: &str) {
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("object".to_owned()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let nullable = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("function".to_owned()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let callable = self.emit_jump(Instruction::JumpIfTruthy(0));
        let refused = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(nullable);
        self.code.push(Instruction::Dup);
        let object = self.emit_jump(Instruction::JumpIfNotNullish(0));

        self.patch_jump(refused);
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(message.to_owned()));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);

        self.patch_jump(callable);
        self.patch_jump(object);
    }

    /// Take one element or property apart, given how to push the value it receives.
    ///
    /// The value is emitted by a callback rather than passed in already on the stack, because a
    /// target that stores through a reference has to be evaluated *before* it: `({ a: o.p } = s)`
    /// evaluates `o` and only then runs the getter for `a`, and which of the two throws first is
    /// observable. A nested pattern is not a reference, so `[{ a: o.p }] = xs` evaluates `o`
    /// while taking the inner object apart rather than before reading the element.
    fn destructure_element(
        &mut self,
        leaf: PatternLeaf,
        target: &Pattern,
        span: Span,
        value: impl FnOnce(&mut Self),
    ) {
        // In declaration position a reference target is refused rather than evaluated, so
        // nothing is pushed here that the leaf below would not know to pop.
        if matches!(leaf, PatternLeaf::Assign) {
            self.reference_prologue(target);
        }
        value(self);
        self.destructure(leaf, target, span);
    }

    /// Evaluate what a target stores through, looking past any default it carries.
    fn reference_prologue(&mut self, target: &Pattern) {
        match target {
            Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
                self.expression(object);
            }
            Pattern::Computed { object, property } => {
                self.expression(object);
                self.expression(property);
            }
            // `[o.p = 1] = xs` evaluates `o` before the element, not after the default decides
            // whether one is needed: the default is part of the value, not part of the target.
            Pattern::WithDefault { inner, .. } => self.reference_prologue(inner),
            _ => {}
        }
    }

    /// Finish a leaf that stores through a reference, with the value on top of the stack and the
    /// reference under it.
    fn store_through_reference(&mut self, leaf: PatternLeaf, store: Instruction, span: Span) {
        match leaf {
            PatternLeaf::Assign => {
                self.code.push(store);
                // The store leaves the value, which is what an assignment expression evaluates
                // to and what a pattern walk has no further use for.
                self.code.push(Instruction::Pop);
            }
            // `let [o.p] = xs` is a syntax error: a declaration introduces names, and a property
            // reference is not one. Nothing was pushed for the reference, so only the value goes.
            PatternLeaf::Declare(_) => {
                self.error(
                    Some(span),
                    "a declaration cannot bind through a property reference",
                );
                self.code.push(Instruction::Pop);
            }
        }
    }

    /// Move the value being taken apart off the stack when the walk needs the room.
    ///
    /// The temporary deliberately goes in the scope that is current, rather than in one pushed
    /// around the walk: the names the pattern binds are declared as it runs, and a scope of the
    /// walk's own would swallow them when it was popped.
    fn pattern_source(&mut self, spilled: bool, span: Span) -> PatternSource {
        if !spilled {
            return PatternSource::Stack;
        }
        let name = format!("%source{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.declare_hidden(&name, span);
        PatternSource::Hidden(name)
    }

    fn load_pattern_source(&mut self, source: &PatternSource) {
        match source {
            PatternSource::Stack => self.code.push(Instruction::Dup),
            PatternSource::Hidden(name) => self.load_hidden(name),
        }
    }

    /// Read one property of the source, which is already on top of the stack.
    fn read_key(&mut self, key: &SourceKey) {
        match key {
            SourceKey::Static(index) => self.code.push(Instruction::GetProperty(*index)),
            SourceKey::Computed(held) => {
                self.load_hidden(held);
                self.code.push(Instruction::GetComputed);
            }
        }
    }

    /// Drop the source now the walk is done with it. A spilled one is already off the stack.
    fn finish_pattern_source(&mut self, source: &PatternSource) {
        if matches!(source, PatternSource::Stack) {
            self.code.push(Instruction::Pop);
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
            if bound_from_stack {
                self.initialize(name);
                return;
            }
            // The initializer is compiled *before* the name leaves its dead zone, because it is
            // inside that zone: `let x = x` is a run-time `ReferenceError`, not a read of an
            // undefined slot and not a refusal of the source.
            if let Some(initializer) = initializer {
                self.expression(initializer);
            } else if kind != VariableKind::Var {
                // A `let` with no initializer is undefined; a `var` keeps what it already had,
                // which is what makes `var x = 1; var x;` leave 1 in place.
                self.push_constant(Constant::Undefined);
            } else {
                self.initialize(name);
                return;
            }
            self.initialize(name);
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
        // A captured binding is a cell at every scope, including module scope. `LoadGlobal`
        // reaches the currently executing module frame, which is correct only while a callable
        // stays in its declaring module. Exported functions and classes travel; their cells travel
        // with them. `predeclare` applies the same rule to hoisted bindings, and keeping this
        // non-hoisted path weaker lets hidden superclass bindings index the importer by slot.
        let celled = self.cell_everything || self.captured.contains(name);
        // A `let` or `const` whose initializer has not run yet is in its dead zone, and the
        // initializer is part of that zone. A `var` never is, and a binding the caller is about
        // to store into from the stack has no initializer of its own to wait for.
        let in_dead_zone = kind != VariableKind::Var && !bound_from_stack;
        self.scopes[scope_index].insert(
            name.to_owned(),
            Local {
                slot,
                kind,
                imported: false,
                initialized: !in_dead_zone,
                celled,
            },
        );
        if celled {
            self.code.push(Instruction::NewCell(slot));
            // A captured binding can be read from a function called before the declaration runs,
            // where the compiler cannot see the read. The marker is what makes that a
            // `ReferenceError` rather than an undefined the program never wrote. Uncaptured
            // bindings use a scratch marker below when their initializer reads them directly.
            if in_dead_zone {
                self.code.push(Instruction::MarkUninitialized(slot));
            }
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
        self.initialize(name);
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
        let resources = resource_declarations(list);
        if resources.is_empty() {
            self.plain_statements(list);
            return;
        }
        self.resource_scope(list, &resources);
    }

    /// Compile a statement list whose `using` declarations have to be disposed of when control
    /// leaves it, whichever way it leaves.
    ///
    /// The shape is `try`/`finally` with no `catch`, and for the same reason: a disposal has to
    /// run on every way out, and re-emitting it at each exit is what keeps `break`, `continue`
    /// and `return` ordinary jumps. It goes on the same cleanup stack a `finally` uses, so a
    /// non-local exit crossing both runs them in the right order without either knowing about
    /// the other.
    fn resource_scope(&mut self, list: &[Statement], resources: &[bool]) {
        let span = list
            .first()
            .map_or(Span::new(0, 0), |statement| statement.span);
        self.with_resource_scope(resources, span, |compiler| compiler.plain_statements(list));
    }

    /// Compile `body` with `resources` disposed of on every way out of it.
    fn with_resource_scope(
        &mut self,
        resources: &[bool],
        span: Span,
        body: impl FnOnce(&mut Self),
    ) {
        let mut slots = Vec::with_capacity(resources.len());
        for awaiting in resources {
            let index = self.hidden_bindings;
            self.hidden_bindings += 1;
            let slot = ResourceSlot {
                value: format!("%resource{index}"),
                method: format!("%dispose{index}"),
                awaiting: *awaiting,
            };
            // Cleared on entry rather than only written on use, because a loop body is one scope
            // entered many times: without this, a pass that never reached a declaration would
            // dispose of what the previous pass left in the slot.
            self.push_constant(Constant::Undefined);
            self.declare_hidden(&slot.value, span);
            self.push_constant(Constant::Undefined);
            self.declare_hidden(&slot.method, span);
            slots.push(slot);
        }
        let cleanup = Cleanup::DisposeResources(slots.clone());
        self.resource_scopes.push(ResourceScope { slots, next: 0 });
        self.finalizers.push(cleanup.clone());

        let handler_jump = self.emit_jump(Instruction::PushHandler(0));
        self.handler_depth += 1;
        body(self);
        self.code.push(Instruction::PopHandler);
        self.handler_depth -= 1;

        self.finalizers.pop();
        self.emit_finalizer(&cleanup);
        let done = self.emit_jump(Instruction::Jump(0));

        // The exception is still on the stack for `Throw` to take; the disposals below it push
        // and pop in balance, so it is still on top when they are done.
        self.patch_jump(handler_jump);
        self.emit_finalizer(&cleanup);
        self.code.push(Instruction::Throw);
        self.patch_jump(done);
        self.resource_scopes
            .pop()
            .expect("this scope pushed its own resource state");
    }

    fn plain_statements(&mut self, list: &[Statement]) {
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

    /// Read `name` from the bindings around it, ignoring any `with` object.
    ///
    /// The whole of Kiln's ordinary identifier resolution: a slot, a cell, an upvalue, a module
    /// slot, or -- when nothing bound it -- a built-in or a `ReferenceError` where the read is.
    fn load_lexically(&mut self, name: &str) {
        match self.resolve_or_capture(name) {
            // Reading a binding whose declaration has not run yet is the temporal dead zone.
            // Keep the read in the instruction stream: if control never reaches it, compilation
            // succeeds, and if it does, the marker produces the language's `ReferenceError`.
            // A nested function reading the same name resolves as an upvalue and checks the
            // shared cell when it runs.
            Resolution::Local(local) if !local.initialized && local.celled && self.eval_program => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            // Any read of a binding whose declaration has not run yet: `x; let x = 1`,
            // `let x = x`, or a parameter read from another parameter's default in
            // `function f(x = y, y) {}`. The check has to be a *run-time* `ReferenceError`,
            // because the source is well formed and only reaching the read fails -- exactly what
            // `assert.throws(ReferenceError, () => f())` asks for. A cell that is uninitialised by
            // construction is that check, spelled with instructions that already exist rather
            // than with an opcode of its own; a scratch one rather than the binding's own,
            // because a plain local has no cell at this point in the code and an uncelled slot is
            // not one to load from.
            Resolution::Local(local) if !local.initialized => {
                let _ = local;
                let scratch = self.local_count;
                self.local_count += 1;
                self.code.push(Instruction::NewCell(scratch));
                self.code.push(Instruction::MarkUninitialized(scratch));
                self.code.push(Instruction::LoadCell(scratch));
            }
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
                // A `ReferenceError` where the read happens, which is what JavaScript does
                // and what lets a module compile when a branch it never takes mentions a
                // global this realm does not provide. See ADR 0017.
                None => {
                    let name = self.constant_index(Constant::String(name.to_owned()));
                    self.code.push(Instruction::ThrowReference(name));
                }
            },
        }
    }

    /// `with (object) body`: an object environment record around the body's scope.
    ///
    /// The object goes into a hidden binding rather than staying on the stack, because the body
    /// may create a function that outlives the statement -- and that function's names still
    /// resolve against the object when it is called. A binding is the only thing a closure can
    /// capture, so the object becomes one, and it is boxed like any other captured binding.
    fn with_statement(&mut self, object: &Expression, body: &Statement, span: Span) {
        // Each level costs one lookup chain at every name inside it, and an assignment repeats
        // its value expression once per level. Four is far past anything written on purpose, and
        // refusing beyond it is a limit that says so rather than emitting code nobody can read.
        if self.with_scopes.len() >= 4 {
            self.error(
                Some(span),
                "unsupported statement: `with` nested more than four deep",
            );
            return;
        }
        let name = format!("%with{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());
        self.captured.insert(name.clone());
        self.expression(object);
        self.code.push(Instruction::ToObject);
        self.declare_bound(VariableKind::Let, &name, span);
        self.with_scopes.push(WithScope {
            name,
            functions: self.enclosing.len(),
            scopes: self.scopes.len(),
        });
        self.statement(body);
        self.with_scopes.pop().expect("just pushed");
        self.scopes.pop().expect("with scope was just pushed");
    }

    /// The `with` objects `name` has to be looked for on, innermost first.
    ///
    /// Empty for every name in a program with no `with` in it, and for a name whose binding is
    /// written *inside* the body: `with (o) { let x = 1; x }` is the `let`, whatever `o` holds,
    /// because the block's scope is nested within the object environment record.
    fn with_scopes_for(&self, name: &str) -> Vec<WithScope> {
        if self.with_scopes.is_empty() {
            return Vec::new();
        }
        // `this` is not a binding an environment record answers for, and a compiler-introduced
        // name is not one any object can have: neither is ever looked for on a `with` object.
        if name == "this" || name.starts_with('%') || name.starts_with('*') {
            return Vec::new();
        }
        let bound = self
            .scopes
            .iter()
            .rposition(|scope| scope.contains_key(name));
        let functions = self.enclosing.len();
        self.with_scopes
            .iter()
            .rev()
            .filter(|with| {
                if with.functions == functions {
                    bound.is_none_or(|at| at < with.scopes)
                } else {
                    // The `with` is in an enclosing function, so every binding this function
                    // declares -- a parameter, a local -- is written inside it and shadows it.
                    bound.is_none()
                }
            })
            .cloned()
            .collect()
    }

    /// Push the object of one active `with`.
    fn load_with_object(&mut self, with: &WithScope) {
        match self.resolve_or_capture(&with.name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            Resolution::Unresolved => {
                self.error(None, "a `with` object went out of scope");
                self.push_constant(Constant::Undefined);
            }
        }
    }

    /// Read `name`, looking on each active `with` object before the bindings around it.
    ///
    /// `fallback` emits the read for the case no object has it -- the ordinary lexical one, or
    /// the `"undefined"` that a `typeof` of an unbound name produces.
    fn with_scoped_load(&mut self, name: &str, fallback: impl FnOnce(&mut Self)) {
        let scopes = self.with_scopes_for(name);
        if scopes.is_empty() {
            fallback(self);
            return;
        }
        let key = self.constant_index(Constant::String(name.to_owned()));
        let mut found = Vec::with_capacity(scopes.len());
        for with in &scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_with_object(with);
            self.code.push(Instruction::GetProperty(key));
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        fallback(self);
        for jump in found {
            self.patch_jump(jump);
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
        // An outer binding is the *caller's*, borrowed into scope 0 so a direct eval can read and
        // write it. A declaration in the eval'd source either takes it over or shadows it, and
        // which of the two is the whole difference strictness makes to an eval: a sloppy `var` or
        // function declaration belongs to the caller's variable environment and so becomes that
        // binding, while a `let`, a `const`, a class, and every declaration in strict code belong
        // to the eval's own scope and get a slot of their own.
        if scope_index == 0 && self.outer_names.remove(name) {
            if initialized && !self.strict {
                // `var x;` over an existing binding keeps its value -- which is why this returns
                // rather than falling through to the `undefined` store below.
                return;
            }
            self.scopes[0].remove(name);
        }
        if self.scopes[scope_index].contains_key(name) {
            return;
        }
        let slot = self.local_count;
        self.local_count += 1;
        // Module scope boxes a captured binding exactly like any other scope. It used to be
        // exempt, on the reasoning that a function reaches a module binding through the module
        // frame -- true within one module, and wrong across two: the frame a `LoadGlobal` finds
        // belongs to whichever module is *running*, not to the one the function was written in.
        // An exported function reading its own module's `const` read the importer's slot of the
        // same number instead, which is a wrong answer rather than a missing one.
        let captured = self.captured.contains(name) || self.cell_everything;
        let celled = captured;
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
        } else {
            // Keep the run-time state on every lexical slot, not only captured ones. A switch
            // shares one scope between all of its cases, so compiler-time `initialized` state
            // can be true after compiling one case even though another case jumps straight to a
            // read before the declaration. The marker lets the machine preserve the TDZ across
            // that control-flow join; captured cells need it for the same reason when a closure
            // is called too early.
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
            Cleanup::DisposeResources(slots) => self.emit_disposals(&slots.clone()),
        }
    }

    /// Record the value a `using` declaration just bound, so the end of the scope disposes of it.
    ///
    /// The method is read here rather than at the disposal, which is what the specification's
    /// `GetDisposeMethod` says and is observable twice over: an accessor runs once, and a value
    /// with no disposal method is refused where it was declared instead of much later, when the
    /// program has already used it and cannot do anything about it.
    fn register_resource(&mut self, name: &str, awaiting: bool, span: Span) {
        let Some(scope) = self.resource_scopes.last_mut() else {
            self.error(Some(span), "a `using` declaration outside any scope");
            return;
        };
        let Some(slot) = scope.slots.get(scope.next).cloned() else {
            self.error(
                Some(span),
                "more `using` declarations than this scope counted",
            );
            return;
        };
        scope.next += 1;
        debug_assert_eq!(slot.awaiting, awaiting);

        self.load_lexically(name);
        self.code.push(Instruction::Dup);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        // `using x = null` binds and disposes of nothing, and never looks for a method: a
        // missing resource is the one case the language lets pass.
        self.code.push(Instruction::Pop);
        let done = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(present);
        self.store_hidden(&slot.value);
        let dispose = self.constant_index(Constant::String("@@dispose".to_owned()));
        if awaiting {
            let asynchronous = self.constant_index(Constant::String("@@asyncDispose".to_owned()));
            self.load_hidden(&slot.value);
            self.code.push(Instruction::GetProperty(asynchronous));
            self.code.push(Instruction::Dup);
            let found = self.emit_jump(Instruction::JumpIfNotNullish(0));
            // `await using` falls back to the synchronous method, whose result is awaited like
            // any other -- which is what lets one object serve both forms.
            self.code.push(Instruction::Pop);
            self.load_hidden(&slot.value);
            self.code.push(Instruction::GetProperty(dispose));
            self.patch_jump(found);
        } else {
            self.load_hidden(&slot.value);
            self.code.push(Instruction::GetProperty(dispose));
        }

        self.code.push(Instruction::Dup);
        let callable = self.emit_jump(Instruction::JumpIfNotNullish(0));
        self.code.push(Instruction::Pop);
        self.emit_type_error(if awaiting {
            "the value is not asynchronously disposable"
        } else {
            "the value is not disposable"
        });
        self.patch_jump(callable);
        self.store_hidden(&slot.method);
        self.patch_jump(done);
    }

    /// Throw a `TypeError` the compiler decided on, built from the realm's own constructor.
    ///
    /// Through `LoadBuiltin` rather than by naming `TypeError`, because a program is free to
    /// shadow that name and the error a language rule raises is not the shadowing program's to
    /// choose.
    fn emit_type_error(&mut self, message: &str) {
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(message.to_owned()));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
    }

    /// Dispose of a scope's `using` resources, in reverse declaration order.
    ///
    /// A slot whose method binding is still empty belongs to a declaration this exit never
    /// reached -- an early `return` above it, or a throw from its own initialiser -- and there is
    /// nothing to dispose of.
    fn emit_disposals(&mut self, slots: &[ResourceSlot]) {
        for slot in slots.iter().rev() {
            self.load_hidden(&slot.method);
            let skip = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_hidden(&slot.value);
            self.load_hidden(&slot.method);
            self.code.push(Instruction::NewArray { count: 0 });
            self.code.push(Instruction::Apply);
            if slot.awaiting {
                self.code.push(Instruction::Await);
            }
            // The specification discards what a disposal returns; only whether it threw matters.
            self.code.push(Instruction::Pop);
            self.patch_jump(skip);
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
        pattern: &Pattern,
        iterable: &Expression,
        body: &Statement,
        awaiting: bool,
        span: Span,
    ) {
        if awaiting && !self.compiling_function() && !self.module_body {
            // `for await` at top level is top-level `await` under another name, and a script is
            // not an evaluation anything can resume.
            self.error(
                Some(span),
                "top-level `for await` is only available in a module",
            );
            return;
        }
        let iterator_binding = format!("%iter{}", self.hidden_bindings);
        let result_binding = format!("%step{}", self.hidden_bindings);
        let asynchronous_binding = format!("%async{}", self.hidden_bindings);
        let next_binding = format!("%next{}", self.hidden_bindings);
        let method_binding = format!("%method{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());

        let symbol = self.constant_index(Constant::String("@@iterator".to_owned()));
        if awaiting {
            // `for await` prefers the async iterator and falls back to the sync one, and the two
            // are awaited in different places: an async iterator's `next` returns a promise of
            // the result, while a sync one returns the result directly and it is each *value*
            // that may be a promise. Which was found is remembered rather than re-derived.
            let subject = format!("%iterable{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.expression(iterable);
            self.declare_hidden(&subject, span);

            // `GetMethod(obj, @@asyncIterator)` is *one* read, and the method it answers with is
            // the one that gets called. Reading the symbol to decide and then reading it again to
            // call it asked an accessor twice and let a program hand back two different methods,
            // one of which was never invoked. It also missed `null`, which `GetMethod` treats as
            // absent exactly as it treats `undefined`.
            let asynchronous = self.constant_index(Constant::String("@@asyncIterator".to_owned()));
            self.load_hidden(&subject);
            self.code.push(Instruction::GetProperty(asynchronous));
            self.declare_hidden(&method_binding, span);

            self.load_hidden(&method_binding);
            let use_async = self.emit_jump(Instruction::JumpIfNotNullish(0));

            self.load_hidden(&subject);
            self.code.push(Instruction::CallMethod {
                name: symbol,
                argc: 0,
            });
            self.push_constant(Constant::Boolean(false));
            let chosen = self.emit_jump(Instruction::Jump(0));

            self.patch_jump(use_async);
            self.load_hidden(&subject);
            self.load_hidden(&method_binding);
            self.code.push(Instruction::NewArray { count: 0 });
            self.code.push(Instruction::Apply);
            self.push_constant(Constant::Boolean(true));

            self.patch_jump(chosen);
            self.declare_hidden(&asynchronous_binding, span);
            self.declare_hidden(&iterator_binding, span);
        } else {
            // `iterable[Symbol.iterator]()`, which is what makes any iterable work rather than
            // only an array.
            self.expression(iterable);
            self.code.push(Instruction::CallMethod {
                name: symbol,
                argc: 0,
            });
            self.declare_hidden(&iterator_binding, span);
        }

        // `GetIteratorFromMethod` reads `next` once and the record keeps it, so every step calls
        // the method the opening captured. Reading it per step is a `Get` the language does not
        // perform: a three-element loop asked a proxy four times where it asks once, and an
        // accessor that answers differently the second time was followed rather than pinned.
        let next = self.constant_index(Constant::String("next".to_owned()));
        self.load_hidden(&iterator_binding);
        self.code.push(Instruction::GetProperty(next));
        self.declare_hidden(&next_binding, span);

        // One handler for the whole loop rather than one per iteration: the guarded region is
        // the same every time round, and pushing it once keeps `continue` an ordinary jump.
        let handler_jump = self.emit_jump(Instruction::PushHandler(0));

        let start = self.code.len();
        self.load_hidden(&iterator_binding);
        self.load_hidden(&next_binding);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        if awaiting {
            // An async iterator hands back a promise of the result object; a sync one hands back
            // the object itself and has nothing to wait for here.
            self.load_hidden(&asynchronous_binding);
            let ready = self.emit_jump(Instruction::JumpIfFalse(0));
            self.code.push(Instruction::Await);
            self.patch_jump(ready);
        }
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
        if awaiting {
            // The other half of the rule: a sync iterator's values are awaited one by one, which
            // is what makes `for await (const x of promises)` work on a plain array.
            self.load_hidden(&asynchronous_binding);
            let ready = self.emit_jump(Instruction::JumpIfTruthy(0));
            self.code.push(Instruction::Await);
            self.patch_jump(ready);
        }
        // A pattern, so `for (const [key, value] of entries)` binds both -- through the same
        // `bind_pattern` a declaration uses, defaults and nesting included.
        self.bind_pattern(kind, pattern, span);

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
        let wrapped_binding = format!("%wrapped{}", self.hidden_bindings);
        let subject_binding = format!("%subject{}", self.hidden_bindings);
        let next_binding = format!("%next{}", self.hidden_bindings);
        let method_binding = format!("%method{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());

        // Whether each step has to be awaited, and where. Fixed at compile time by the kind of
        // generator this `yield*` was written in, exactly as `GetGeneratorKind` fixes it.
        let asynchronous = self.async_generator;
        let symbol = self.constant_index(Constant::String("@@iterator".to_owned()));
        let value_key = self.constant_index(Constant::String("value".to_owned()));

        match value {
            Some(value) => self.expression(value),
            None => self.push_constant(Constant::Undefined),
        }
        if asynchronous {
            // `GetIterator(value, async)`: `@@asyncIterator` decides, and `@@iterator` is only
            // reached when it is absent. A present-but-not-callable `@@asyncIterator` is a
            // TypeError raised *without* ever touching `@@iterator` -- which is what the two
            // must not share a path for.
            self.declare_hidden(&subject_binding, span);
            let asynchronous_key =
                self.constant_index(Constant::String("@@asyncIterator".to_owned()));
            // Read once and kept: `GetMethod` performs a single `Get`, and the method it answers
            // with is the one that is called. Reading the symbol to decide and again to call it
            // ran an accessor twice and let a program hand back a method that was never invoked.
            self.load_hidden(&subject_binding);
            self.code.push(Instruction::GetProperty(asynchronous_key));
            self.declare_hidden(&method_binding, span);
            self.load_hidden(&method_binding);
            let is_async = self.emit_jump(Instruction::JumpIfNotNullish(0));

            // Absent: the sync iterator, whose steps the specification wraps in an
            // async-from-sync iterator. The wrapper is not materialised -- nothing here hands
            // it to the program -- but the two places it awaits are, which is the whole of what
            // it does that the async protocol does not.
            self.load_hidden(&subject_binding);
            self.code.push(Instruction::CallMethod {
                name: symbol,
                argc: 0,
            });
            self.push_constant(Constant::Boolean(true));
            let chosen = self.emit_jump(Instruction::Jump(0));

            self.patch_jump(is_async);
            self.load_hidden(&subject_binding);
            self.load_hidden(&method_binding);
            self.code.push(Instruction::NewArray { count: 0 });
            self.code.push(Instruction::Apply);
            self.push_constant(Constant::Boolean(false));

            self.patch_jump(chosen);
            self.declare_hidden(&wrapped_binding, span);
            self.declare_hidden(&iterator_binding, span);
        } else {
            self.code.push(Instruction::CallMethod {
                name: symbol,
                argc: 0,
            });
            self.declare_hidden(&iterator_binding, span);
        }

        // The iterator record's `next`, read once when the delegate was opened. Every step calls
        // what was captured, which is what the specification does and what a re-read per step
        // could not be: `yield*` over three values asked for `next` four times.
        let next = self.constant_index(Constant::String("next".to_owned()));
        self.load_hidden(&iterator_binding);
        self.code.push(Instruction::GetProperty(next));
        self.declare_hidden(&next_binding, span);

        self.push_constant(Constant::Undefined);
        self.declare_hidden(&sent_binding, span);

        let start = self.code.len();
        self.load_hidden(&iterator_binding);
        self.load_hidden(&next_binding);
        self.load_hidden(&sent_binding);
        self.code.push(Instruction::NewArray { count: 1 });
        self.code.push(Instruction::Apply);
        if asynchronous {
            // An async iterator answers with a promise of the step, so the step object itself
            // is what has to be awaited. A wrapped sync iterator answers with the step directly
            // and it is each *value* that may be a promise, which is awaited further down.
            self.load_hidden(&wrapped_binding);
            let sync_step = self.emit_jump(Instruction::JumpIfTruthy(0));
            self.code.push(Instruction::Await);
            self.patch_jump(sync_step);
        }
        self.declare_hidden(&step_binding, span);

        self.load_hidden(&step_binding);
        let done = self.constant_index(Constant::String("done".to_owned()));
        self.code.push(Instruction::GetProperty(done));
        let finished = self.emit_jump(Instruction::JumpIfTruthy(0));

        // Not done: yield this value out, and keep what the caller sends back.
        //
        // No await here even for a wrapped sync iterator: `Yield` inside an async generator
        // already awaits what it is handed, and `AsyncFromSyncIteratorContinuation` resolves
        // each value exactly once. Awaiting again would unwrap a promise a second time and
        // spend a tick the specification does not.
        self.load_hidden(&step_binding);
        self.code.push(Instruction::GetProperty(value_key));
        self.code.push(Instruction::Yield);
        self.store_hidden(&sent_binding);
        self.code.push(Instruction::Jump(start as u32));

        // Done: the delegate's own return value is what `yield*` evaluates to.
        self.patch_jump(finished);
        self.load_hidden(&step_binding);
        self.code.push(Instruction::GetProperty(value_key));
        if asynchronous {
            self.await_wrapped_value(&wrapped_binding);
        }

        self.scopes.pop().expect("delegate scope was just pushed");
    }

    /// Await the value on top of the stack, but only when the delegate is a wrapped sync
    /// iterator.
    ///
    /// `AsyncFromSyncIteratorContinuation` resolves each value before it becomes the step, so a
    /// sync iterator of promises yielded through an async `yield*` hands on their values. A real
    /// async iterator has already settled its step by this point and awaiting again would cost a
    /// tick the specification does not spend.
    fn await_wrapped_value(&mut self, wrapped_binding: &str) {
        self.load_hidden(wrapped_binding);
        let plain = self.emit_jump(Instruction::JumpIfFalse(0));
        self.code.push(Instruction::Await);
        self.patch_jump(plain);
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
        // A `using` in a `for` head belongs to the loop rather than to one pass through it: the
        // head runs once, and what it declares is disposed of when the loop is left -- by the
        // test failing, by a `break`, by a `return`, or by a throw.
        let resources = init.map_or_else(Vec::new, |init| {
            resource_declarations(std::slice::from_ref(init))
        });
        if resources.is_empty() {
            self.for_statement_body(init, test, update, body);
            return;
        }
        self.with_resource_scope(&resources, span, |compiler| {
            compiler.for_statement_body(init, test, update, body);
        });
    }

    fn for_statement_body(
        &mut self,
        init: Option<&Statement>,
        test: Option<&Expression>,
        update: Option<&Expression>,
        body: &Statement,
    ) {
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

        // Every arm shares the switch's scope, so the hoisting pass spans all of them -- a
        // function declared in one case is callable from another, which is what one scope means.
        //
        // Emitted here, before the dispatch chain, because the chain's jumps land *inside* the
        // bodies: a prologue placed after them is skipped by every path that reaches an arm.
        // That is invisible while hoisting only allocates slots, and is a wrong answer the moment
        // it emits anything -- a `NewCell` for a captured declaration, or the dead-zone marker a
        // `let` in a case needs.
        for case in cases {
            for statement in &case.body {
                self.hoist_names(statement);
            }
        }

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
            ExpressionKind::Identifier(name) => {
                self.with_scoped_load(name, |compiler| compiler.load_lexically(name));
            }
            // `typeof x` inside a `with` has to ask the object first, and only then fall back
            // to the rule below -- the name may be absent from every binding around it and still
            // be a property of the object.
            ExpressionKind::Unary {
                operator: UnaryOperator::TypeOf,
                argument,
            } if matches!(
                &argument.kind,
                ExpressionKind::Identifier(name) if !self.with_scopes_for(name).is_empty()
            ) =>
            {
                let ExpressionKind::Identifier(name) = &argument.kind else {
                    unreachable!("the guard above matched an identifier")
                };
                let unbound = self.names_nothing(name)
                    && Builtin::from_name(name).is_none()
                    && name != "undefined"
                    && !(name == "arguments" && self.compiling_function());
                self.with_scoped_load(name, |compiler| {
                    // The *value* `undefined`, not the string: the `typeof` below is what turns
                    // it into one, and pushing the answer here would report `"string"`.
                    if unbound {
                        compiler.push_constant(Constant::Undefined);
                    } else {
                        compiler.load_lexically(name);
                    }
                });
                self.code.push(Instruction::Unary(UnaryOp::TypeOf));
            }
            // `typeof undeclaredName` is "undefined" rather than a throw -- the one place a name
            // may be read without existing, and the whole reason feature detection works.
            ExpressionKind::Unary {
                operator: UnaryOperator::TypeOf,
                argument,
            } if matches!(
                &argument.kind,
                ExpressionKind::Identifier(name)
                    if self.names_nothing(name)
                        && Builtin::from_name(name).is_none()
                        && name != "undefined"
                        && name != "this"
                        && !(name == "arguments" && self.compiling_function())
            ) =>
            {
                self.push_constant(Constant::String("undefined".to_owned()));
            }
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
            ExpressionKind::PatternAssignment { pattern, value } => {
                // An assignment is an expression whose value is the right-hand side, so a copy
                // is kept back before the pattern consumes it.
                self.expression(value);
                self.code.push(Instruction::Dup);
                self.assign_pattern(pattern, expression.span);
            }
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
            ExpressionKind::Call {
                callee,
                arguments,
                optional,
            } => self.call(callee, arguments, *optional, expression.span),
            ExpressionKind::This => self.code.push(Instruction::LoadThis),
            ExpressionKind::NewTarget => self.code.push(Instruction::LoadNewTarget),
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
            ExpressionKind::UpdateMember {
                increment,
                prefix,
                object,
                property,
            } => self.update_member(*increment, *prefix, object, property, expression.span),
            ExpressionKind::SuperCall { arguments } => {
                if Self::has_spread(arguments) {
                    // The count is the length of an array the program computes, which is exactly
                    // what `argc` cannot say. The parent is reached through the same runtime-list
                    // shape `Apply` uses, but through `SuperApply`, because running the parent
                    // against the instance being built is not an ordinary call.
                    self.load_super();
                    self.array_literal(arguments, expression.span);
                    self.code.push(Instruction::SuperApply);
                    return;
                }
                if arguments.len() > u16::MAX as usize {
                    self.error(Some(expression.span), "too many arguments to `super`");
                    return;
                }
                self.load_super();
                for argument in arguments {
                    let (ListElement::Item(value) | ListElement::Spread(value)) = argument else {
                        continue;
                    };
                    self.expression(value);
                }
                // The parent's return value is this expression's value, so `SuperCall` leaves
                // exactly one thing on the stack and needs no epilogue here.
                self.code.push(Instruction::SuperCall {
                    argc: arguments.len() as u16,
                });
            }
            // `#x in o` compiles to the ordinary `in`: a private field is stored under a key no
            // program can write, so asking whether the key is there *is* the brand check.
            ExpressionKind::PrivateIn { name, object } => {
                self.push_constant(Constant::String(self.private_key(name)));
                self.expression(object);
                self.code.push(Instruction::Binary(BinaryOp::In));
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
                // A function expression the source gave a name to can call itself by that name,
                // and the name is visible nowhere else. The scope pushed here is what keeps it
                // there -- the same shape a named class expression uses, and for the same reason.
                let self_bound = function
                    .binds_own_name
                    .then(|| function.name.clone())
                    .flatten();
                if let Some(name) = &self_bound {
                    self.scopes.push(HashMap::new());
                    self.predeclare(VariableKind::Const, name, false);
                }
                if let Some(index) = self.compile_function(function, expression.span) {
                    self.code.push(Instruction::MakeFunction(index));
                }
                if let Some(name) = &self_bound {
                    // The closure is both the expression's value and what the binding holds, so
                    // it is duplicated rather than loaded back.
                    self.code.push(Instruction::Dup);
                    match self.resolve(name) {
                        Resolution::Local(local) if local.celled => {
                            self.code.push(Instruction::StoreCell(local.slot));
                        }
                        Resolution::Local(local) => {
                            self.code.push(Instruction::StoreLocal(local.slot));
                        }
                        _ => {
                            self.error(Some(expression.span), "could not bind function expression");
                            self.code.push(Instruction::Pop);
                        }
                    }
                    self.scopes.pop().expect("function scope was just pushed");
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
                if !self.compiling_function() && !self.module_body {
                    // A script has nowhere to put the suspension: only a module body is an
                    // evaluation the host can stop and resume, which is what makes top-level
                    // `await` a module-only form rather than a general one.
                    self.error(
                        Some(expression.span),
                        "top-level `await` is only available in a module",
                    );
                    self.push_constant(Constant::Undefined);
                    return;
                }
                self.expression(value);
                self.code.push(Instruction::Await);
            }
            // The specifier expression is evaluated like any other, so a `throw` inside it
            // throws. Only what happens *after* it -- coercing it to a string, resolving it,
            // evaluating the module -- rejects the promise instead, which is why the coercion
            // is the instruction's job and not something emitted around it.
            ExpressionKind::ImportCall { specifier, options } => {
                self.expression(specifier);
                // The second argument is evaluated in source order and then discarded: nothing
                // reads an import attribute yet, but a getter written in one still runs.
                if let Some(options) = options {
                    self.expression(options);
                    self.code.push(Instruction::Pop);
                }
                self.code.push(Instruction::DynamicImport);
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
                    // `${x}` is ToString(x), which reduces an object with the *string* hint --
                    // `toString` before `valueOf`. Concatenation reduces with the default hint,
                    // the other way round, so a template is not `"" + x`: for an object with a
                    // `valueOf` and no own `toString` the two genuinely disagree, and desugaring
                    // to `+` silently produced the wrong one.
                    //
                    // A literal is already a primitive and converts to itself, so it keeps the
                    // plain concatenation -- which is also what leaves a template of constants
                    // compilable by a backend that has no calls.
                    if matches!(expression.kind, ExpressionKind::Literal(_)) {
                        self.expression(expression);
                    } else {
                        self.code.push(Instruction::LoadBuiltin(Builtin::String));
                        self.expression(expression);
                        self.code.push(Instruction::Call { argc: 1 });
                    }
                    self.code.push(Instruction::Binary(BinaryOp::Add));
                    // A run of text between two holes is skipped when it is empty, which is the
                    // common shape: `${a}${b}` has two empty quasis in the middle and the end.
                    if !quasi.is_empty() {
                        self.push_constant(Constant::String(quasi.clone()));
                        self.code.push(Instruction::Binary(BinaryOp::Add));
                    }
                }
            }
            ExpressionKind::TemplateStrings { cooked, raw } => {
                if cooked.len() > u32::MAX as usize {
                    self.error(Some(expression.span), "template has too many segments");
                    return;
                }
                // Both halves are frozen, which the specification requires: a tag that changed
                // the object it was handed would be changing something its caller may hold. The
                // `Object` doing the freezing is pushed first, under everything else, because
                // `CallMethod` wants its receiver beneath the argument and there is no swap.
                let freeze = self.constant_index(Constant::String("freeze".to_owned()));
                self.code.push(Instruction::LoadBuiltin(Builtin::Object));
                for segment in cooked {
                    match segment {
                        Some(text) => self.push_constant(Constant::String(text.clone())),
                        None => self.push_constant(Constant::Undefined),
                    }
                }
                self.code.push(Instruction::NewArray {
                    count: cooked.len() as u32,
                });
                // `SetProperty` leaves the value, not the target, so the object is duplicated
                // first and the leftover raw array popped after.
                self.code.push(Instruction::Dup);
                self.code.push(Instruction::LoadBuiltin(Builtin::Object));
                for segment in raw {
                    self.push_constant(Constant::String(segment.clone()));
                }
                self.code.push(Instruction::NewArray {
                    count: raw.len() as u32,
                });
                self.code.push(Instruction::CallMethod {
                    name: freeze,
                    argc: 1,
                });
                let name = self.constant_index(Constant::String("raw".to_owned()));
                self.code.push(self.set_property_instruction(name));
                self.code.push(Instruction::Pop);
                self.code.push(Instruction::CallMethod {
                    name: freeze,
                    argc: 1,
                });
            }
            ExpressionKind::RegExp { pattern, flags } => {
                let pattern = self.constant_index(Constant::String(pattern.clone()));
                let flags = self.constant_index(Constant::String(flags.clone()));
                self.code.push(Instruction::NewRegExp { pattern, flags });
            }
            ExpressionKind::PrivateMember { object, name } => {
                self.expression(object);
                let name = self.constant_index(Constant::String(self.private_key(name)));
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
                        self.code.push(self.delete_property_instruction(name));
                    }
                    ObjectKey::Computed(key) => {
                        self.expression(key);
                        self.code.push(self.delete_computed_instruction());
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
                // `{ __proto__: v }` is not a property at all: it links the object to `v`. Only
                // this spelling does -- `{ __proto__ }` and `{ __proto__() {} }` are ordinary own
                // properties, and so is `{ ["__proto__"]: v }`, because the specification asks
                // whether the key was computed rather than what it evaluated to.
                ObjectMember::Property(property)
                    if matches!(&property.key, ObjectKey::Static(key) if key == "__proto__")
                        && property.form == kiln_hir::PropertyForm::KeyValue =>
                {
                    // The prototype is set on an object, so the run of pairs written before it
                    // has to become one first -- the same reason an accessor flushes the run.
                    self.emit_object_segment(&mut pending, accumulating);
                    accumulating = true;
                    self.expression(&property.value);
                    self.code.push(Instruction::SetPrototype);
                }
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
                        enumerable: true,
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
        let scopes = self.with_scopes_for(target);
        if !scopes.is_empty() {
            self.with_assignment(&scopes, operator, target, value, span);
            return;
        }
        self.assign_lexically(operator, target, value, span);
    }

    /// `name = value` inside a `with`, written to the innermost object that has the name.
    ///
    /// The value expression is compiled once per object plus once for the lexical case. Only one
    /// of those copies ever runs, and the alternative -- spilling the value into a temporary
    /// first -- would evaluate it before the reference is resolved, which is the opposite of the
    /// order the specification gives.
    fn with_assignment(
        &mut self,
        scopes: &[WithScope],
        operator: AssignmentOperator,
        target: &str,
        value: &Expression,
        span: Span,
    ) {
        let key = self.constant_index(Constant::String(target.to_owned()));
        let mut found = Vec::with_capacity(scopes.len());
        for with in scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            if operator.is_short_circuiting() {
                self.load_with_object(with);
                self.code.push(Instruction::GetProperty(key));
                let skip = self.emit_jump(short_circuit_jump(operator));
                self.load_with_object(with);
                self.expression(value);
                self.code.push(self.set_property_instruction(key));
                let done = self.emit_jump(Instruction::Jump(0));
                self.patch_jump(skip);
                self.load_with_object(with);
                self.code.push(Instruction::GetProperty(key));
                self.patch_jump(done);
            } else if operator == AssignmentOperator::Assign {
                self.load_with_object(with);
                self.expression(value);
                self.code.push(self.set_property_instruction(key));
            } else {
                self.load_with_object(with);
                self.load_with_object(with);
                self.code.push(Instruction::GetProperty(key));
                self.expression(value);
                self.code
                    .push(Instruction::Binary(compound_operator(operator)));
                self.code.push(self.set_property_instruction(key));
            }
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        self.assign_lexically(operator, target, value, span);
        for jump in found {
            self.patch_jump(jump);
        }
    }

    /// `name = value` against the bindings around it, ignoring any `with` object.
    fn assign_lexically(
        &mut self,
        operator: AssignmentOperator,
        target: &str,
        value: &Expression,
        span: Span,
    ) {
        let Some((load, store)) = self.assignment_place(target, span) else {
            return;
        };
        if operator.is_short_circuiting() {
            // `a ??= b` reads `a`, and assigns only if it is the wrong one -- so `b` may never
            // be evaluated and the store may never happen. The current value is left on the
            // stack as the expression's result when the store is skipped.
            self.code.push(load.clone());
            let skip = self.emit_jump(short_circuit_jump(operator));
            self.expression(value);
            self.code.push(Instruction::Dup);
            self.code.push(store);
            let done = self.emit_jump(Instruction::Jump(0));
            self.patch_jump(skip);
            self.code.push(load);
            self.patch_jump(done);
            return;
        }
        if operator == AssignmentOperator::Assign {
            self.expression(value);
        } else {
            self.code.push(load);
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        }
        self.code.push(Instruction::Dup);
        self.code.push(store);
    }

    /// Store the value on top of the stack into an existing binding, consuming it.
    ///
    /// The leaf of a destructuring assignment. Unlike [`Compiler::assignment`] there is no
    /// operator and no result to leave behind: the pattern walk already holds what it needs.
    fn store_existing(&mut self, name: &str, span: Span) {
        if let Some((_, store)) = self.assignment_place(name, span) {
            self.code.push(store);
        }
    }

    /// How to read and write `target`, or `None` when it names nothing.
    ///
    /// Reports const and imported-binding assignments, and compiles an unresolved name to a
    /// `ReferenceError` raised where the assignment happens -- so a caller that gets `None` has
    /// already had its diagnostic or its throw emitted and should emit nothing more.
    fn assignment_place(&mut self, target: &str, span: Span) -> Option<(Instruction, Instruction)> {
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
                // Assigning to a name that does not exist is a `ReferenceError` in a module,
                // raised where the assignment happens rather than when it is compiled.
                let _ = span;
                let name = self.constant_index(Constant::String(target.to_owned()));
                self.code.push(Instruction::ThrowReference(name));
                return None;
            }
        };
        let (load, store): (Instruction, Instruction) = match place {
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
        // Assignment is also a read of the binding's state: `x = 1; let x;` is a ReferenceError,
        // not a write that makes the later declaration appear to have run. A switch shares one
        // scope between its cases, so compiler-time `initialized` can be true here after another
        // case's declaration even when this path reaches the assignment first. Probe the
        // run-time slot before every lexical write; initialized bindings pay only a load/pop.
        if kind != VariableKind::Var || matches!(&load, Instruction::LoadUpvalue(_)) {
            self.code.push(load.clone());
            self.code.push(Instruction::Pop);
        }
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
        Some((load, store))
    }

    /// Compile a class into a constructor and a prototype.
    ///
    /// The desugaring is not a shortcut -- a class really is a constructor function whose
    /// `prototype` holds the instance methods -- so it reuses the machinery `new` already needs
    /// rather than introducing a parallel object model for classes.
    /// A class expression leaves its constructor on the stack.
    fn class_expression(&mut self, class: &Class, span: Span) {
        // A class expression may be anonymous, and its name -- when it has one -- is visible only
        // inside its own body. The scope pushed below is what keeps it there, so a named class
        // expression can bind under its own name: `class A extends B { m() { return A; } }` as an
        // expression has to find `A` from inside `m`, and nowhere else. An anonymous one gets a
        // hidden binding instead, carrying a character an identifier cannot contain.
        let binding = match &class.name {
            Some(name) => name.clone(),
            None => {
                let hidden = format!("%class{}", self.hidden_bindings);
                self.hidden_bindings += 1;
                hidden
            }
        };

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

    /// A class body is strict code, whatever encloses it.
    ///
    /// Not only its methods -- which are functions and would inherit it anyway -- but the
    /// heritage expression, the computed keys and the field initialisers, all of which are
    /// compiled into whatever code surrounds the class. So the flag is raised around the whole
    /// declaration and lowered again after it.
    fn class_declaration(&mut self, class: &Class) {
        let outer_strict = std::mem::replace(&mut self.strict, true);
        self.strict_class_declaration(class);
        self.strict = outer_strict;
    }

    fn strict_class_declaration(&mut self, class: &Class) {
        let Some(name) = class.name.clone() else {
            self.error(Some(class.span), "a class declaration requires a name");
            return;
        };
        // Everything compiled from here to the end of this function -- fields, methods, and any
        // class nested inside one -- names its private fields in this class's own scope.
        self.private_scopes.push(self.next_private_scope);
        self.next_private_scope += 1;
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

        // A class expression with no written name still needs a binding to hold itself, and
        // `class_expression` invents one carrying a `%` that no identifier can contain. That
        // binding is scaffolding: `(class {}).name` is the empty string, and reporting the
        // scaffolding instead put `"%class0"` where the program could read it.
        let written_name = if name.starts_with('%') {
            String::new()
        } else {
            name.clone()
        };
        let constructor = match &class.constructor {
            // The constructor *is* the class as a value, so it carries the class's name: that is
            // what `new.target.name` and a console's `C {}` both read.
            Some(constructor) => Function {
                name: Some(written_name.clone()),
                ..constructor.clone()
            },
            // A class with no constructor gets an empty one, so `new` needs no special case for
            // classes that declared none.
            None => Function {
                rest: None,
                asynchronous: false,
                name: Some(written_name),
                // A class binds its own name through `class_expression`, not through this.
                binds_own_name: false,
                arrow: false,
                generator: false,
                parameters: Vec::new(),
                body: Vec::new(),
                strict: true,
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
                    let name = self.constant_index(Constant::String(self.private_key(name)));
                    self.emit_field_value(field);
                    self.code.push(self.set_property_instruction(name));
                }
                ObjectKey::Static(key) => {
                    let name = self.constant_index(Constant::String(key.clone()));
                    self.emit_field_value(field);
                    self.code.push(self.set_property_instruction(name));
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.emit_field_value(field);
                    self.code.push(self.set_computed_instruction());
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
                    self.push_constant(Constant::String(self.private_key(name)));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::DefineMethod);
                }
                ObjectKey::Static(key) if !accessor => {
                    self.push_constant(Constant::String(key.clone()));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::DefineMethod);
                }
                ObjectKey::Static(key) => {
                    self.push_constant(Constant::String(key.clone()));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::DefineAccessor {
                        setter: matches!(method.kind, kiln_hir::MethodKind::Set),
                        enumerable: false,
                    });
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.code.push(Instruction::MakeFunction(body));
                    if accessor {
                        self.code.push(Instruction::DefineAccessor {
                            setter: matches!(method.kind, kiln_hir::MethodKind::Set),
                            enumerable: false,
                        });
                    } else {
                        self.code.push(Instruction::DefineMethod);
                    }
                }
            }
            // Each definition leaves something; nothing consumes it.
            self.code.push(Instruction::Pop);
        }

        if class.superclass.is_some() {
            self.super_bindings.pop();
        }
        self.private_scopes
            .pop()
            .expect("this class pushed its own private scope");
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
        let mut captured = names_captured_by_this_function(function);
        // `super` is syntax, not an identifier, so the ordinary name walk cannot discover the
        // hidden binding that represents it. Every callable compiled while a derived class is
        // active may emit `load_super` (an explicit call/member access, or a synthesized
        // forwarding constructor) and must carry that binding with the closure. Otherwise an
        // exported derived class reads the caller module's slot by number when constructed.
        if let Some(super_binding) = self.super_bindings.last() {
            captured.insert(super_binding.clone());
        }
        // A `with` object is a hidden binding for the same reason, and the name walk cannot see
        // it either. A function written inside a `with` body still resolves its names against
        // that object when it is called, which may be long after the statement has ended -- so
        // the closure has to carry it.
        for with in &self.with_scopes {
            captured.insert(with.name.clone());
        }
        // A direct `eval` anywhere in this function -- including inside one nested in it -- means
        // the eval'd source may name any binding here, so every one of them has to be a cell.
        let cell_everything = function_contains_direct_eval(function);

        // Every parameter owns a positional slot, whether or not it binds a name there. A plain
        // identifier binds the slot itself, which keeps the common call free of any prologue; a
        // pattern leaves the slot anonymous and is destructured out of it below, into ordinary
        // locals that behave like any other binding.
        let mut parameters = HashMap::with_capacity(function.parameters.len());
        let mut celled_parameters = Vec::new();
        let mut binding_steps = Vec::new();
        // Whether this list has a dead zone to keep at all. Without a default there is nothing
        // that can run while a parameter is unbound, so a function with none pays nothing and
        // behaves exactly as it did.
        let parameters_have_defaults = function
            .parameters
            .iter()
            .any(|parameter| matches!(parameter.pattern, Pattern::WithDefault { .. }));
        for (slot, parameter) in function.parameters.iter().enumerate() {
            let slot = slot as u32;
            let identifier = match &parameter.pattern {
                Pattern::Identifier(name) => Some(name),
                Pattern::WithDefault { inner, .. } => match inner.as_ref() {
                    Pattern::Identifier(name) => Some(name),
                    _ => None,
                },
                _ => None,
            };
            let Some(name) = identifier else {
                binding_steps.push(ParameterStep::Destructure(
                    slot,
                    &parameter.pattern,
                    parameter.span,
                ));
                continue;
            };
            if parameters.contains_key(name) {
                self.error(
                    Some(parameter.span),
                    format!("duplicate parameter `{name}`"),
                );
                return None;
            }
            let celled = captured.contains(name) || cell_everything;
            if celled {
                celled_parameters.push(slot);
            }
            parameters.insert(
                name.clone(),
                Local {
                    slot,
                    // Parameters share a function's variable environment with `var`.
                    // Their initialization rules are still handled below, but `var x` in a
                    // body whose parameter is `x` must reuse this slot.
                    kind: VariableKind::Var,
                    imported: false,
                    // Uninitialised until the binding list reaches it, which is what puts the
                    // parameters to a default's right in a dead zone.
                    initialized: !parameters_have_defaults,
                    celled,
                },
            );
            binding_steps.push(
                if matches!(parameter.pattern, Pattern::WithDefault { .. }) {
                    ParameterStep::Defaulted(slot, celled, &parameter.pattern, parameter.span, name)
                } else {
                    ParameterStep::Plain(name)
                },
            );
        }

        let parameter_count = function.parameters.len() as u32;
        // A rest parameter is already excluded: it lives in `function.rest` rather than among
        // the positional ones, so counting stops at the first default on its own.
        let declared_length = function
            .parameters
            .iter()
            .take_while(|parameter| !matches!(parameter.pattern, Pattern::WithDefault { .. }))
            .count() as u32;
        let outer_finalizer_depth =
            std::mem::replace(&mut self.function_finalizer_depth, self.finalizers.len());
        // A nested body cannot jump out to an enclosing loop, so it starts with no handlers of
        // its own to account for.
        let outer_handler_depth = std::mem::replace(&mut self.handler_depth, 0);
        // An arrow cannot contain a `yield`, so it never reads this; every other function body
        // establishes its own generator kind, and restores the enclosing one on the way out.
        let outer_async_generator = std::mem::replace(
            &mut self.async_generator,
            function.generator && function.asynchronous,
        );
        // Strictness is inherited and never given up: a body written inside strict code is
        // strict whether or not it says so, and a `"use strict"` of its own turns it on for
        // this body and everything nested in it. Restored on the way out, because a sibling
        // written after a strict function is not strict because of it.
        let inherited_strict = self.strict || function.strict;
        let outer_strict = std::mem::replace(&mut self.strict, inherited_strict);
        let outer = FunctionState {
            code: std::mem::take(&mut self.code),
            scopes: std::mem::replace(&mut self.scopes, vec![parameters]),
            local_count: std::mem::replace(&mut self.local_count, parameter_count),
            upvalues: std::mem::take(&mut self.upvalues),
            upvalue_names: std::mem::take(&mut self.upvalue_names),
            captured: std::mem::replace(&mut self.captured, captured),
            cell_everything: std::mem::replace(&mut self.cell_everything, cell_everything),
        };
        self.enclosing.push(outer);

        // A captured parameter arrives as a plain value in its slot, so the cell has to be
        // created and seeded before the body can read it.
        for slot in celled_parameters {
            self.code.push(Instruction::LoadLocal(slot));
            self.code.push(Instruction::NewCell(slot));
            self.code.push(Instruction::StoreCell(slot));
        }

        // Bind the parameters in declaration order, before anything in the body can read them.
        // In *declaration* order rather than patterns-then-defaults, because the order is
        // observable: a default is an expression, and it runs after everything to its left has
        // been bound and before anything to its right has. `bind_pattern` is the same code a
        // `const { a } = x` uses, so defaults, renaming, nesting and rest behave identically
        // wherever they are written.
        let outer_dead_zone =
            std::mem::replace(&mut self.parameter_dead_zone, parameters_have_defaults);
        for step in binding_steps {
            match step {
                ParameterStep::Destructure(slot, pattern, span) => {
                    self.code.push(Instruction::LoadLocal(slot));
                    self.bind_pattern(VariableKind::Let, pattern, span);
                }
                ParameterStep::Defaulted(slot, celled, pattern, span, name) => {
                    self.code.push(if celled {
                        Instruction::LoadCell(slot)
                    } else {
                        Instruction::LoadLocal(slot)
                    });
                    self.assign_pattern(pattern, span);
                    // Past its own default, and so out of the dead zone for everything after it.
                    self.initialize(name);
                }
                // Already in its slot: the call put it there, and there is nothing to emit.
                ParameterStep::Plain(name) => self.initialize(name),
            }
        }
        self.parameter_dead_zone = outer_dead_zone;

        // `...rest` is the arguments beyond the declared parameters. Taken from the call rather
        // than from slots, because its length is decided by the caller.
        if let Some(rest) = &function.rest {
            self.code.push(Instruction::RestArguments(parameter_count));
            self.bind_pattern(VariableKind::Let, rest, function.span);
        }

        // Everything above binds parameters, and so is what the language calls
        // FunctionDeclarationInstantiation; everything below is the body. A generator has to be
        // able to run the first without the second, so the seam is recorded rather than left
        // implicit in the instruction stream.
        let prologue_end = self.code.len() as u32;

        if forwards_to_super {
            self.load_super();
            self.code.push(Instruction::SuperForward);
            self.code.push(Instruction::Pop);
        }
        for field in fields {
            self.code.push(Instruction::LoadThis);
            match &field.key {
                ObjectKey::Private(name) => {
                    let name = self.constant_index(Constant::String(self.private_key(name)));
                    self.emit_field_value(field);
                    self.code.push(self.set_property_instruction(name));
                }
                ObjectKey::Static(key) => {
                    let name = self.constant_index(Constant::String(key.clone()));
                    self.emit_field_value(field);
                    self.code.push(self.set_property_instruction(name));
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.emit_field_value(field);
                    self.code.push(self.set_computed_instruction());
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
        let strict = std::mem::replace(&mut self.strict, outer_strict);
        let proto = FunctionProto {
            name: function.name.clone(),
            arrow: function.arrow,
            asynchronous: function.asynchronous,
            // An async body suspends at every `await`, which is a `yield` in every way the
            // machine cares about -- so it is compiled as a generator and driven by the async
            // driver rather than by the program.
            generator: function.generator || function.asynchronous,
            async_generator: function.generator && function.asynchronous,
            parameter_count,
            declared_length,
            local_count: std::mem::replace(&mut self.local_count, restored.local_count),
            prologue_end,
            code: std::mem::replace(&mut self.code, restored.code),
            upvalues: std::mem::replace(&mut self.upvalues, restored.upvalues),
        };
        self.scopes = restored.scopes;
        self.function_finalizer_depth = outer_finalizer_depth;
        self.handler_depth = outer_handler_depth;
        self.async_generator = outer_async_generator;
        self.upvalue_names = restored.upvalue_names;
        self.captured = restored.captured;
        self.cell_everything = restored.cell_everything;

        let index = self.functions.len() as u32;
        self.functions.push(proto);
        self.strict_functions.push(strict);
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
        let scopes = self.with_scopes_for(target);
        if !scopes.is_empty() {
            self.with_update(&scopes, increment, prefix, target, span);
            return;
        }
        self.update_lexically(increment, prefix, target, span);
    }

    /// `name++` inside a `with`, updating the innermost object that has the name.
    ///
    /// The old value is spilled into a hidden binding rather than juggled on the stack: the
    /// object has to sit beneath the new value for the store, and the old one has to survive
    /// past it for a postfix update, which is one value too many to keep in order otherwise.
    fn with_update(
        &mut self,
        scopes: &[WithScope],
        increment: bool,
        prefix: bool,
        target: &str,
        span: Span,
    ) {
        let key = self.constant_index(Constant::String(target.to_owned()));
        let held = format!("%update{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        let mut found = Vec::with_capacity(scopes.len());
        for with in scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_with_object(with);
            self.code.push(Instruction::GetProperty(key));
            // `ToNumeric` first, for the same reason the lexical form does it: `x++` where `x`
            // is "5" is 5 then 6, not "5" then "51".
            self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
            self.declare_hidden(&held, span);
            self.load_with_object(with);
            self.load_hidden(&held);
            self.code.push(Instruction::Unary(if increment {
                UnaryOp::Increment
            } else {
                UnaryOp::Decrement
            }));
            self.code.push(self.set_property_instruction(key));
            if !prefix {
                self.code.push(Instruction::Pop);
                self.load_hidden(&held);
            }
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        self.update_lexically(increment, prefix, target, span);
        for jump in found {
            self.patch_jump(jump);
        }
    }

    /// `name++` against the bindings around it, ignoring any `with` object.
    fn update_lexically(&mut self, increment: bool, prefix: bool, target: &str, span: Span) {
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
                // Inside a `with` the name may be a property of the object and only fail to be
                // one at run time, so the refusal has to wait until then. Outside one the
                // compiler already knows, and says so where the mistake is written.
                if self.with_scopes.is_empty() {
                    self.error(Some(span), format!("unresolved identifier `{target}`"));
                    self.push_constant(Constant::Undefined);
                } else {
                    let name = self.constant_index(Constant::String(target.to_owned()));
                    self.code.push(Instruction::ThrowReference(name));
                }
                return;
            }
        };

        self.code.push(load);
        // `ToNumeric` first: `x++` where `x` is "5" is 5 then 6, not "5" then "51". Without a
        // conversion the `+ 1` below concatenates, which is the trap the member form fell into --
        // and `ToNumber` is not the conversion an update applies: it narrows a BigInt, so `1n++`
        // left a *number* behind and the addition would have refused to mix the two anyway.
        self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
        if !prefix {
            // Keep the old value beneath the arithmetic; the store below consumes the new one.
            self.code.push(Instruction::Dup);
        }
        self.code.push(Instruction::Unary(if increment {
            UnaryOp::Increment
        } else {
            UnaryOp::Decrement
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
        if operator.is_short_circuiting() {
            self.short_circuiting_member(operator, object, None, Some(property), value);
            return;
        }
        self.expression(object);
        self.expression(property);

        if operator != AssignmentOperator::Assign {
            self.code.push(Instruction::Dup2);
            self.code.push(Instruction::GetComputed);
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }

        self.code.push(self.set_computed_instruction());
    }

    /// `object.property ??= value`, and the `&&=`/`||=` forms, static or computed.
    ///
    /// These read the property and assign only if it is the wrong one, so the right-hand side may
    /// never be evaluated and the store may never happen -- and the receiver, the key and the
    /// current value each have to survive that decision without being computed twice. Hidden
    /// locals hold them, for the same reason an update does.
    fn short_circuiting_member(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        name: Option<u32>,
        key: Option<&Expression>,
        value: &Expression,
    ) {
        let span = object.span;
        let receiver = format!("%sc{}", self.hidden_bindings);
        let key_binding = format!("%sckey{}", self.hidden_bindings);
        let current = format!("%sccur{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        self.expression(object);
        self.declare_hidden(&receiver, span);
        if let Some(key) = key {
            self.expression(key);
            self.declare_hidden(&key_binding, span);
        }

        self.load_hidden(&receiver);
        match name {
            Some(name) => self.code.push(Instruction::GetProperty(name)),
            None => {
                self.load_hidden(&key_binding);
                self.code.push(Instruction::GetComputed);
            }
        }
        self.declare_hidden(&current, span);

        self.load_hidden(&current);
        let skip = self.emit_jump(short_circuit_jump(operator));

        self.load_hidden(&receiver);
        if name.is_none() {
            self.load_hidden(&key_binding);
        }
        self.expression(value);
        match name {
            Some(name) => self.code.push(self.set_property_instruction(name)),
            None => self.code.push(self.set_computed_instruction()),
        }
        let done = self.emit_jump(Instruction::Jump(0));

        // Skipped: the expression evaluates to the value that was already there.
        self.patch_jump(skip);
        self.load_hidden(&current);
        self.patch_jump(done);
        self.scopes.pop().expect("assignment scope was just pushed");
    }

    /// `object.property++`, and the other three spellings.
    ///
    /// The object and the key are each evaluated once and held in hidden locals: `o().x++` must
    /// not call `o` twice, and `a[i()]++` must not call `i` twice. That is the same rule a
    /// compound member assignment follows, and the reason neither can be lowered to
    /// `o.x = o.x + 1`.
    fn update_member(
        &mut self,
        increment: bool,
        prefix: bool,
        object: &Expression,
        property: &ObjectKey,
        span: Span,
    ) {
        if let ObjectKey::Private(name) = property {
            self.error(Some(span), format!("update of the private name `#{name}`"));
            self.push_constant(Constant::Undefined);
            return;
        }

        let receiver = format!("%upd{}", self.hidden_bindings);
        let key_binding = format!("%updkey{}", self.hidden_bindings);
        let old_binding = format!("%old{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        self.expression(object);
        self.declare_hidden(&receiver, span);
        let name = match property {
            ObjectKey::Static(name) => Some(self.constant_index(Constant::String(name.clone()))),
            ObjectKey::Computed(key) => {
                self.expression(key);
                self.declare_hidden(&key_binding, span);
                None
            }
            ObjectKey::Private(_) => unreachable!("refused above"),
        };

        // Read the current value, as a *numeric*: the old value a postfix update yields is the
        // conversion's result rather than whatever was stored.
        self.load_hidden(&receiver);
        match name {
            Some(name) => self.code.push(Instruction::GetProperty(name)),
            None => {
                self.load_hidden(&key_binding);
                self.code.push(Instruction::GetComputed);
            }
        }
        // `ToNumeric`, not `+ 0` and not unary plus: `+` on a string concatenates, so `o.s++`
        // where `s` is "5" would produce "50", and unary plus narrows a BigInt to a number, which
        // an update must not.
        self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
        self.declare_hidden(&old_binding, span);

        // Write the new value through the same reference the read used.
        self.load_hidden(&receiver);
        if name.is_none() {
            self.load_hidden(&key_binding);
        }
        self.load_hidden(&old_binding);
        self.code.push(Instruction::Unary(if increment {
            UnaryOp::Increment
        } else {
            UnaryOp::Decrement
        }));
        match name {
            Some(name) => self.code.push(self.set_property_instruction(name)),
            None => self.code.push(self.set_computed_instruction()),
        }

        // The store leaves the new value, which is what a prefix update evaluates to; a postfix
        // one discards it and hands back what was there before.
        if !prefix {
            self.code.push(Instruction::Pop);
            self.load_hidden(&old_binding);
        }
        self.scopes.pop().expect("update scope was just pushed");
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
        let name = self.constant_index(Constant::String(self.private_key(name)));
        if operator.is_short_circuiting() {
            self.short_circuiting_private(operator, object, name, value);
            return;
        }
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

    /// `object.#name ??= value`, and the `&&=`/`||=` forms.
    ///
    /// The same shape as [`Compiler::short_circuiting_member`] and for the same reason: the
    /// receiver has to survive a decision that may skip both the right-hand side and the store,
    /// so `o().#x ||= v` calls `o` once. Without this the operator fell through to the compound
    /// path, which asks `compound_operator` for the arithmetic a short-circuiting assignment
    /// does not have -- and that is unreachable, so the compiler panicked on valid code.
    fn short_circuiting_private(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        name: u32,
        value: &Expression,
    ) {
        let span = object.span;
        let receiver = format!("%scp{}", self.hidden_bindings);
        let current = format!("%scpcur{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        self.expression(object);
        self.declare_hidden(&receiver, span);

        self.load_hidden(&receiver);
        self.code.push(Instruction::GetPrivate(name));
        self.declare_hidden(&current, span);

        self.load_hidden(&current);
        let skip = self.emit_jump(short_circuit_jump(operator));

        self.load_hidden(&receiver);
        self.expression(value);
        self.code.push(Instruction::SetPrivate(name));
        let done = self.emit_jump(Instruction::Jump(0));

        // Skipped: the expression evaluates to the value that was already there.
        self.patch_jump(skip);
        self.load_hidden(&current);
        self.patch_jump(done);
        self.scopes.pop().expect("assignment scope was just pushed");
    }

    fn member_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        property: &str,
        value: &Expression,
    ) {
        let name = self.constant_index(Constant::String(property.to_owned()));
        if operator.is_short_circuiting() {
            self.short_circuiting_member(operator, object, Some(name), None, value);
            return;
        }
        self.expression(object);

        if operator != AssignmentOperator::Assign {
            // Read the current value from the same object reference the write will use.
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::GetProperty(name));
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }

        // `SetProperty` leaves the assigned value, which is what an assignment expression
        // evaluates to.
        self.code.push(self.set_property_instruction(name));
    }

    /// `f?.()` and `o.m?.()`: call only when the callee is there.
    ///
    /// The callee is read into a hidden local so the nullish test has something to look at
    /// without disturbing the receiver beneath it, and the receiver into another so a method call
    /// keeps its `this` across the jump. Skipping means the arguments are never evaluated, which
    /// is the point: `maybe?.(expensive())` does not compute the argument when `maybe` is absent.
    fn optional_call(&mut self, callee: &Expression, arguments: &[ListElement], span: Span) {
        if self.chains.is_empty() {
            self.error(Some(span), "`?.()` outside of an optional chain");
            self.push_constant(Constant::Undefined);
            return;
        }
        let receiver = format!("%oc{}", self.hidden_bindings);
        let method = format!("%ocfn{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        // A method call keeps its receiver; anything else has none and gets `undefined`.
        let has_receiver = match &callee.kind {
            ExpressionKind::StaticMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                self.declare_hidden(&receiver, span);
                self.load_hidden(&receiver);
                let name = self.constant_index(Constant::String(property.clone()));
                self.code.push(Instruction::GetProperty(name));
                true
            }
            ExpressionKind::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                self.declare_hidden(&receiver, span);
                self.load_hidden(&receiver);
                self.expression(property);
                self.code.push(Instruction::GetComputed);
                true
            }
            _ => {
                self.expression(callee);
                false
            }
        };
        self.declare_hidden(&method, span);

        self.load_hidden(&method);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        // Absent: the whole chain is undefined, and the arguments are never touched.
        self.push_constant(Constant::Undefined);
        let exit = self.emit_jump(Instruction::Jump(0));
        self.chains
            .last_mut()
            .expect("checked just above")
            .push(exit);

        self.patch_jump(present);
        if has_receiver {
            self.load_hidden(&receiver);
        } else {
            self.push_constant(Constant::Undefined);
        }
        self.load_hidden(&method);
        self.array_literal(arguments, span);
        self.code.push(Instruction::Apply);
        self.scopes.pop().expect("call scope was just pushed");
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

    fn call(&mut self, callee: &Expression, arguments: &[ListElement], optional: bool, span: Span) {
        if optional {
            self.optional_call(callee, arguments, span);
            return;
        }
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

        // `eval(...)`, written as that name, with nothing shadowing it: a *direct* eval, which
        // evaluates in this scope rather than in the global one. Every other way of reaching the
        // same function -- `(0, eval)(s)`, `globalThis.eval(s)`, a saved reference -- is indirect
        // and compiles to an ordinary call of `Builtin::Eval`. The difference is the language's,
        // not an optimisation: only the direct form can see the names around it.
        if let ExpressionKind::Identifier(name) = &callee.kind
            && name == "eval"
            && self.names_nothing("eval")
        {
            let bindings = self.eval_site_bindings(span);
            let argc = if spreading {
                self.array_literal(arguments, span);
                None
            } else {
                Some(self.emit_arguments(arguments))
            };
            self.code.push(Instruction::DirectEval(Box::new(EvalSite {
                bindings,
                strict: self.strict,
                argc,
            })));
            return;
        }
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
                // `super.m(...xs)` resolves the method on the parent but runs it against the
                // current receiver, which is precisely the `[this, callee, arguments]` shape
                // `Apply` takes -- no dedicated instruction is needed for this half.
                self.code.push(Instruction::LoadThis);
                self.expression(callee);
                self.array_literal(arguments, span);
                self.code.push(Instruction::Apply);
                return;
            }
            // `this.#m()` is a method call: the private name is read off the receiver and the
            // receiver becomes `this`. Reached through `Apply` rather than a `CallPrivate`
            // instruction, because the shape `Apply` already takes -- receiver, callee,
            // arguments -- is exactly this one.
            ExpressionKind::PrivateMember { object, name } => {
                let name = self.constant_index(Constant::String(self.private_key(name)));
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

    /// The bindings a direct `eval` written here can see, innermost first.
    ///
    /// Every one is handed over as a *cell*, so the eval'd source shares the binding rather than
    /// a copy of its value: `function f() { var x = 1; eval("x = 2"); return x; }` is 2, and a
    /// copy would make it 1. That is what [`Compiler::cell_everything`] guarantees -- a function
    /// containing an eval boxes all of its bindings, and so does every function around it.
    ///
    /// Resolving each name is what *captures* it, so an enclosing function's binding gains its
    /// upvalue here rather than at a use the eval'd source has not been written yet.
    fn eval_site_bindings(&mut self, span: Span) -> Vec<EvalBinding> {
        // Names first: resolving one mutates the upvalue lists, which cannot happen while the
        // scopes it reads are borrowed. Innermost scope first, so a shadowed name never wins;
        // sorted within a scope, because two bindings of one scope cannot shadow each other and
        // a hash map's order would otherwise make compilation non-reproducible.
        // The declaring `Local` is taken here too. An upvalue cannot say how its binding was
        // declared -- a capture carries no kind -- and the eval'd source needs to know: `var x`
        // over a caller's `var` is that binding, and over a caller's `let` it is a SyntaxError.
        let mut order: Vec<(String, EvalDeclaration)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let levels = self.scopes.iter().rev().chain(
            self.enclosing
                .iter()
                .rev()
                .flat_map(|level| level.scopes.iter().rev()),
        );
        for scope in levels {
            let mut names: Vec<&String> = scope.keys().collect();
            names.sort();
            for name in names {
                // A compiler-introduced binding carries a character an identifier cannot, so it
                // is never a name the eval'd source could write.
                if name.starts_with('%') || !seen.insert(name.clone()) {
                    continue;
                }
                let local = scope[name];
                // An import is the exporter's binding, and writing one is refused wherever it is
                // written. `const` is the nearest thing this vocabulary has to that.
                let declaration = match local.kind {
                    _ if local.imported => EvalDeclaration::Const,
                    VariableKind::Var => EvalDeclaration::Var,
                    VariableKind::Let => EvalDeclaration::Let,
                    // A `using` binding cannot be reassigned either, and what an eval'd source
                    // may do with it is exactly what it may do with a `const`.
                    VariableKind::Const | VariableKind::Using | VariableKind::AwaitUsing => {
                        EvalDeclaration::Const
                    }
                };
                order.push((name.clone(), declaration));
            }
        }

        let mut bindings = Vec::with_capacity(order.len());
        for (name, declaration) in order {
            let access = match self.resolve_or_capture(&name) {
                Resolution::Local(local) if local.celled => EvalAccess::Cell(local.slot),
                Resolution::Upvalue(index) => EvalAccess::Upvalue(index),
                // Unreachable: `cell_everything` boxed every binding in every scope that can
                // reach this call. Refusing rather than dropping the name, because a dropped
                // binding does not disappear -- the eval'd source would resolve it to a built-in
                // or to nothing, and answer with something plausible from the wrong scope.
                _ => {
                    self.error(
                        Some(span),
                        format!("a direct `eval` cannot reach the binding `{name}`"),
                    );
                    continue;
                }
            };
            bindings.push(EvalBinding {
                name,
                access,
                declaration,
            });
        }
        bindings
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

    /// Whether `name` binds nothing anywhere in scope, without creating a capture to find out.
    ///
    /// `resolve` alone is not this question: it looks in the current function and then jumps to
    /// module scope, skipping every function in between. Asking it directly made
    /// `typeof enclosingParameter` answer "undefined" -- which is the wrong answer for a name
    /// that plainly exists, and is exactly what every CommonJS shim tests when it writes
    /// `typeof exports == "object"` inside a nested function.
    fn names_nothing(&self, name: &str) -> bool {
        if self.scopes.iter().any(|scope| scope.contains_key(name)) {
            return false;
        }
        if self
            .enclosing
            .iter()
            .any(|level| level.scopes.iter().any(|scope| scope.contains_key(name)))
        {
            return false;
        }
        matches!(self.resolve(name), Resolution::Unresolved)
    }

    /// Thread a binding from the function that owns it down to the one being compiled.
    ///
    /// Every function in between gains an upvalue too, because a closure can only reach cells
    /// its immediately enclosing frame holds -- a two-level capture is two hops, not one long
    /// one. Levels are indices into `enclosing` followed by the current function.
    ///
    /// Module scope is included. It was skipped while a module binding was reached through the
    /// module frame instead, which cannot survive a function value leaving its module.
    fn capture(&mut self, name: &str) -> Option<u32> {
        // Scope 0 of level 0 is module scope, whose bindings are globals rather than captures --
        // but a *block* inside the module body is an ordinary scope, and a closure created there
        // captures from it like any other. Skipping level 0 entirely missed exactly that: a
        // `for-of` at module level whose body creates a closure over the loop variable.
        let (owner, local) = (0..self.enclosing.len()).rev().find_map(|level| {
            self.enclosing[level]
                .scopes
                .iter()
                .rev()
                .find_map(|scope| scope.get(name))
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
            Literal::BigInt(digits) => Constant::BigInt(digits.clone()),
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

/// Module-scope names whose value can change after the module body has run.
///
/// A `let` or `var` declaration, or any name something assigns to or updates -- including inside
/// a nested function, which is the case that matters: `export let count` incremented by an
/// exported `bump()` is the whole reason a live binding is not the same as a snapshot.
/// Whether each `using` declaration written directly in this statement list is awaited.
///
/// Only this level: a `using` inside a nested block belongs to that block's scope and is
/// disposed of when *it* is left, which is what makes the disposal order the reverse of the
/// declaration order rather than of the execution order.
fn resource_declarations(list: &[Statement]) -> Vec<bool> {
    let mut resources = Vec::new();
    for statement in list {
        if let StatementKind::Variable { kind, declarations } = &statement.kind
            && let Some(awaiting) = kind.disposal()
        {
            resources.extend(std::iter::repeat_n(awaiting, declarations.len()));
        }
    }
    resources
}

fn mutable_module_bindings(body: &[Statement]) -> HashSet<String> {
    let mut names = HashSet::new();
    for statement in body {
        if let StatementKind::Variable { kind, declarations } = &statement.kind
            && *kind != VariableKind::Const
        {
            for declaration in declarations {
                collect_pattern_names(&declaration.pattern, &mut names);
            }
        }
        collect_assigned_names(statement, &mut names);
    }
    names
}

/// Every name assigned or updated anywhere in `statement`, however deeply nested.
///
/// Deliberately an over-approximation, like the capture analysis: a name boxed that never changes
/// costs one pointer chase, where a name *missed* would read as a stale value through an import.
fn collect_assigned_names(statement: &Statement, names: &mut HashSet<String>) {
    let mut expressions: Vec<&Expression> = Vec::new();
    let mut statements: Vec<&Statement> = vec![statement];
    while let Some(statement) = statements.pop() {
        match &statement.kind {
            StatementKind::Variable { declarations, .. } => {
                for declaration in declarations {
                    if let Some(initializer) = &declaration.initializer {
                        expressions.push(initializer);
                    }
                }
            }
            StatementKind::Function(function) => statements.extend(function.body.iter()),
            StatementKind::Class(class) => {
                for method in &class.methods {
                    statements.extend(method.function.body.iter());
                }
                if let Some(constructor) = &class.constructor {
                    statements.extend(constructor.body.iter());
                }
            }
            StatementKind::Expression(expression)
            | StatementKind::Throw(expression)
            | StatementKind::ExportDefault(expression) => expressions.push(expression),
            StatementKind::Return(value) => expressions.extend(value.iter()),
            StatementKind::Block(body) => statements.extend(body.iter()),
            StatementKind::If {
                test,
                consequent,
                alternate,
            } => {
                expressions.push(test);
                statements.push(consequent);
                statements.extend(alternate.iter().map(std::convert::AsRef::as_ref));
            }
            StatementKind::While { test, body } | StatementKind::DoWhile { test, body } => {
                expressions.push(test);
                statements.push(body);
            }
            StatementKind::For {
                init,
                test,
                update,
                body,
            } => {
                statements.extend(init.iter().map(std::convert::AsRef::as_ref));
                expressions.extend(test.iter());
                expressions.extend(update.iter());
                statements.push(body);
            }
            StatementKind::ForOf { iterable, body, .. } => {
                expressions.push(iterable);
                statements.push(body);
            }
            StatementKind::ForIn { object, body, .. } => {
                expressions.push(object);
                statements.push(body);
            }
            StatementKind::Labelled { body, .. } => statements.push(body),
            StatementKind::With { object, body } => {
                expressions.push(object);
                statements.push(body);
            }
            StatementKind::Switch {
                discriminant,
                cases,
            } => {
                expressions.push(discriminant);
                for case in cases {
                    expressions.extend(case.test.iter());
                    statements.extend(case.body.iter());
                }
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                statements.extend(block.iter());
                if let Some(handler) = handler {
                    statements.extend(handler.body.iter());
                }
                statements.extend(finalizer.iter());
            }
            StatementKind::Break { .. }
            | StatementKind::Continue { .. }
            | StatementKind::Empty
            | StatementKind::Unsupported { .. } => {}
        }
    }
    for expression in expressions {
        collect_identifiers(expression, names, true);
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
        // A parameter default runs in this function's own scope. Ordinary identifiers in it may
        // therefore refer outward and must be captured by this function itself, not only when a
        // second nested function appears inside the pattern. `path-to-regexp` relies on exactly
        // this shape: `function match(path, { delimiter = DEFAULT_DELIMITER } = {})`.
        collect_pattern_identifiers(&parameter.pattern, &mut captured);
    }
    if let Some(rest) = &function.rest {
        collect_pattern_identifiers(rest, &mut captured);
    }
    captured
}

/// Whether `body` contains a syntactically direct `eval(...)` call.
///
/// This deliberately looks through nested functions. An eval in a nested function can still name
/// a binding owned by any enclosing function, so every enclosing level has to box its bindings as
/// well. Shadowing is left to normal resolution when the call itself is compiled; treating a
/// shadowed `eval` as direct here merely boxes a few extra locals and cannot change their values.
fn body_contains_direct_eval(body: &[Statement]) -> bool {
    body.iter().any(statement_contains_direct_eval)
}

fn function_contains_direct_eval(function: &Function) -> bool {
    function
        .parameters
        .iter()
        .any(|parameter| pattern_contains_direct_eval(&parameter.pattern))
        || function
            .rest
            .as_ref()
            .is_some_and(pattern_contains_direct_eval)
        || body_contains_direct_eval(&function.body)
}

fn statement_contains_direct_eval(statement: &Statement) -> bool {
    match &statement.kind {
        StatementKind::Variable { declarations, .. } => declarations.iter().any(|declaration| {
            pattern_contains_direct_eval(&declaration.pattern)
                || declaration
                    .initializer
                    .as_ref()
                    .is_some_and(expression_contains_direct_eval)
        }),
        StatementKind::Function(function) => function_contains_direct_eval(function),
        StatementKind::Class(class) => class_contains_direct_eval(class),
        StatementKind::Expression(expression)
        | StatementKind::Throw(expression)
        | StatementKind::ExportDefault(expression) => expression_contains_direct_eval(expression),
        StatementKind::Return(value) => value.as_ref().is_some_and(expression_contains_direct_eval),
        StatementKind::Block(body) => body_contains_direct_eval(body),
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            expression_contains_direct_eval(test)
                || statement_contains_direct_eval(consequent)
                || alternate
                    .as_deref()
                    .is_some_and(statement_contains_direct_eval)
        }
        StatementKind::While { test, body } | StatementKind::DoWhile { test, body } => {
            expression_contains_direct_eval(test) || statement_contains_direct_eval(body)
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            init.as_deref().is_some_and(statement_contains_direct_eval)
                || test.as_ref().is_some_and(expression_contains_direct_eval)
                || update.as_ref().is_some_and(expression_contains_direct_eval)
                || statement_contains_direct_eval(body)
        }
        StatementKind::ForOf { iterable, body, .. } => {
            expression_contains_direct_eval(iterable) || statement_contains_direct_eval(body)
        }
        StatementKind::ForIn { object, body, .. } => {
            expression_contains_direct_eval(object) || statement_contains_direct_eval(body)
        }
        StatementKind::Labelled { body, .. } => statement_contains_direct_eval(body),
        StatementKind::With { object, body } => {
            expression_contains_direct_eval(object) || statement_contains_direct_eval(body)
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            expression_contains_direct_eval(discriminant)
                || cases.iter().any(|case| {
                    case.test
                        .as_ref()
                        .is_some_and(expression_contains_direct_eval)
                        || body_contains_direct_eval(&case.body)
                })
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            body_contains_direct_eval(block)
                || handler
                    .as_ref()
                    .is_some_and(|handler| body_contains_direct_eval(&handler.body))
                || body_contains_direct_eval(finalizer)
        }
        StatementKind::Break { .. }
        | StatementKind::Continue { .. }
        | StatementKind::Empty
        | StatementKind::Unsupported { .. } => false,
    }
}

fn class_contains_direct_eval(class: &Class) -> bool {
    class
        .superclass
        .as_ref()
        .is_some_and(expression_contains_direct_eval)
        || class
            .constructor
            .as_ref()
            .is_some_and(function_contains_direct_eval)
        || class
            .methods
            .iter()
            .any(|method| function_contains_direct_eval(&method.function))
        || class.fields.iter().any(|field| {
            matches!(&field.key, ObjectKey::Computed(key) if expression_contains_direct_eval(key))
                || field
                    .value
                    .as_ref()
                    .is_some_and(expression_contains_direct_eval)
        })
}

fn pattern_contains_direct_eval(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Identifier(_) => false,
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            expression_contains_direct_eval(object)
        }
        Pattern::Computed { object, property } => {
            expression_contains_direct_eval(object) || expression_contains_direct_eval(property)
        }
        Pattern::WithDefault { inner, default } => {
            pattern_contains_direct_eval(inner) || expression_contains_direct_eval(default)
        }
        Pattern::Object { properties, rest } => {
            properties.iter().any(|property| {
                matches!(&property.key, PatternKey::Computed(key) if expression_contains_direct_eval(key))
                    || pattern_contains_direct_eval(&property.value)
            }) || rest
                .as_deref()
                .is_some_and(pattern_contains_direct_eval)
        }
        Pattern::Array { elements, rest } => {
            elements
                .iter()
                .flatten()
                .any(pattern_contains_direct_eval)
                || rest
                    .as_deref()
                    .is_some_and(pattern_contains_direct_eval)
        }
    }
}

fn expression_contains_direct_eval(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Call {
            callee,
            arguments,
            optional,
        } => {
            (!optional
                && matches!(&callee.kind, ExpressionKind::Identifier(name) if name == "eval"))
                || expression_contains_direct_eval(callee)
                || arguments.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::Function(function) => function_contains_direct_eval(function),
        ExpressionKind::Class(class) => class_contains_direct_eval(class),
        ExpressionKind::ImportCall { specifier, options } => {
            expression_contains_direct_eval(specifier)
                || options
                    .as_deref()
                    .is_some_and(expression_contains_direct_eval)
        }
        ExpressionKind::Unary { argument, .. }
        | ExpressionKind::Chain {
            expression: argument,
        }
        | ExpressionKind::Await(argument)
        | ExpressionKind::PrivateIn {
            object: argument, ..
        }
        | ExpressionKind::PrivateMember {
            object: argument, ..
        }
        | ExpressionKind::StaticMember {
            object: argument, ..
        } => expression_contains_direct_eval(argument),
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            expression_contains_direct_eval(left) || expression_contains_direct_eval(right)
        }
        ExpressionKind::Assignment { value, .. } => expression_contains_direct_eval(value),
        ExpressionKind::PatternAssignment { pattern, value } => {
            pattern_contains_direct_eval(pattern) || expression_contains_direct_eval(value)
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expression_contains_direct_eval(test)
                || expression_contains_direct_eval(consequent)
                || expression_contains_direct_eval(alternate)
        }
        ExpressionKind::New { callee, arguments } => {
            expression_contains_direct_eval(callee)
                || arguments.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::SuperCall { arguments } => {
            arguments.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::Array { elements } => {
            elements.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::Sequence { operands } => {
            operands.iter().any(expression_contains_direct_eval)
        }
        ExpressionKind::Yield { value, .. } => value
            .as_ref()
            .is_some_and(|value| expression_contains_direct_eval(value)),
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
            expression_contains_direct_eval(object) || expression_contains_direct_eval(property)
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            expression_contains_direct_eval(object)
                || expression_contains_direct_eval(property)
                || expression_contains_direct_eval(value)
        }
        ExpressionKind::MemberAssignment { object, value, .. }
        | ExpressionKind::PrivateAssignment { object, value, .. } => {
            expression_contains_direct_eval(object) || expression_contains_direct_eval(value)
        }
        ExpressionKind::UpdateMember {
            object, property, ..
        }
        | ExpressionKind::Delete { object, property } => {
            expression_contains_direct_eval(object)
                || matches!(property.as_ref(), ObjectKey::Computed(key) if expression_contains_direct_eval(key))
        }
        ExpressionKind::Template { expressions, .. } => {
            expressions.iter().any(expression_contains_direct_eval)
        }
        ExpressionKind::Object { members } => members.iter().any(|member| match member {
            ObjectMember::Property(property) => {
                matches!(&property.key, ObjectKey::Computed(key) if expression_contains_direct_eval(key))
                    || expression_contains_direct_eval(&property.value)
            }
            ObjectMember::Spread(source) => expression_contains_direct_eval(source),
            ObjectMember::Accessor { key, function, .. } => {
                matches!(key, ObjectKey::Computed(key) if expression_contains_direct_eval(key))
                    || function_contains_direct_eval(function)
            }
        }),
        ExpressionKind::Identifier(_)
        | ExpressionKind::Update { .. }
        | ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::NewTarget
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::RegExp { .. }
        | ExpressionKind::TemplateStrings { .. }
        | ExpressionKind::Unsupported { .. } => false,
    }
}

fn list_element_contains_direct_eval(element: &ListElement) -> bool {
    match element {
        ListElement::Item(value) | ListElement::Spread(value) => {
            expression_contains_direct_eval(value)
        }
        ListElement::Hole => false,
    }
}

/// Names that nested functions inside a pattern's expressions refer to.
///
/// A pattern reads names as well as binding them: a default, a computed key, and the object a
/// reference target stores through are all ordinary expressions, and a closure in any of them
/// decides whether the name it reaches has to be boxed.
fn collect_pattern_captures(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier(_) => {}
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            collect_identifiers(object, names, false);
        }
        Pattern::Computed { object, property } => {
            collect_identifiers(object, names, false);
            collect_identifiers(property, names, false);
        }
        Pattern::WithDefault { inner, default } => {
            collect_identifiers(default, names, false);
            collect_pattern_captures(inner, names);
        }
        Pattern::Object { properties, rest } => {
            for property in properties {
                if let PatternKey::Computed(key) = &property.key {
                    collect_identifiers(key, names, false);
                }
                collect_pattern_captures(&property.value, names);
            }
            if let Some(rest) = rest {
                collect_pattern_captures(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                collect_pattern_captures(element, names);
            }
            if let Some(rest) = rest {
                collect_pattern_captures(rest, names);
            }
        }
    }
}

/// Every identifier evaluated while a pattern is initialised.
///
/// Unlike [`collect_pattern_captures`], this starts in collecting mode: the pattern itself is
/// already inside a nested function relative to its owner. A function parameter default such as
/// `function f(x = f.value) {}` therefore captures the outer `f` binding even though the
/// function body never mentions it.
fn collect_pattern_identifiers(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier(_) => {}
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            collect_identifiers(object, names, true);
        }
        Pattern::Computed { object, property } => {
            collect_identifiers(object, names, true);
            collect_identifiers(property, names, true);
        }
        Pattern::WithDefault { inner, default } => {
            collect_identifiers(default, names, true);
            collect_pattern_identifiers(inner, names);
        }
        Pattern::Object { properties, rest } => {
            for property in properties {
                if let PatternKey::Computed(key) = &property.key {
                    collect_identifiers(key, names, true);
                }
                // `{ delimiter = DEFAULT_DELIMITER }` stores the default on the property value
                // pattern, so recurse into that pattern's expressions as well as its computed
                // key. Merely walking the bound name misses the initializer entirely.
                collect_pattern_identifiers(&property.value, names);
            }
            if let Some(rest) = rest {
                collect_pattern_identifiers(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                collect_pattern_identifiers(element, names);
            }
            if let Some(rest) = rest {
                collect_pattern_identifiers(rest, names);
            }
        }
    }
}

/// Whether a target stores through a property reference rather than binding a name.
fn stores_through_reference(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Member { .. } | Pattern::Computed { .. } | Pattern::PrivateMember { .. } => true,
        Pattern::WithDefault { inner, .. } => stores_through_reference(inner),
        _ => false,
    }
}

fn collect_nested_function_names(
    statement: &Statement,
    names: &mut HashSet<String>,
    inside_function: bool,
) {
    match &statement.kind {
        StatementKind::Function(function) => {
            for parameter in &function.parameters {
                collect_pattern_identifiers(&parameter.pattern, names);
            }
            if let Some(rest) = &function.rest {
                collect_pattern_identifiers(rest, names);
            }
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
        StatementKind::With { object, body } => {
            collect_identifiers(object, names, inside_function);
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
        for parameter in &function.parameters {
            collect_pattern_identifiers(&parameter.pattern, names);
        }
        if let Some(rest) = &function.rest {
            collect_pattern_identifiers(rest, names);
        }
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
        AssignmentOperator::BitwiseAnd => BinaryOp::BitwiseAnd,
        AssignmentOperator::BitwiseOr => BinaryOp::BitwiseOr,
        AssignmentOperator::BitwiseXor => BinaryOp::BitwiseXor,
        AssignmentOperator::ShiftLeft => BinaryOp::ShiftLeft,
        AssignmentOperator::ShiftRight => BinaryOp::ShiftRight,
        AssignmentOperator::UnsignedShiftRight => BinaryOp::UnsignedShiftRight,
        AssignmentOperator::Assign => unreachable!("a plain assignment performs no operation"),
        AssignmentOperator::LogicalAnd
        | AssignmentOperator::LogicalOr
        | AssignmentOperator::Nullish => {
            unreachable!("a short-circuiting assignment computes nothing")
        }
    }
}

/// The jump that skips the store for a short-circuiting assignment.
///
/// `a ||= b` assigns only when `a` is falsy, so the jump taken *past* the store is the one taken
/// when `a` is truthy -- which is why each operator maps to the opposite test.
fn short_circuit_jump(operator: AssignmentOperator) -> Instruction {
    match operator {
        AssignmentOperator::LogicalAnd => Instruction::JumpIfFalse(0),
        AssignmentOperator::LogicalOr => Instruction::JumpIfTruthy(0),
        AssignmentOperator::Nullish => Instruction::JumpIfNotNullish(0),
        _ => unreachable!("only a short-circuiting assignment skips its store"),
    }
}

/// The reserved property name a private member is stored under.
///
/// Reserved rather than ordinary, so nothing that enumerates own string properties can see it and
/// no computed access can spell it -- `obj["#x"]` names a different property, as it should.
fn private_key(scope: u32, name: &str) -> String {
    format!("@@private:{scope}:{name}")
}

/// The names a pattern binds, in source order.
///
/// Ordered, unlike [`collect_pattern_names`], because these decide slot numbering and the
/// bytecode is a cache key: the same source has to produce the same instructions, and a hash set
/// does not promise that.
fn pattern_names_in_order(pattern: &Pattern, names: &mut Vec<String>) {
    match pattern {
        Pattern::Identifier(name) => names.push(name.clone()),
        // A reference target binds nothing: `[o.p] = xs` stores into an object that already
        // exists, and `o` was declared somewhere else.
        Pattern::Member { .. } | Pattern::Computed { .. } | Pattern::PrivateMember { .. } => {}
        Pattern::WithDefault { inner, .. } => pattern_names_in_order(inner, names),
        Pattern::Object { properties, rest } => {
            for property in properties {
                pattern_names_in_order(&property.value, names);
            }
            if let Some(rest) = rest {
                pattern_names_in_order(rest, names);
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
        Pattern::Member { .. } | Pattern::Computed { .. } | Pattern::PrivateMember { .. } => {}
        Pattern::WithDefault { inner, .. } => collect_pattern_names(inner, names),
        Pattern::Object { properties, rest } => {
            for property in properties {
                collect_pattern_names(&property.value, names);
            }
            if let Some(rest) = rest {
                collect_pattern_names(rest, names);
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
            for parameter in &function.parameters {
                collect_pattern_identifiers(&parameter.pattern, names);
            }
            if let Some(rest) = &function.rest {
                collect_pattern_identifiers(rest, names);
            }
            for statement in &function.body {
                // A function expression always establishes a nested function relative to the
                // expression that contains it. This must not inherit `collecting`: at module
                // level that flag is false by design, yet
                // `Layer.prototype.match = function match () { return MODULE_CONST }` still
                // closes over the module. Inheriting false emitted a `LoadGlobal` that later
                // indexed whichever CommonJS module called the exported method.
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
        ExpressionKind::PrivateIn { object, .. } => {
            collect_identifiers(object, names, collecting);
        }
        ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::NewTarget
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::Unsupported { .. } => {}
        ExpressionKind::SuperCall { arguments } => {
            for argument in arguments {
                let (ListElement::Item(value) | ListElement::Spread(value)) = argument else {
                    continue;
                };
                collect_identifiers(value, names, collecting);
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
        ExpressionKind::ImportCall { specifier, options } => {
            collect_identifiers(specifier, names, collecting);
            if let Some(options) = options {
                collect_identifiers(options, names, collecting);
            }
        }
        // Every name the pattern assigns is written by this function, so each is a capture
        // candidate for the same reason a plain assignment target is.
        ExpressionKind::PatternAssignment { pattern, value } => {
            if collecting {
                collect_pattern_names(pattern, names);
            }
            collect_pattern_captures(pattern, names);
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
        ExpressionKind::Call {
            callee, arguments, ..
        }
        | ExpressionKind::New { callee, arguments } => {
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
            // The value is already inside this property definition. Once the collector
            // is walking a function body, a function expression assigned here remains
            // nested relative to that owner. Losing that state is what left module
            // bindings unboxed in `Layer.prototype.match = function match (...) {}`.
            collect_identifiers(value, names, true);
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
        ExpressionKind::UpdateMember {
            object, property, ..
        } => {
            collect_identifiers(object, names, collecting);
            if let ObjectKey::Computed(key) = property.as_ref() {
                collect_identifiers(key, names, collecting);
            }
        }
        // A template object is built from constants; it names nothing.
        ExpressionKind::TemplateStrings { .. } => {}
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
                        // Property values are expressions in this lexical region. For capture
                        // analysis the important case is a function value: its body is nested
                        // relative to the current owner even when the surrounding expression is
                        // itself already inside another function.
                        collect_identifiers(&property.value, names, true);
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
#[cfg(test)]
mod tests {
    use super::*;

    fn module(body: Vec<Statement>) -> Module {
        Module {
            strict: false,
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
            strict: false,
            binds_own_name: false,
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

    fn call(callee: Expression, arguments: Vec<Expression>) -> Expression {
        Expression {
            kind: ExpressionKind::Call {
                callee: Box::new(callee),
                arguments: arguments.into_iter().map(ListElement::Item).collect(),
                optional: false,
            },
            span: Span::default(),
        }
    }

    fn declaration(kind: VariableKind, name: &str, initializer: Option<Expression>) -> Statement {
        statement(StatementKind::Variable {
            kind,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: Pattern::Identifier(name.to_owned()),
                initializer,
                span: Span::default(),
            }],
        })
    }

    fn string(text: &str) -> Expression {
        Expression {
            kind: ExpressionKind::Literal(Literal::String(text.to_owned())),
            span: Span::default(),
        }
    }

    /// A direct `eval` hands its caller's bindings over as cells, not as slot numbers.
    ///
    /// The eval'd source is compiled separately, so a slot number would mean nothing to it. A
    /// cell is the one storage two independently compiled programs can share -- and sharing is
    /// the requirement, because `eval("x = 2")` has to be visible to the caller afterwards.
    #[test]
    fn a_direct_eval_carries_the_bindings_around_it() {
        // function f() { var x = 1; eval("x"); }
        let program = compile(&module(vec![function(
            "f",
            &[],
            vec![
                declaration(VariableKind::Var, "x", Some(number(1.0))),
                statement(StatementKind::Expression(call(
                    identifier("eval"),
                    vec![string("x")],
                ))),
            ],
        )]))
        .expect("the module compiles");
        let body = &program.functions[0];
        let site = body
            .code
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::DirectEval(site) => Some(site),
                _ => None,
            })
            .expect("the call lowered to a direct eval");
        let binding = site
            .bindings
            .iter()
            .find(|binding| binding.name == "x")
            .expect("`x` is in scope at the call");
        assert!(
            matches!(binding.access, EvalAccess::Cell(_)),
            "the caller's binding travels as a cell: {:?}",
            binding.access
        );
        assert!(
            body.code.contains(&Instruction::NewCell(0)),
            "and the function boxed it, which is what makes the cell exist"
        );
    }

    /// The caller's names occupy the eval program's first slots, in the order they were handed
    /// over. That ordering *is* the contract between the compiler and the machine that installs
    /// the cells; nothing else records which slot a name landed in.
    #[test]
    fn eval_code_binds_the_callers_names_to_its_first_slots() {
        let program = compile_eval(
            &module(vec![statement(StatementKind::Expression(identifier("x")))]),
            &EvalOptions {
                outer: vec![EvalOuter {
                    name: "x".to_owned(),
                    declaration: EvalDeclaration::Var,
                }],
                strict: false,
            },
        )
        .expect("the eval source compiles");
        assert!(
            program.code.contains(&Instruction::LoadCell(0)),
            "`x` reads the cell installed in slot 0: {:?}",
            program.code
        );
        assert!(
            matches!(program.code.last(), Some(Instruction::Return)),
            "and the program ends by returning its completion value"
        );
    }

    /// Strictness is inherited into every nested body, and a class body raises it.
    ///
    /// The store instructions carry no strictness of their own, so the interpreter reads it off
    /// the body a write was written in. That table is only right if the compiler propagates the
    /// flag the way the language does: outward-in for a `"use strict"` prologue, and
    /// unconditionally through a class -- whose methods are strict in the sloppiest of files.
    #[test]
    fn strictness_reaches_every_body_written_inside_a_strict_one() {
        let mut inner = kiln_hir::Function {
            name: Some("inner".to_owned()),
            binds_own_name: true,
            arrow: false,
            generator: false,
            asynchronous: false,
            parameters: Vec::new(),
            rest: None,
            body: Vec::new(),
            strict: false,
            span: Span::default(),
        };
        let mut outer = inner.clone();
        outer.name = Some("outer".to_owned());
        outer.strict = true;
        outer.body = vec![statement(StatementKind::Function(Box::new(inner.clone())))];

        let (_, strictness) = compile_with_strictness(
            &module(vec![statement(StatementKind::Function(Box::new(outer)))]),
            CompileOptions::default(),
        )
        .expect("the program compiles");
        assert_eq!(
            strictness.functions,
            vec![true, true],
            "the nested body is strict because the one around it said so"
        );
        assert!(!strictness.body, "and the file around them both is not");

        inner.name = Some("m".to_owned());
        let class = kiln_hir::Class {
            name: Some("C".to_owned()),
            superclass: None,
            constructor: None,
            methods: vec![kiln_hir::ClassMethod {
                key: ObjectKey::Static("m".to_owned()),
                function: inner,
                is_static: false,
                kind: kiln_hir::MethodKind::Normal,
            }],
            fields: Vec::new(),
            span: Span::default(),
        };
        let (_, strictness) = compile_with_strictness(
            &module(vec![statement(StatementKind::Class(Box::new(class)))]),
            CompileOptions::default(),
        )
        .expect("the class compiles");
        assert!(
            strictness.functions.iter().all(|strict| *strict),
            "a class body is strict code whatever encloses it: {:?}",
            strictness.functions
        );
    }

    #[test]
    fn property_mutations_carry_their_local_strictness() {
        let assignment = || {
            statement(StatementKind::Expression(Expression {
                kind: ExpressionKind::MemberAssignment {
                    operator: AssignmentOperator::Assign,
                    object: Box::new(Expression {
                        kind: ExpressionKind::Object {
                            members: Vec::new(),
                        },
                        span: Span::default(),
                    }),
                    property: "answer".into(),
                    value: Box::new(number(42.0)),
                },
                span: Span::default(),
            }))
        };
        let mut strict_module = module(vec![assignment()]);
        strict_module.strict = true;
        let strict = compile(&strict_module).expect("strict module compiles");
        assert!(strict.code.iter().any(|instruction| matches!(
            instruction,
            Instruction::SetPropertyStrict { strict: true, .. }
        )));

        let sloppy = compile_commonjs(&module(vec![assignment()])).expect("CommonJS compiles");
        assert!(sloppy.program.code.iter().any(|instruction| matches!(
            instruction,
            Instruction::SetPropertyStrict { strict: false, .. }
        )));
    }

    #[test]
    fn commonjs_var_may_redeclare_a_wrapper_parameter() {
        let program = compile_commonjs(&module(vec![declaration(
            VariableKind::Var,
            "module",
            Some(number(42.0)),
        )]))
        .expect("CommonJS wrapper parameters have function-parameter var semantics");

        assert!(program.program.code.contains(&Instruction::StoreCell(2)));
    }

    /// Strictness decides whose binding a `var` in eval code is.
    ///
    /// A sloppy direct eval declares into the caller's variable environment, so `var x` over a
    /// name the caller already has *is* that binding. Strict eval code gets a scope of its own,
    /// so the same declaration shadows it -- and writing one must not reach the caller's.
    #[test]
    fn a_sloppy_eval_var_takes_the_callers_binding_and_a_strict_one_shadows_it() {
        let source = || module(vec![declaration(VariableKind::Var, "x", Some(number(2.0)))]);
        let outer = || {
            vec![EvalOuter {
                name: "x".to_owned(),
                declaration: EvalDeclaration::Var,
            }]
        };
        let sloppy = compile_eval(
            &source(),
            &EvalOptions {
                outer: outer(),
                strict: false,
            },
        )
        .expect("the sloppy eval compiles");
        assert!(
            sloppy.code.contains(&Instruction::StoreCell(0)),
            "the sloppy `var` writes the caller's cell: {:?}",
            sloppy.code
        );
        let strict = compile_eval(
            &source(),
            &EvalOptions {
                outer: outer(),
                strict: true,
            },
        )
        .expect("the strict eval compiles");
        assert!(
            !strict.code.contains(&Instruction::StoreCell(0)),
            "the strict `var` gets a slot of its own instead: {:?}",
            strict.code
        );
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
                strict: false,
                binds_own_name: false,
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
                            form: kiln_hir::PropertyForm::KeyValue,
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
                            form: kiln_hir::PropertyForm::KeyValue,
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

    #[test]
    fn a_captured_defaulted_identifier_parameter_uses_its_positional_cell() {
        let default = Expression {
            kind: ExpressionKind::StaticMember {
                object: Box::new(identifier("factory")),
                property: "fallback".to_owned(),
                optional: false,
            },
            span: Span::default(),
        };
        let function = statement(StatementKind::Function(Box::new(kiln_hir::Function {
            strict: false,
            binds_own_name: false,
            asynchronous: false,
            name: Some("factory".to_owned()),
            arrow: false,
            generator: false,
            parameters: vec![kiln_hir::Parameter {
                pattern: Pattern::WithDefault {
                    inner: Box::new(Pattern::Identifier("value".to_owned())),
                    default: Box::new(default),
                },
                span: Span::default(),
            }],
            rest: None,
            body: vec![statement(StatementKind::Return(Some(arrow(
                &[],
                vec![statement(StatementKind::Return(Some(identifier("value"))))],
            ))))],
            span: Span::default(),
        })));
        let program = compile(&module(vec![function])).expect("defaulted parameter compiles");
        let factory = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("factory"))
            .expect("factory proto");
        assert!(factory.code.contains(&Instruction::LoadCell(0)));
        assert!(factory.code.contains(&Instruction::StoreCell(0)));
        assert_eq!(
            factory
                .code
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::LoadGlobal(_)))
                .count(),
            0,
            "the function's module binding is captured, never read from the caller's module"
        );
        assert!(factory.code.contains(&Instruction::LoadUpvalue(0)));
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
                    optional: false,
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

    /// A function reads a module binding through a captured cell, never through its own slots.
    ///
    /// It used to read one through `LoadGlobal`, against the running module's frame. That is
    /// right while the function stays home and wrong the moment it is exported: the frame found
    /// belongs to the importer. A cell belongs to the binding, so it travels with the closure.
    /// What must never appear is `LoadLocal`, which would read the callee frame's own slot.
    #[test]
    fn a_function_reads_module_scope_through_a_captured_cell() {
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
                .any(|instruction| matches!(instruction, Instruction::LoadUpvalue(_))),
            "a module binding a function reads has to reach it through a cell"
        );
        assert!(
            !program.functions[0]
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadLocal(_))),
            "module reads must not compile to LoadLocal, which would read the frame's own slot"
        );
        // The cell is made at module scope, before anything can capture it.
        assert!(program.code.contains(&Instruction::NewCell(0)));
    }

    /// A function expression assigned to a prototype is still nested in the surrounding module.
    ///
    /// This is the shape used by Express' router (`Layer.prototype.match = function match (...)`)
    /// and it once lost the collector's `inside_function` state while walking the assignment.
    /// The method consequently emitted `LoadGlobal`, which indexed the importer's module frame
    /// when the function crossed the CommonJS boundary.
    #[test]
    fn a_prototype_assigned_function_captures_its_module_binding() {
        let assigned = Expression {
            kind: ExpressionKind::MemberAssignment {
                operator: AssignmentOperator::Assign,
                object: Box::new(identifier("prototype")),
                property: "match".to_owned(),
                value: Box::new(Expression {
                    kind: ExpressionKind::Function(Box::new(kiln_hir::Function {
                        strict: false,
                        binds_own_name: true,
                        asynchronous: false,
                        name: Some("match".to_owned()),
                        arrow: false,
                        generator: false,
                        parameters: Vec::new(),
                        rest: None,
                        body: vec![statement(StatementKind::Return(Some(identifier(
                            "module_constant",
                        ))))],
                        span: Span::default(),
                    })),
                    span: Span::default(),
                }),
            },
            span: Span::default(),
        };
        let program = compile(&module(vec![
            declaration(VariableKind::Const, "module_constant", Some(number(1.0))),
            declaration(
                VariableKind::Const,
                "prototype",
                Some(Expression {
                    kind: ExpressionKind::Object {
                        members: Vec::new(),
                    },
                    span: Span::default(),
                }),
            ),
            statement(StatementKind::Expression(assigned)),
        ]))
        .expect("prototype assignment compiles");
        let method = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("match"))
            .expect("assigned function proto");
        assert!(method.code.contains(&Instruction::LoadUpvalue(0)));
        assert!(
            !method
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
            "an exported method must carry the module binding it reads: {:?}",
            method.code
        );
        assert!(program.code.contains(&Instruction::NewCell(0)));
    }

    #[test]
    fn a_function_does_not_capture_a_local_it_declares_itself() {
        let program = compile(&module(vec![
            declaration(VariableKind::Const, "DEFAULT_DELIMITER", Some(string("/"))),
            function(
                "uses_default",
                &[],
                vec![
                    declaration(
                        VariableKind::Const,
                        "delimiter",
                        Some(identifier("DEFAULT_DELIMITER")),
                    ),
                    statement(StatementKind::Return(Some(identifier("delimiter")))),
                ],
            ),
        ]))
        .expect("function with a local compiles");
        let body = &program.functions[0];
        assert!(
            body.code.contains(&Instruction::LoadUpvalue(0)),
            "the outer module binding is the capture: {:?}",
            body.code
        );
        assert!(
            !body.code.iter().any(|instruction| matches!(
                instruction,
                Instruction::LoadGlobal(_) | Instruction::StoreGlobal(_)
            )),
            "the function's own local must not force caller-relative module reads: {:?}",
            body.code
        );
    }

    #[test]
    fn a_class_method_default_captures_its_module_binding() {
        let method = kiln_hir::Function {
            strict: false,
            binds_own_name: false,
            asynchronous: false,
            name: Some("read".to_owned()),
            arrow: false,
            generator: false,
            parameters: vec![kiln_hir::Parameter {
                pattern: Pattern::WithDefault {
                    inner: Box::new(Pattern::Identifier("value".to_owned())),
                    default: Box::new(identifier("shared")),
                },
                span: Span::default(),
            }],
            rest: None,
            body: vec![statement(StatementKind::Return(Some(identifier("value"))))],
            span: Span::default(),
        };
        let program = compile(&module(vec![
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: Pattern::Identifier("shared".to_owned()),
                    initializer: Some(number(1.0)),
                    span: Span::default(),
                }],
            }),
            statement(StatementKind::Class(Box::new(kiln_hir::Class {
                name: Some("Reader".to_owned()),
                superclass: None,
                constructor: None,
                methods: vec![kiln_hir::ClassMethod {
                    key: kiln_hir::ObjectKey::Static("read".to_owned()),
                    function: method,
                    is_static: false,
                    kind: kiln_hir::MethodKind::Normal,
                }],
                fields: Vec::new(),
                span: Span::default(),
            }))),
        ]))
        .expect("class method compiles");
        let read = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("read"))
            .expect("method proto");
        assert!(read.code.contains(&Instruction::LoadUpvalue(0)));
        assert!(
            !read
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
            "an exported method must carry its module binding instead of indexing its caller"
        );
        assert!(program.code.contains(&Instruction::NewCell(0)));
    }

    #[test]
    fn a_class_constructor_inside_a_wrapper_captures_the_wrapper_binding() {
        let constructor = kiln_hir::Function {
            strict: false,
            binds_own_name: false,
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            parameters: vec![kiln_hir::Parameter {
                pattern: Pattern::Identifier("value".to_owned()),
                span: Span::default(),
            }],
            rest: None,
            body: vec![statement(StatementKind::Return(Some(Expression {
                kind: ExpressionKind::StaticMember {
                    object: Box::new(identifier("exports")),
                    property: "IDENTIFIER".to_owned(),
                    optional: false,
                },
                span: Span::default(),
            })))],
            span: Span::default(),
        };
        let wrapper = kiln_hir::Function {
            strict: false,
            binds_own_name: false,
            asynchronous: false,
            name: Some("wrapper".to_owned()),
            arrow: false,
            generator: false,
            parameters: vec![kiln_hir::Parameter {
                pattern: Pattern::Identifier("exports".to_owned()),
                span: Span::default(),
            }],
            rest: None,
            body: vec![statement(StatementKind::Class(Box::new(kiln_hir::Class {
                name: Some("Name".to_owned()),
                superclass: None,
                constructor: Some(constructor),
                methods: Vec::new(),
                fields: Vec::new(),
                span: Span::default(),
            })))],
            span: Span::default(),
        };
        let program = compile(&module(vec![statement(StatementKind::Function(Box::new(
            wrapper,
        )))]))
        .expect("CommonJS-shaped wrapper compiles");
        let wrapper = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("wrapper"))
            .expect("wrapper proto");
        assert!(
            wrapper.code.contains(&Instruction::NewCell(0)),
            "the wrapper parameter must be boxed for the nested class constructor"
        );
        let constructor = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("Name"))
            .expect("class constructor proto");
        assert!(constructor.code.contains(&Instruction::LoadUpvalue(0)));
        assert!(
            !constructor
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
            "the constructor must carry the wrapper binding across module calls"
        );
    }

    #[test]
    fn an_exported_derived_constructor_captures_its_hidden_super_binding() {
        let program = compile(&module(vec![
            statement(StatementKind::Class(Box::new(kiln_hir::Class {
                name: Some("Base".to_owned()),
                superclass: None,
                constructor: None,
                methods: Vec::new(),
                fields: Vec::new(),
                span: Span::default(),
            }))),
            statement(StatementKind::Class(Box::new(kiln_hir::Class {
                name: Some("Derived".to_owned()),
                superclass: Some(identifier("Base")),
                constructor: Some(kiln_hir::Function {
                    strict: false,
                    binds_own_name: false,
                    asynchronous: false,
                    name: None,
                    arrow: false,
                    generator: false,
                    parameters: Vec::new(),
                    rest: None,
                    body: vec![statement(StatementKind::Expression(Expression {
                        kind: ExpressionKind::SuperCall {
                            arguments: Vec::new(),
                        },
                        span: Span::default(),
                    }))],
                    span: Span::default(),
                }),
                methods: Vec::new(),
                fields: Vec::new(),
                span: Span::default(),
            }))),
        ]))
        .expect("derived class compiles");
        let derived = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("Derived"))
            .expect("derived constructor proto");
        assert!(derived.code.contains(&Instruction::LoadUpvalue(0)));
        assert!(
            !derived
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
            "super must travel with the exported constructor instead of indexing its caller"
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

    #[test]
    fn var_may_redeclare_a_function_parameter() {
        let program = compile(&module(vec![function(
            "f",
            &["codepoint"],
            vec![declaration(
                VariableKind::Var,
                "codepoint",
                Some(number(1.0)),
            )],
        )]))
        .expect("a var declaration reuses its parameter binding");
        assert!(
            program.functions[0]
                .code
                .contains(&Instruction::StoreLocal(0))
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
    fn an_anonymous_class_expression_does_not_report_its_hidden_binding_as_a_name() {
        // The binding a nameless class expression is held in carries a `%` so it can never
        // collide with an identifier the program wrote. That is scaffolding, and it used to
        // reach the constructor's `name`, so `(class {}).name` read back `"%class0"` -- a value
        // no engine produces and no program could have written.
        let program = compile(&module(vec![statement(StatementKind::Expression(
            Expression {
                kind: ExpressionKind::Class(Box::new(kiln_hir::Class {
                    name: None,
                    superclass: None,
                    constructor: None,
                    methods: Vec::new(),
                    fields: Vec::new(),
                    span: Span::default(),
                })),
                span: Span::default(),
            },
        ))]))
        .expect("an empty class compiles");

        assert!(
            program
                .functions
                .iter()
                .all(|proto| proto.name.as_deref() == Some("")),
            "{:?}",
            program
                .functions
                .iter()
                .map(|proto| proto.name.clone())
                .collect::<Vec<_>>()
        );
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
                optional: false,
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

    /// A redeclaration is a compile error; an unresolved identifier is not.
    ///
    /// The two used to be reported together. ADR 0017 separated them: a name declared twice is a
    /// statement about the text and can only ever be wrong, while a name that resolves to
    /// nothing may be a host global this realm does not provide, behind a `typeof` guard that
    /// never gets to run if lowering refuses the module. The unresolved one lowers to
    /// `ThrowReference` and raises a `ReferenceError` if control reaches it.
    #[test]
    fn rejects_redeclared_locals_but_defers_unresolved_ones_to_runtime() {
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
        let diagnostics = compile(&module(body)).expect_err("a redeclaration is rejected");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("redeclaration"))
        );
        assert!(
            !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unresolved identifier")),
            "an unresolved identifier is a runtime ReferenceError, not a compile error"
        );

        // On its own it compiles, and what it compiles to is the throw.
        let program = compile(&module(vec![Statement {
            kind: StatementKind::Expression(identifier("missing")),
            span: Span::new(2, 9),
        }]))
        .expect("an unresolved identifier compiles");
        assert!(
            program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::ThrowReference(_)))
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
        // The import reads through a cell, because an import is a live view of the exporter's
        // binding rather than a copy of it. The export here is a `const` and so is *not* boxed:
        // a snapshot of something that cannot change is the same thing as a live view of it, and
        // leaving it unboxed is what keeps such a module compilable by the native backend.
        assert!(linked.program.code.contains(&Instruction::LoadCell(0)));
        assert!(linked.program.code.contains(&Instruction::StoreLocal(1)));
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
                            form: kiln_hir::PropertyForm::KeyValue,
                            span: Span::default(),
                        }),
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("b".into()),
                            value: number(2.0),
                            form: kiln_hir::PropertyForm::KeyValue,
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
                            form: kiln_hir::PropertyForm::KeyValue,
                            span: Span::default(),
                        }),
                        kiln_hir::ObjectMember::Spread(number(0.0)),
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("b".into()),
                            value: number(2.0),
                            form: kiln_hir::PropertyForm::KeyValue,
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
                    optional: false,
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
                    optional: false,
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
                    optional: false,
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
    fn a_direct_read_before_a_declaration_throws_where_the_read_is() {
        // `x; let x = 1` is well-formed JavaScript: it compiles, and the read throws a
        // `ReferenceError` when it is reached. Pre-declaring every name in a block would
        // otherwise turn that into a silent `undefined`, which is the trade the dead-zone
        // marker exists to avoid -- so the read becomes one of a cell that is uninitialised by
        // construction, and the source is not refused.
        let program = compile(&module(vec![
            statement(StatementKind::Expression(identifier("x"))),
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("x".into()),
                    initializer: Some(number(1.0)),
                    span: Span::default(),
                }],
            }),
        ]))
        .expect("a temporal dead zone is not a compile error");
        assert!(
            program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::MarkUninitialized(_))),
            "{:?}",
            program.code
        );
    }

    #[test]
    fn an_uncaptured_module_lexical_needs_a_runtime_dead_zone_marker() {
        let program = compile(&module(vec![statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("value".into()),
                initializer: Some(number(1.0)),
                span: Span::default(),
            }],
        })]))
        .expect("compiles");

        assert!(
            program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::MarkUninitialized(_)))
        );
    }

    #[test]
    fn a_module_lexical_captured_by_a_nested_function_needs_a_dead_zone_marker() {
        let program = compile(&module(vec![
            function(
                "read",
                &[],
                vec![statement(StatementKind::Return(Some(identifier("value"))))],
            ),
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("value".into()),
                    initializer: Some(number(1.0)),
                    span: Span::default(),
                }],
            }),
        ]))
        .expect("compiles");

        assert!(
            program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::MarkUninitialized(_)))
        );
    }

    #[test]
    fn a_script_rejects_top_level_await_with_a_module_only_diagnostic() {
        let source = statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::Await(Box::new(number(1.0))),
            span: Span::new(4, 9),
        }));
        let diagnostics = compile(&module(vec![source])).expect_err("script await is rejected");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.message == "top-level `await` is only available in a module"
                && diagnostic.span == Some(Span::new(4, 9))
        }));
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
                strict: false,
                binds_own_name: false,
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

    /// `[o.p] = xs` evaluates `o` before it reads the element, not after.
    ///
    /// The order is observable whenever either side throws or runs a getter, and it is the one
    /// thing a pattern walk cannot get right by emitting the value first and the target second --
    /// which is what every other leaf does.
    #[test]
    fn a_reference_target_is_evaluated_before_the_value_it_receives() {
        let body = vec![
            statement(StatementKind::Variable {
                kind: VariableKind::Var,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("o".into()),
                    initializer: None,
                    span: Span::default(),
                }],
            }),
            statement(StatementKind::Variable {
                kind: VariableKind::Var,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("xs".into()),
                    initializer: None,
                    span: Span::default(),
                }],
            }),
            statement(StatementKind::Expression(Expression {
                kind: ExpressionKind::PatternAssignment {
                    pattern: kiln_hir::Pattern::Array {
                        elements: vec![Some(kiln_hir::Pattern::Member {
                            object: Box::new(identifier("o")),
                            property: "p".into(),
                        })],
                        rest: None,
                    },
                    value: Box::new(identifier("xs")),
                },
                span: Span::default(),
            })),
        ];
        let program = compile(&module(body)).expect("valid HIR compiles");
        let at = |wanted: &Instruction| {
            program
                .code
                .iter()
                .position(|instruction| instruction == wanted)
                .expect("instruction is emitted")
        };
        let receiver = at(&Instruction::LoadLocal(0));
        // The value an element receives is whatever stepping the iterator produced, so the step
        // is what has to come after the receiver and before the store. The step is an `Apply` of
        // the `next` the record captured, not a fresh read of it: the read happens once, when the
        // iterator is opened, which is before the first element's receiver is even evaluated.
        let next = program
            .constants
            .iter()
            .position(|constant| matches!(constant, Constant::String(name) if name == "next"))
            .expect("the walk names the method it captures") as u32;
        let captured = at(&Instruction::GetProperty(next));
        assert!(
            captured < receiver,
            "the iterator record is complete before any element is taken"
        );
        let read = at(&Instruction::Apply);
        let store = program
            .code
            .iter()
            .position(|instruction| {
                matches!(
                    instruction,
                    Instruction::SetProperty(_) | Instruction::SetPropertyStrict { .. }
                )
            })
            .expect("the element is stored through the reference");
        assert!(receiver < read, "the receiver is evaluated before the read");
        assert!(read < store, "the value exists before it is stored");
    }

    /// `let [o.p] = xs` is a syntax error, and the parser says so first -- but nothing in the
    /// HIR shape prevents a frontend from building it, so the walk refuses it rather than
    /// storing through a reference in a position that only ever declares names.
    #[test]
    fn a_declaration_cannot_bind_through_a_reference() {
        let body = vec![statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Array {
                    elements: vec![Some(kiln_hir::Pattern::Member {
                        object: Box::new(identifier("o")),
                        property: "p".into(),
                    })],
                    rest: None,
                },
                initializer: Some(identifier("xs")),
                span: Span::default(),
            }],
        })];
        let diagnostics = compile(&module(body)).expect_err("a declaration is refused");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("property reference")),
            "the diagnostic names what it refused: {diagnostics:?}"
        );
    }
}
