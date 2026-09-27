//! A small, parser-independent stack bytecode for executable Kiln modules.
//!
//! The frontend owns the parser-specific lowering into [`kiln_hir`].  This crate deliberately
//! consumes only that owned HIR and produces plain data, so an interpreter, serializer, or native
//! backend can be added without coupling it to Oxc (or to any other parser).

use std::sync::Arc;

pub use kiln_hir::{Span, TypeAnnotation, VariableKind};
pub use kiln_host_abi::HostOp;
pub use kiln_string::StringUnits;

/// Version of the bytecode contract produced by this crate.
pub const BYTECODE_VERSION: u16 = 6;

/// A constant embedded in a [`Program`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Constant {
    Undefined,
    Null,
    Boolean(bool),
    /// IEEE-754 `f64` bits; preserving bits makes program equality total (including NaN/-0).
    Number(u64),
    String(StringUnits),
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

/// Which kind of element a private definition installs.
///
/// A private name is not a property name with a reserved spelling: `#m` and `get #m` behave
/// differently on both a read and a write, so the kind travels with the definition rather than
/// being guessed from what the slot happens to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateKind {
    /// `#m() {}` -- readable, and a write to it is a TypeError.
    Method,
    /// `get #x() {}` -- a read calls it; with no setter beside it a write is a TypeError.
    Getter,
    /// `set #x(v) {}` -- a write calls it; with no getter beside it a read is a TypeError.
    Setter,
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
    JumpIfReturnSignal(u32),
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
    /// `ToPropertyKey` of the key on top of the stack, replacing it. Stack is object, then key.
    ///
    /// This is the memoisation the specification performs on a Reference Record: a compound
    /// assignment reads and writes through *one* reference, and `GetValue` stores the converted
    /// key back into it so `PutValue` does not convert again. Without it a key object's
    /// `toString` runs twice, which the program can see.
    ///
    /// It reads the object beneath the key without consuming it, because the order is fixed:
    /// a nullish object throws *before* the key converts, so `null[k] += 1` never runs `k`'s
    /// `toString`. Rather than duplicate that error here, a nullish object leaves the key alone
    /// and the read that follows throws it.
    ///
    /// Only an object does any work; every primitive is already a key and passes through, which
    /// keeps `a[0]` on the array fast path. Plain assignment does not use this -- there the
    /// specification converts in `PutValue`, after the right-hand side has run.
    ToPropertyKey,
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
    /// Suspend the running generator in a `yield*` delegation.
    ///
    /// Stack: `[iterator, value] -> [sent]`.
    /// `exit` is the jump target if the inner iterator's `throw()` completes with `done: true`.
    YieldStar(u32),
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
    /// `super.x`: read the named property of the value on top -- the home object's prototype --
    /// with the running function's `this` as the receiver. Stack: `[prototype] -> [value]`.
    ///
    /// Not two `GetProperty`s. The receiver is what a getter found up there is called with, and
    /// an ordinary read would hand it the prototype: `class Q extends P { read() { return
    /// super.who } }` asked `P.prototype`'s getter about `P.prototype` and answered `undefined`
    /// where the instance had the value.
    LoadSuperProperty(u32),
    /// `super[k]`: the computed spelling of [`Instruction::LoadSuperProperty`].
    /// Stack: `[key, prototype] -> [value]`.
    ///
    /// The key sits *under* the prototype because the language evaluates it first: the home
    /// object's `[[Prototype]]` is read after the key expression has run, so a key expression
    /// that re-links the home object is seen by the lookup it precedes.
    LoadSuperElement,
    /// `super.x = v`: write the named property of the value on top -- the home object's prototype --
    /// with the running function's `this` as the receiver. Stack: `[prototype, value] -> [value]`.
    SetSuperPropertyStrict {
        name: u32,
        strict: bool,
    },
    /// `super[k] = v`: the computed spelling of [`Instruction::SetSuperPropertyStrict`].
    /// Stack: `[key, prototype, value] -> [value]`.
    SetSuperElementStrict {
        strict: bool,
    },
    /// Read a value's `[[Prototype]]`. Stack: `[value] -> [prototype]`.
    ///
    /// The read half of [`Instruction::SetPrototype`], and it exists for exactly one caller:
    /// `super.x` is a lookup on the *current* prototype of the object the method was defined on,
    /// and reading it through `Object.getPrototypeOf` would let a program that replaced that
    /// function change what `super` means.
    LoadPrototype,
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
    /// `import(specifier, options)`. The options value remains on the stack so its evaluation is
    /// observable to the host; loaders may reject attributes they do not support explicitly.
    DynamicImportWithOptions,
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
    /// Sloppy assignment to a name nothing bound: create or overwrite it on the global object.
    ///
    /// `undeclaredGlobal = 1` in sloppy code does not throw -- the unresolvable reference is
    /// settled against the global object environment record, which creates the property. That is
    /// how a UMD header installs itself, and it is the one place where an *assignment* to an
    /// unresolved name is not the `ReferenceError` a *read* of the same name is. Strict code
    /// never emits this; it keeps [`Instruction::ThrowReference`].
    ///
    /// The operand is the name constant. Stack: `[value] -> []`.
    StoreGlobalBinding(u32),
    /// The same assignment written in strict code: the global object's `SetMutableBinding` with
    /// `S` true, which writes an existing property and throws a `ReferenceError` for a name the
    /// realm does not have.
    ///
    /// Strict mode refuses to *create* a global by assignment; it does not refuse to write one.
    /// The difference can only be settled at run time, because a global the program installed a
    /// line earlier -- or that global code declared with `var` -- is a perfectly resolvable
    /// reference the compiler cannot see. Deciding it at compile time made `"use strict"` code
    /// throw on a write to a name that was plainly there.
    ///
    /// The operand is the name constant. Stack: `[value] -> []`.
    StoreGlobalBindingStrict(u32),
    /// Throw a `ReferenceError` now if the global object has no property `name`.
    ///
    /// Emitted where strict code *resolves* an assignment target no scope bound, which is before
    /// the right-hand side runs: `ResolveBinding` decides then whether the reference is
    /// unresolvable, and `PutValue` refuses an unresolvable one in strict mode however the value
    /// was produced.
    ///
    /// The operand is the name constant. Stack: `[] -> []`.
    RequireGlobalBinding(u32),
    /// The global object's property `name`, or `undefined` when it has none.
    ///
    /// `typeof neverDeclared` is `"undefined"` rather than a throw -- the one place a name may be
    /// read without existing, and the whole reason feature detection works. That answer cannot be
    /// folded at compile time, though: `globalThis.x = 1` a line above, a `var` in global code,
    /// or an `eval` anywhere makes the same name a perfectly ordinary global, and folding said
    /// `"undefined"` for one that was plainly there.
    ///
    /// The operand is the name constant. Stack: `[] -> [value]`.
    LoadGlobalBindingOrUndefined(u32),
    /// `delete name` in sloppy code, where no scope declared `name`.
    ///
    /// The last environment record an unqualified reference reaches is the global object's, so
    /// this is that object's `[[Delete]]`: `true` when the property is gone or was never there,
    /// `false` when it is non-configurable.  `x = 1; delete x` is therefore `true` and really
    /// removes the property, while `delete NaN` is `false`.
    ///
    /// Only emitted for a name the compiler could not place.  A binding some scope *did* declare
    /// is `false` without asking anything at run time, and strict code never reaches here at all
    /// -- an unqualified `delete` is an early SyntaxError.
    ///
    /// The operand is the name constant. Stack: `[] -> [boolean]`.
    DeleteGlobalBinding(u32),
    /// `CreateGlobalVarBinding`/`CreateGlobalFunctionBinding`: declare `name` on the global
    /// object, leaving an existing property alone.
    ///
    /// Global code's variable environment *is* the global object, so `var x` at the top of a
    /// script is a property of it and not a slot -- which is why `globalThis.x` finds it, why an
    /// indirect `eval` can read it, and why `delete x` is decided by the property's own
    /// attributes.
    ///
    /// It creates rather than assigns: `var x;` over an `x` the realm already has keeps the value
    /// it had, which is the difference between declaring a binding and writing one.
    /// `configurable` is the specification's `D`, false for a script and true for an `eval`.
    ///
    /// Stack: `[] -> []`.
    DeclareGlobalBinding {
        name: u32,
        configurable: bool,
    },
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
    /// Install a private method or accessor half. Stack: `[target, function] -> [target]`.
    ///
    /// Separate from [`Instruction::DefineMethod`] because a private element has a *kind* and an
    /// ordinary property does not: `get #x` reads through a call and `#m` refuses a write, and
    /// storing either as a plain slot answers both questions wrongly and silently. An instance
    /// element is installed on each instance rather than on the prototype, because the presence
    /// of the name on the object *is* the brand check.
    DefinePrivate {
        name: u32,
        kind: PrivateKind,
    },
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
    /// `{ [key]: function () {} }`: define an *enumerable* property and name the function after
    /// the key, the way `NamedEvaluation` does.
    ///
    /// Separate from [`Instruction::DefineMethod`], which names the same way but defines a
    /// method -- not enumerated, because a class's methods are not. An object literal's are, so
    /// the two cannot share one instruction. Emitted only where the value is a *syntactically*
    /// anonymous function: `{ [k]: (0, function () {}) }` is a parenthesised reference and keeps
    /// the empty name, which a runtime test for "has no name yet" could not tell apart.
    DefineNamedProperty,
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
    /// Load or materialize the canonical frozen template object for a tagged template site.
    ///
    /// Stack: `[] -> [template_object]`.
    ///
    /// Per ECMA-262 §13.2.8.5 (`GetTemplateObject`), evaluating a syntactic tagged template
    /// site yields the identical frozen object (`===`) on every evaluation within the realm.
    LoadTemplateObject(u32),
    Return,
    /// A stack-neutral placeholder used by closed-world backends when a compiler-proven pure
    /// declaration is omitted while preserving bytecode PCs and jump targets.
    Nop,
}

/// A template literal site for canonical template object caching (ECMA-262 §13.2.8.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateSite {
    pub cooked: Vec<Option<StringUnits>>,
    pub raw: Vec<StringUnits>,
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
    /// The text this program was compiled from, which [`FunctionProto::span`] indexes into.
    ///
    /// `Function.prototype.toString` is specified to answer with the source text the function was
    /// written as, so the text has to outlive the parse. One shared buffer per program rather
    /// than a copy per function: function spans nest, and copying would store an outer body once
    /// for every function written inside it -- quadratic in nesting depth on exactly the deeply
    /// wrapped bundles where it would hurt.
    ///
    /// Shared with the [`Module`] it was compiled from rather than copied out of it, so a program
    /// and its HIR cost one buffer between them and not two.
    pub source: Arc<str>,
    /// Syntactic template literal sites compiled in this program, referenced by
    /// [`Instruction::LoadTemplateObject`].
    pub template_sites: Vec<TemplateSite>,
}

impl Default for Program {
    fn default() -> Self {
        Self {
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: Vec::new(),
            local_count: 0,
            functions: Vec::new(),
            source: "".into(),
            template_sites: Vec::new(),
        }
    }
}

impl Program {
    /// Whether this program or any nested function prototype contains [`Instruction::DirectEval`].
    pub fn has_direct_eval(&self) -> bool {
        self.code
            .iter()
            .any(|inst| matches!(inst, Instruction::DirectEval(_)))
            || self.functions.iter().any(|f| {
                f.code
                    .iter()
                    .any(|inst| matches!(inst, Instruction::DirectEval(_)))
            })
    }

    /// Whether this program or any nested function prototype contains [`Instruction::DynamicImport`].
    pub fn has_dynamic_import(&self) -> bool {
        self.code.iter().any(|inst| {
            matches!(
                inst,
                Instruction::DynamicImport | Instruction::DynamicImportWithOptions
            )
        }) || self.functions.iter().any(|f| {
            f.code.iter().any(|inst| {
                matches!(
                    inst,
                    Instruction::DynamicImport | Instruction::DynamicImportWithOptions
                )
            })
        })
    }
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
    /// Lossless display name supplied by named evaluation when [`Self::name`] is absent.
    pub inferred_name: Option<StringUnits>,
    /// An arrow function: it has no `this` of its own and captures the defining scope's.
    pub arrow: bool,
    /// A generator: calling it builds an object rather than running the body.
    pub generator: bool,
    /// `async function*`: both of the above at once.
    ///
    /// `generator` is set for an async function too -- its body suspends the same way -- so the
    /// pair cannot say which of the three shapes this is. This is the third bit.
    pub async_generator: bool,
    /// A method: written with method-definition syntax (object method, class method, static
    /// method, or accessor). It has no `prototype` property unless it is a generator, and can
    /// never be constructed with `new`.
    pub is_method: bool,
    pub parameter_count: u32,
    /// Whether this function's `arguments` is the *mapped* kind: the code is sloppy and
    /// `IsSimpleParameterList` is true.
    ///
    /// A fact about the function rather than about the mention, which is why it is here beside
    /// `arrow` rather than on `LoadArguments`. An arrow captures the enclosing call's object
    /// where it is created, which can be before anything has mentioned `arguments` -- so the
    /// answer has to be reachable from the frame, not only from the instruction that asks.
    pub mapped_arguments: bool,
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
    /// This function's own text within [`Program::source`], as
    /// `Function.prototype.toString` must answer it.
    ///
    /// The whole definition, not the body: `toString` answers with what the program wrote, which
    /// for a method is its key and parameter list too -- `({ m(x) {} }).m.toString()` is
    /// `"m(x) {}"`, and for a getter it keeps the `get`.
    pub span: Span,
    /// TypeScript type annotations, if any.
    pub type_annotations: Option<TypeAnnotations>,
}

/// TypeScript type annotations for a function.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TypeAnnotations {
    pub parameters: Vec<Option<TypeAnnotation>>,
    pub return_type: Option<TypeAnnotation>,
}

impl FunctionProto {
    /// The optional name a runtime closure should expose, preserving JavaScript UTF-16 units.
    /// Written names take precedence because they establish a binding; inferred names are
    /// display metadata used only when no written name exists.
    ///
    /// An *empty* written name is no written name: anonymous class constructors
    /// carry `Some("")` as their spelling, and it must not shadow the name a
    /// named evaluation inferred (`const Dep = class {}` is `"Dep"`, not `""`).
    pub fn effective_name_units(&self) -> Option<StringUnits> {
        self.name
            .as_deref()
            .map(StringUnits::from)
            .filter(|units| !units.is_empty())
            .or_else(|| self.inferred_name.clone())
    }

    /// The name exposed by function metadata, preserving JavaScript UTF-16 units.
    ///
    /// Written names take precedence because they also establish a binding. Inferred names are
    /// display metadata only and may contain lone surrogates from a property key.
    pub fn display_name_units(&self) -> StringUnits {
        self.effective_name_units().unwrap_or_default()
    }

    /// Return the annotated type for parameter `slot`, if any.
    pub fn parameter_type(&self, slot: usize) -> Option<TypeAnnotation> {
        self.type_annotations
            .as_ref()
            .and_then(|ta| ta.parameters.get(slot).copied().flatten())
    }

    /// Return the annotated return type, if any.
    pub fn return_type(&self) -> Option<TypeAnnotation> {
        self.type_annotations.as_ref().and_then(|ta| ta.return_type)
    }
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
    /// The error kind that reports several failures as one, and what `Promise.any` rejects with.
    /// Appended for the reason every name after `Json` is: the discriminant is hashed into the
    /// artifact cache key, so inserting one in the middle would invalidate every cached build.
    AggregateError,
    /// Cancellation, which every modern asynchronous API takes as an argument. Appended for the
    /// reason above.
    AbortController,
    AbortSignal,
    /// The UTF-8 encoder half of the pair whose decoder is already here.
    TextEncoder,
    StructuredClone,
}

macro_rules! builtin_bindings {
    ($( $variant:ident => $name:literal ),+ $(,)?) => {
        /// The canonical global spelling for every compiler-provided builtin.
        pub const GLOBAL_BINDINGS: &'static [(Self, &'static str)] = &[
            $( (Self::$variant, $name), )+
        ];

        /// The global name that resolves to this built-in.
        pub fn from_name(name: &str) -> Option<Self> {
            match name {
                $( $name => Some(Self::$variant), )+
                "global" => Some(Self::GlobalThis),
                _ => None,
            }
        }

        pub fn name(self) -> &'static str {
            match self {
                $( Self::$variant => $name, )+
            }
        }
    };
}

impl Builtin {
    builtin_bindings! {
        Error => "Error",
        Symbol => "Symbol",
        String => "String",
        GlobalThis => "globalThis",
        Set => "Set",
        Map => "Map",
        Array => "Array",
        Object => "Object",
        Json => "JSON",
        Math => "Math",
        Number => "Number",
        Boolean => "Boolean",
        NotANumber => "NaN",
        Infinity => "Infinity",
        ParseInt => "parseInt",
        EncodeUri => "encodeURI",
        EncodeUriComponent => "encodeURIComponent",
        DecodeUri => "decodeURI",
        DecodeUriComponent => "decodeURIComponent",
        ParseFloat => "parseFloat",
        IsNaN => "isNaN",
        IsFinite => "isFinite",
        TypeError => "TypeError",
        RangeError => "RangeError",
        ReferenceError => "ReferenceError",
        SyntaxError => "SyntaxError",
        UriError => "URIError",
        SetTimeout => "setTimeout",
        SetInterval => "setInterval",
        ClearTimeout => "clearTimeout",
        ClearInterval => "clearInterval",
        QueueMicrotask => "queueMicrotask",
        Promise => "Promise",
        RegExp => "RegExp",
        Date => "Date",
        WeakMap => "WeakMap",
        WeakSet => "WeakSet",
        Performance => "performance",
        Reflect => "Reflect",
        BigInt => "BigInt",
        Process => "process",
        Buffer => "Buffer",
        Url => "URL",
        Function => "Function",
        Proxy => "Proxy",
        Escape => "escape",
        Unescape => "unescape",
        ArrayBuffer => "ArrayBuffer",
        Int8Array => "Int8Array",
        Uint8Array => "Uint8Array",
        Uint8ClampedArray => "Uint8ClampedArray",
        Int16Array => "Int16Array",
        Uint16Array => "Uint16Array",
        Int32Array => "Int32Array",
        Uint32Array => "Uint32Array",
        Float32Array => "Float32Array",
        Float64Array => "Float64Array",
        Eval => "eval",
        DisposableStack => "DisposableStack",
        SuppressedError => "SuppressedError",
        AsyncDisposableStack => "AsyncDisposableStack",
        EvalError => "EvalError",
        Atomics => "Atomics",
        TextDecoder => "TextDecoder",
        SetImmediate => "setImmediate",
        ClearImmediate => "clearImmediate",
        AggregateError => "AggregateError",
        AbortController => "AbortController",
        AbortSignal => "AbortSignal",
        TextEncoder => "TextEncoder",
        StructuredClone => "structuredClone",
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
    /// A local cell plus the constant-table index of its source binding name, for diagnostics.
    NamedParentLocal { slot: u32, name: u32 },
    /// A threaded cell plus the constant-table index of its source binding name.
    NamedParentUpvalue { index: u32, name: u32 },
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

/// One class's private names, as a direct `eval` written inside that class has to see them.
///
/// The names alone would not do. A private element is stored under a key built from the class's
/// *identity* -- `class A { #v } class B { #v }` declare two different fields -- and that
/// identity is a counter belonging to the compiler that compiled the class. An eval'd source is
/// compiled by a second compiler whose counter starts at zero, so a name resolved there against a
/// fresh identity would name a slot no object of the enclosing class carries, and the brand check
/// would report the class as not having declared its own field. The identity travels with the
/// names for that reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateScope {
    pub scope: u32,
    /// Sorted, because the bytecode is a cache key and a hash set has no order to promise.
    pub names: Vec<String>,
}

/// What `new.target` means at a direct `eval` call site.
///
/// Three answers rather than a flag, because "not allowed" and "allowed, and always `undefined`"
/// are different things and the second is not the caller's `new.target`: a class field
/// initialiser is its own function boundary for this one question, even though Kiln emits its
/// code straight into the constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EvalNewTarget {
    /// The call is at the top level of a script, or in an arrow written there. `new.target` in
    /// the eval'd source is a `SyntaxError`, which is what the language says and what
    /// `eval('new.target')` at top level is written to observe.
    #[default]
    Forbidden,
    /// The call is inside a function, so `new.target` names whatever that call was constructed
    /// with -- `undefined` for an ordinary call.
    Caller,
    /// The call is inside a class field initialiser or a `static {}` block, where `new.target`
    /// is legal and is always `undefined`.
    Undefined,
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
    /// Whether the calling code's variable environment *is* the global environment record.
    ///
    /// True for a direct `eval` written at the top level of a script, and it is what decides
    /// where the eval's own `var` and function declarations go: onto the global object, where
    /// the script and every later `eval` can see them, rather than into slots that die with the
    /// eval's frame. An indirect `eval` never carries a site at all and is global by definition.
    pub global_var_scope: bool,
    /// The hidden home-object binding in scope here, when `super.x` is legal at this call.
    ///
    /// `Some` only inside a method, an accessor, a class field initialiser or a `static {}`
    /// block, and inside arrows written in one -- not inside an ordinary function nested in a
    /// method, where the language makes `super.x` a `SyntaxError` however close the home object
    /// is. The binding itself is handed over among `bindings` like any other, because it is an
    /// ordinary celled local that happens to carry a name no source can write.
    pub home: Option<String>,
    /// What `new.target` means here, including whether it may be written at all.
    pub new_target: EvalNewTarget,
    /// The private names in scope, outermost class first.
    pub private_scopes: Vec<PrivateScope>,
    /// Names a `var` in the eval'd source may not take over, sorted.
    ///
    /// `EvalDeclarationInstantiation` walks from the call's lexical environment out to its
    /// *variable* environment and refuses a `var` that collides with a binding in any record
    /// between them -- because hoisting one there would put it where a like-named lexical
    /// binding already shadows it. A block's `let` reaches the eval'd source as an outer binding
    /// and is caught by the ordinary redeclaration rule; a *parameter* environment does not,
    /// because this compiler gives a function's parameters and its body one scope. So the names
    /// that environment holds travel separately: the formals, and `arguments` for anything but
    /// an arrow.
    ///
    /// Empty unless the call is written inside a parameter default, which is the only place the
    /// two environments are distinct.
    pub blocked_var_names: Vec<String>,
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

/// Target specifier for a dynamic import (`import(...)`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DynamicImportTarget {
    /// Statically known string literal, e.g. `import("./foo.js")`.
    Static(String),
    /// Non-statically-resolvable expression, e.g. `import(expr)`.
    Dynamic,
}

/// Whole-program linking and closed-world facts extracted for one compiled module.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModuleLinkFacts {
    /// Literal Node loader requests discovered by the source frontend. These remain lazy at
    /// execution time but must participate in graph closure and liveness selection.
    pub node_runtime_requests: Vec<kiln_hir::NodeRuntimeRequest>,
    /// Module-level bindings that are never mutated across the program.
    pub immutable_bindings: Vec<String>,
    /// Whether the module syntax contains direct `eval(...)`.
    pub direct_eval: bool,
    /// Whether the module syntax contains `new Function(...)` or dynamic `Function(...)`.
    pub new_function: bool,
    /// Dynamic import call sites and their targets.
    pub dynamic_imports: Vec<DynamicImportTarget>,
    /// Top-level declaration evaluations that have no observable effect unless one of their
    /// bindings is read. The compiler records bytecode and function-table ownership while the
    /// HIR still preserves declaration boundaries; a closed-world backend may then omit a row
    /// whose slots are not demanded without guessing those boundaries from flattened bytecode.
    pub removable_declarations: Vec<RemovableDeclaration>,
}

/// One side-effect-free top-level declaration owned by a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovableDeclaration {
    pub local_slots: Vec<u32>,
    pub pc_start: u32,
    pub pc_end: u32,
}

/// Linkage-aware bytecode for one module. [`Program`] stays parser- and graph-independent so the
/// existing single-file interpreter and native backend APIs remain stable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModuleProgram {
    pub program: Program,
    /// Runtime evaluation dependencies, including side-effect-only imports, in source order.
    pub dependencies: Vec<String>,
    pub imports: Vec<ImportBinding>,
    pub exports: Vec<ExportBinding>,
    /// Which of this module's bodies run as strict code.
    ///
    /// An ESM module is strict throughout and reports [`Strictness::always`]. A CommonJS module
    /// is *not*: node runs it as a sloppy function body, so `this` in a bare call is the global
    /// object and a write refused by a frozen object is discarded. The linker cannot infer this
    /// from the bytecode, so the compiler that knew the goal has to say it here.
    pub strictness: Strictness,
    /// Whole-program linking and closed-world facts.
    pub link_facts: ModuleLinkFacts,
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
    /// Whether this program is *global* code, whose variable environment is the global
    /// environment record.
    ///
    /// It is not the same question as which grammar goal parsed the source, which is why the
    /// caller has to say. Kiln parses a CommonJS body with the script grammar too, and a CommonJS
    /// body has a variable environment of its own -- Node wraps it in a function -- so its
    /// top-level `var`s must stay slots. Only a real script has the global object for a variable
    /// environment, and only its `var`s and function declarations become properties of it.
    pub global_code: bool,
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

mod compile;
pub use compile::*;

#[cfg(test)]
mod tests;
