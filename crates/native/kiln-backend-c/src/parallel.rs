//! Which callbacks `parallelMap` may run off the realm thread (ADR 0058, application 2).
//!
//! A chunk of `parallelMap` runs on a Rayon worker with no realm: no host, no `thrown`, no
//! thread-local heap cache, and a collector on the realm thread that assumes it is the only
//! mutator. The only closure that can run there is one that reads its `double` parameters,
//! computes, and returns a `double`. This module is the one place that decides that, from the
//! function's value graph, so the decision is a list of rules a reviewer can read and a test can
//! pin one by one, rather than a property of whatever the emitter happens to produce.

use crate::graph::{FunctionGraph, GraphOp, Proven, Terminator, proven_numbers_with_assumptions};
use kiln_ir::{Builtin, Constant, FunctionProto, Program, UnaryOp};

/// The `Math` members a chunk may call: each is one libm call on a double, allocates nothing,
/// and never throws. This is exactly the set `direct::intrinsic_results` proves returns a number
/// and `emit_graph::ops::call_method` emits inline as C; a name on one list and not the others
/// would either not be proven or not be inline, so keep the three together.
pub const PARALLEL_MATH: &[(&str, usize)] = &[
    ("sqrt", 1),
    ("abs", 1),
    ("floor", 1),
    ("ceil", 1),
    ("round", 1),
    ("trunc", 1),
    ("sign", 1),
    ("min", 2),
    ("max", 2),
    ("pow", 2),
    ("sin", 1),
    ("cos", 1),
    ("tan", 1),
    ("asin", 1),
    ("acos", 1),
    ("atan", 1),
    ("atan2", 2),
    ("sinh", 1),
    ("cosh", 1),
    ("tanh", 1),
    ("asinh", 1),
    ("acosh", 1),
    ("atanh", 1),
    ("exp", 1),
    ("expm1", 1),
    ("log", 1),
    ("log2", 1),
    ("log10", 1),
    ("log1p", 1),
    ("cbrt", 1),
    ("hypot", 2),
];

/// Why a `parallelMap` call site runs sequentially: one stable code per rule, so
/// `kiln build --explain-parallel` names exactly the rule that decided, and a test pins each.
///
/// The codes are part of the tool's output and are never renamed; a rule that goes away keeps
/// its code retired rather than reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reason {
    // Rule 1: the function itself.
    /// A generator, an async function or an async generator.
    NotPlainFunction,
    /// Declared with other than one or two parameters.
    Arity,
    /// Reads a rest parameter.
    RestParameter,
    // Rule 2: control flow.
    /// A `try` block: a handler would catch on a worker.
    ExceptionHandler,
    /// A `throw` the body can reach.
    Throws,
    // Rule 3: values.
    /// A string or bigint literal.
    NonNumericConstant,
    /// An operation the rules do not list (`typeof`, `in`, a bridged instruction, ...).
    UnsupportedOperation,
    // Rule 4: nothing beyond the frame.
    /// Reads a captured binding that is written more than once (tier 1 admits only a binding
    /// assigned exactly once).
    CapturesReassigned,
    /// Reads a captured binding whose writes the compiler cannot enumerate.
    CapturesUnresolved,
    /// Assigns a captured binding.
    WritesCapture,
    /// Reads or writes a global.
    ReadsGlobal,
    /// Reads `this`, `new.target` or `arguments`.
    ReadsThis,
    // Rule 5: no heap.
    /// Reads a property or an element.
    ReadsProperty,
    /// Writes a property or an element.
    WritesProperty,
    /// Allocates: an array, an object, a closure, a cell.
    Allocates,
    /// Calls something other than a listed `Math` function.
    Calls,
    // Rule 6: the result.
    /// A reachable `return` without a value (the answer would be `undefined`).
    ReturnsUndefined,
    /// Some reachable `return` is not proven to be a number.
    ReturnsNonNumber,
    // The plan and the emitter.
    /// The callback has no typed entry: not directly callable, or too large. Since tier 2 a
    /// callback the plan finds untyped runs as its heap clone instead (every callback tier 1's
    /// source rules admit, tier 2's admit too), so this is only the call site's answer when a
    /// verdict it holds has no typed entry behind it.
    NotTyped,
    /// The emitted parallel clone reaches realm state (host, heap, upvalue table), so the
    /// machine check in `lib.rs` rejected it.
    CloneRejected,
    // The call site.
    /// The callback is not one function the compiler can identify at this site.
    CallbackUnknown,
    /// More than the callback was passed (a `thisArg`).
    ExtraArguments,
    /// The call does not have the arguments the method's parallel lowering takes:
    /// `parallelReduce(callback, combine, initial)` with other than three, or a `parallelSort`
    /// with more than a comparator.
    ArgumentShape,
    /// `parallelSort`'s comparator is not one the compiler proves consistent (tier 1 knows
    /// `(a, b) => a - b` and `(a, b) => b - a`); `sort`'s order for any other is the engine's.
    ComparatorUnproven,
    /// The module is emitted with generic bodies only and without the per-site parallel
    /// lowering. No build profile produces this any more (issue 13: release modules past the
    /// specialization limit still lower their qualifying sites, and the development profile
    /// says [`Reason::DevProfile`]); the code stays so a record in an older artifact parses.
    GenericOnly,
    /// A development build (`native-dev`, `check`): it emits no typed or parallel code, by
    /// decision -- the edit-run loop stays proportional to the program, and the answer is
    /// identical either way. `--profile native-release` gives the site its parallel lowering.
    DevProfile,
    /// Tier 2: the callback reads a property this module defines as an accessor (a `get` in an
    /// object literal or a class), or `__proto__`: reading it calls a function, which may be user
    /// code, so the read cannot run on a pool thread. A getter the compiler cannot see (defined
    /// in another module, or with `Object.defineProperty`) is found at run time instead: the
    /// read declines and the call runs sequentially.
    ReadsAccessor,
    /// Tier 3: the callback builds values (an object or array literal, a spread, a string), which
    /// only `parallelMap`, `parallelFilter` and `parallelFlatMap` keep: `parallelReduce` folds
    /// numbers and `parallelSort` compares keys.
    AllocatesForMethod,
    /// Tier 4: the callback calls a built-in not marked pure (`kiln_builtin_surface::PURE_MEMBERS`)
    /// -- `console.log`, `Math.random`, `Date.now`, `arr.push` -- or one marked pure in a form the
    /// mark does not cover (a `thisArg`, a `JSON.stringify` replacer).
    CallsImpure,
    /// Tier 4: the callback calls a function of the program the proof does not admit: the
    /// function's own body breaks a rule (named in the detail), directly or in a function it calls.
    CallsUnproven,
    /// Tier 4: the callback calls a value the compiler cannot identify as one function (a
    /// parameter, a binding assigned more than once, a property).
    CallsUnknown,
    /// Tier 4: the functions the callback calls nest deeper than the proof follows
    /// ([`PARALLEL_CALL_DEPTH`]), or are more than it emits ([`PARALLEL_CALLEES`]).
    CallsTooDeep,
}

impl Reason {
    /// Every reason, in declaration order.
    pub const ALL: &'static [Reason] = &[
        Reason::NotPlainFunction,
        Reason::Arity,
        Reason::RestParameter,
        Reason::ExceptionHandler,
        Reason::Throws,
        Reason::NonNumericConstant,
        Reason::UnsupportedOperation,
        Reason::CapturesReassigned,
        Reason::CapturesUnresolved,
        Reason::WritesCapture,
        Reason::ReadsGlobal,
        Reason::ReadsThis,
        Reason::ReadsProperty,
        Reason::WritesProperty,
        Reason::Allocates,
        Reason::Calls,
        Reason::ReturnsUndefined,
        Reason::ReturnsNonNumber,
        Reason::NotTyped,
        Reason::CloneRejected,
        Reason::CallbackUnknown,
        Reason::ExtraArguments,
        Reason::GenericOnly,
        Reason::DevProfile,
        Reason::ArgumentShape,
        Reason::ComparatorUnproven,
        Reason::ReadsAccessor,
        Reason::AllocatesForMethod,
        Reason::CallsImpure,
        Reason::CallsUnproven,
        Reason::CallsUnknown,
        Reason::CallsTooDeep,
    ];

    /// The stable code printed by `--explain-parallel`.
    pub const fn code(self) -> &'static str {
        match self {
            Reason::NotPlainFunction => "not-plain-function",
            Reason::Arity => "arity",
            Reason::RestParameter => "rest-parameter",
            Reason::ExceptionHandler => "exception-handler",
            Reason::Throws => "throws",
            Reason::NonNumericConstant => "non-numeric-constant",
            Reason::UnsupportedOperation => "unsupported-operation",
            Reason::CapturesReassigned => "captures-reassigned",
            Reason::CapturesUnresolved => "captures-unresolved",
            Reason::WritesCapture => "writes-capture",
            Reason::ReadsGlobal => "reads-global",
            Reason::ReadsThis => "reads-this",
            Reason::ReadsProperty => "reads-property",
            Reason::WritesProperty => "writes-property",
            Reason::Allocates => "allocates",
            Reason::Calls => "calls",
            Reason::ReturnsUndefined => "returns-undefined",
            Reason::ReturnsNonNumber => "returns-non-number",
            Reason::NotTyped => "not-typed",
            Reason::CloneRejected => "clone-rejected",
            Reason::CallbackUnknown => "callback-unknown",
            Reason::ExtraArguments => "extra-arguments",
            Reason::GenericOnly => "generic-only",
            Reason::DevProfile => "dev-profile",
            Reason::ArgumentShape => "argument-shape",
            Reason::ComparatorUnproven => "comparator-unproven",
            Reason::ReadsAccessor => "reads-accessor",
            Reason::AllocatesForMethod => "allocates-for-method",
            Reason::CallsImpure => "calls-impure",
            Reason::CallsUnproven => "calls-unproven",
            Reason::CallsUnknown => "calls-unknown",
            Reason::CallsTooDeep => "calls-too-deep",
        }
    }

    /// The reason whose code is `code`.
    pub fn from_code(code: &str) -> Option<Reason> {
        Reason::ALL.iter().copied().find(|r| r.code() == code)
    }

    /// One line a person reads.
    pub const fn describe(self) -> &'static str {
        match self {
            Reason::NotPlainFunction => "the callback is a generator or async function",
            Reason::Arity => {
                "the callback does not declare the parameters this method passes (one or two; \
                 two or three for parallelReduce's callback, two for its combine)"
            }
            Reason::RestParameter => "the callback reads a rest parameter",
            Reason::ExceptionHandler => "the callback has a try block",
            Reason::Throws => "the callback can throw",
            Reason::NonNumericConstant => "the callback uses a string or bigint literal",
            Reason::UnsupportedOperation => {
                "the callback uses an operation parallel chunks do not run"
            }
            Reason::CapturesReassigned => {
                "the callback captures a binding that is assigned more than once"
            }
            Reason::CapturesUnresolved => {
                "the callback captures a binding whose writes cannot be traced"
            }
            Reason::WritesCapture => "the callback assigns a captured binding",
            Reason::ReadsGlobal => "the callback reads or writes a global",
            Reason::ReadsThis => "the callback reads `this`, `new.target` or `arguments`",
            Reason::ReadsProperty => "the callback reads a property or element",
            Reason::WritesProperty => "the callback writes a property or element",
            Reason::Allocates => {
                "the callback allocates what a pool thread does not build (tier 3 builds object \
                 and array literals, spreads and strings)"
            }
            Reason::Calls => "the callback calls something other than a listed Math function",
            Reason::ReturnsUndefined => "the callback can return undefined",
            Reason::ReturnsNonNumber => "the callback is not proven to return a number",
            Reason::NotTyped => "the callback has no typed entry",
            Reason::CloneRejected => "the emitted parallel body reaches realm state",
            Reason::CallbackUnknown => {
                "the callback is not a function literal the compiler can identify here"
            }
            Reason::ExtraArguments => "the method is passed more than the callback (a thisArg)",
            Reason::GenericOnly => {
                "the module is compiled with generic bodies only and no parallel lowering"
            }
            Reason::DevProfile => {
                "this is a development build, which emits no parallel code; build with `--profile native-release`"
            }
            Reason::ArgumentShape => {
                "the call does not pass the arguments the parallel lowering takes \
                 (parallelReduce: callback, combine, initial)"
            }
            Reason::ComparatorUnproven => {
                "the comparator is not proven consistent (tier 1 knows `(a, b) => a - b` and \
                 `(a, b) => b - a`; tier 2 a difference of one key read from each, \
                 `(a, b) => a.k - b.k`)"
            }
            Reason::ReadsAccessor => {
                "the callback reads a property the program defines with a getter, which is a call"
            }
            Reason::AllocatesForMethod => {
                "the callback builds values, which only parallelMap, parallelFilter and \
                 parallelFlatMap keep (tier 3)"
            }
            Reason::CallsImpure => {
                "the callback calls a built-in that is not marked pure (tier 4 calls Math, string \
                 and number formatting methods, parseInt/parseFloat/Number, JSON.stringify and \
                 array slice/map/filter/join on plain data)"
            }
            Reason::CallsUnproven => "the callback calls a function that is not proven pure",
            Reason::CallsUnknown => {
                "the callback calls a value the compiler cannot identify as one function"
            }
            Reason::CallsTooDeep => {
                "the functions the callback calls nest deeper than the proof follows"
            }
        }
    }
}

/// The first rule a callback failed: the reason, and what it names -- a captured binding, a
/// member, an operation -- when one thing is responsible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub reason: Reason,
    pub detail: Option<String>,
    /// For [`Reason::CapturesReassigned`]: how many writes the unit has of the binding.
    pub writes: Option<usize>,
}

impl Refusal {
    pub fn new(reason: Reason) -> Self {
        Self {
            reason,
            detail: None,
            writes: None,
        }
    }

    pub fn with(reason: Reason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: Some(detail.into()),
            writes: None,
        }
    }
}

/// What the compiler knows about one captured binding of the callback (one upvalue index),
/// from the whole-unit binding walk in `direct.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// The chain ends at a binding this unit cannot enumerate every write of.
    Unresolved,
    /// Every write of the binding the unit contains, not counting the `undefined` a
    /// declaration hoists ahead of its first real write.
    Writes(usize),
}

/// What a qualifying callback needs at the call site: the captured bindings (upvalue indices,
/// ascending) whose values the realm thread reads once and hands to every chunk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParallelShape {
    pub captures: Vec<u32>,
    /// The `LoadUpvalue` values of those captures, which the plan proves numbers under the
    /// call site's run-time guard.
    pub captured_values: Vec<crate::graph::ValueId>,
    /// As a `parallelSort` comparator: the order it is proven to be, over numbers that are not
    /// `NaN` ([`comparator_order`]).
    pub comparator: Option<SortOrder>,
    /// Some reachable `return` is a boolean (the rest numbers): a predicate. Its clone answers a
    /// boxed value rather than a double, and only `parallelFilter` -- which reads the answer
    /// through `ToBoolean` -- runs it on the pool ([`parallel_result`]).
    pub boolean_result: bool,
    /// Tier 2 (ADR 0069, parallel issue 07): the callback reads the heap, and runs as its heap
    /// clone (`parallel_heap.rs`) rather than as a typed `_p` clone. `captures` and
    /// `captured_values` above are then empty: the heap clone reads its captures, of any kind,
    /// from [`HeapShape::captures`].
    pub heap: Option<HeapShape>,
}

/// What a tier-2 callback (one that reads the heap, [`heap_verdict`]) needs at its call site and
/// in its clone: the values the realm thread hands every chunk, and how many named reads each
/// worker keeps a cache for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeapShape {
    /// Captured bindings (upvalue indices, ascending), each written once in the unit, which the
    /// realm thread reads once before the pool starts -- a value of any kind, since the clone
    /// reads it through the same guarded readers as an element.
    pub captures: Vec<u32>,
    /// The constant indices of the property names the body reads by name, in first-use order:
    /// the call site hands the clone the unit's interned keys in this order.
    pub keys: Vec<u32>,
    /// The constant indices of the string literals the body uses, in first-use order.
    pub strings: Vec<u32>,
    /// How many named reads the body makes: the size of each chunk worker's site cache.
    pub sites: usize,
    /// As a `parallelSort` comparator: `(a, b) => K(a) - K(b)` (ascending) or `K(b) - K(a)`
    /// (descending), where `K` reads one element and nothing else varies between the two sides
    /// ([`key_comparator`]); the value is `K` applied to the first parameter.
    pub key: Option<(SortOrder, crate::graph::ValueId)>,
    /// Tier 3 (parallel issue 08): the callback builds values -- an object or array literal, a
    /// spread, `String(x)` or a template -- in its worker's arena.
    pub allocates: bool,
    /// Some `+` is not proven to add two numbers: on a string it joins, which allocates (in an
    /// arena where the site gives its workers one, and declines where it does not).
    pub concat: bool,
    /// Some reachable `return` is proven a string, an object or a function: an answer only
    /// `map`, `filter` and `flatMap` keep.
    pub answers_value: bool,
    /// The constant indices of the member names the body defines (object literal keys, the keys
    /// of `{ ...r, k: v }`), in first-use order: the call site interns them before the pool starts.
    pub defines: Vec<u32>,
    /// Tier 4 (parallel issue 09): the callback calls something beyond tier 2's `Math`,
    /// `charCodeAt` and `String(x)` -- a pure built-in or a function of the program.
    pub calls: bool,
    /// Tier 4: the program's functions the callback calls, directly or through each other (the
    /// callback itself too, when it is recursive), each with the first of its worker cache
    /// entries: every one gets a clone of its own for this callback
    /// (`{symbol}_fn_{callback}_hc{proto}`, [`crate::parallel_heap::emit_heap_callees`]), whose
    /// named reads use `sites[base..]`.
    pub callees: Vec<(u32, usize)>,
    /// Tier 4: the functions called through a closure value (a captured declaration), whose
    /// bodies the call site hands the workers (`env->bodies`) so a clone can check, per call,
    /// that the closure is the one the proof is about.
    pub bodies: Vec<u32>,
    /// Tier 4: what each call site in the callback and its callees calls, by the body it is in
    /// (`u32::MAX` for the callback) and the call's callee value: a function of the program.
    pub targets: std::collections::BTreeMap<(u32, u32), u32>,
}

impl HeapShape {
    /// The position of proto `index` among the bodies the call site hands over.
    pub fn body_slot(&self, index: u32) -> Option<usize> {
        self.bodies.iter().position(|&k| k == index)
    }

    /// The first worker cache entry of callee `index`'s clone.
    pub fn callee_base(&self, index: u32) -> Option<usize> {
        self.callees
            .iter()
            .find(|(k, _)| *k == index)
            .map(|(_, base)| *base)
    }

    /// The position of upvalue `index` among the captures the call site hands over.
    pub fn capture_slot(&self, index: u32) -> Option<usize> {
        self.captures.iter().position(|&k| k == index)
    }

    /// The position of the property name at constant `name` among the keys.
    pub fn key_slot(&self, name: u32) -> Option<usize> {
        self.keys.iter().position(|&k| k == name)
    }

    /// The position of the string literal at constant `index` among the strings.
    pub fn string_slot(&self, index: u32) -> Option<usize> {
        self.strings.iter().position(|&k| k == index)
    }

    /// The position of the member name at constant `index` among the defined names.
    pub fn define_slot(&self, index: u32) -> Option<usize> {
        self.defines.iter().position(|&k| k == index)
    }

    /// Tier 3's label: the callback builds values (so its site keeps them in worker arenas).
    pub fn builds(&self) -> bool {
        self.allocates || self.concat
    }
}

/// The most members an object literal a pool thread builds may have: the inline slots every
/// object starts with (`KILN_C99_INLINE_SLOTS`). More would `malloc` slots, which the sequential
/// literal does.
pub const PARALLEL_OBJECT_MEMBERS: usize = 8;

/// The most elements an array literal a pool thread builds may have: the elements an array keeps
/// inside its own block (`KILN_C99_ARRAY_INBLOCK_MAX`).
pub const PARALLEL_ARRAY_ELEMENTS: usize = 64;

/// Whether a literal member name is one a pool thread defines as the sequential fast path does:
/// not an index (which goes to elements), not `@`-prefixed (a symbol's spelling), not
/// `__proto__`, not empty.
pub fn plain_member_name(name: &str) -> bool {
    let index = !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_digit())
        && (name.len() == 1 || !name.starts_with('0'))
        && name.parse::<u64>().is_ok_and(|n| n < u64::from(u32::MAX));
    !name.is_empty() && !index && !name.starts_with('@') && name != "__proto__"
}

/// `Math`'s value properties: each is non-writable and non-configurable, so a read of one is the
/// constant whatever the program has done, and a tier-2 clone reads it as the constant.
pub const MATH_CONSTANTS: &[(&str, f64)] = &[
    ("E", std::f64::consts::E),
    ("LN10", std::f64::consts::LN_10),
    ("LN2", std::f64::consts::LN_2),
    ("LOG10E", std::f64::consts::LOG10_E),
    ("LOG2E", std::f64::consts::LOG2_E),
    ("PI", std::f64::consts::PI),
    ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
    ("SQRT2", std::f64::consts::SQRT_2),
];

/// Every property name the program defines as an accessor with the syntax that says so -- a `get`
/// or `set` in an object literal or a class body (`DefineAccessor`, its key the constant pushed
/// two instructions before, ahead of the function). A tier-2 read of such a name would very
/// likely call it, so the verdict says so at compile time ([`Reason::ReadsAccessor`]); an
/// accessor defined any other way is met at run time, where the read declines.
pub fn accessor_names(program: &Program) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for code in std::iter::once(&program.code).chain(program.functions.iter().map(|f| &f.code)) {
        for (pc, instruction) in code.iter().enumerate() {
            if !matches!(instruction, kiln_ir::Instruction::DefineAccessor { .. }) || pc < 2 {
                continue;
            }
            if let (kiln_ir::Instruction::MakeFunction(_), kiln_ir::Instruction::Const(k)) =
                (&code[pc - 1], &code[pc - 2])
                && let Some(name) = constant_name(program, *k)
            {
                names.insert(name);
            }
        }
    }
    names
}

/// Tier 2 of the proof (ADR 0069, parallel issue 07): whether `graph` (the body of `proto`) may
/// run on a pool thread while reading the heap, and what its call site hands it.
///
/// Rules 1 and 2 are tier 1's (a plain function, no handler, no throw). The rest, each with the
/// reason it refuses with:
///
/// 3. Values come from constants (numbers, booleans, `undefined`, `null`, and string literals,
///    which the clone reads as the unit's own static strings), the parameters and locals, the
///    captures of rule 4, `Phi`, and the operators -- every unary and binary operator but
///    `typeof`, `in` and `instanceof`. Their operands are not proven anything: the clone
///    converts only what converts without running code (`kiln_c99_par_number`), compares strings
///    by code units, and declines the rest at run time (a string `+` allocates; an object's
///    `valueOf` is user code).
/// 4. Captured bindings written exactly once, read once by the realm thread, of any kind; no
///    write to a capture, no global, no `this`, no `arguments`.
/// 5. Reads: `o.name` and `o[key]`, through the guarded readers in `prelude_types.h`; a name the
///    module defines as an accessor, or `__proto__`, refuses here ([`Reason::ReadsAccessor`]).
///    `Math.<constant>` is the constant. No write.
/// 6. Every reachable `return` has a value. One proven a string, an object or a function is
///    recorded ([`HeapShape::answers_value`]): `map`, `filter` and `flatMap` keep it, and the
///    site of `parallelReduce` or `parallelSort` refuses.
/// 7. Tier 3 (parallel issue 08): allocation of what a worker builds in its arena -- object
///    literals of at most [`PARALLEL_OBJECT_MEMBERS`] members whose constant names are plain
///    ([`plain_member_name`]; computed ones are checked per element), `{ ...r }` and the members
///    defined after it, array literals of at most [`PARALLEL_ARRAY_ELEMENTS`] elements,
///    `String(x)` (a template's substitutions) and `+` on strings. A closure or a cell refuses
///    ([`Reason::Allocates`]), except a closure that is only ever called (rule 8).
/// 8. Tier 4 (parallel issue 09): calls. A built-in marked pure in the member description
///    (`kiln_builtin_surface::PURE_MEMBERS`: `Math`, string methods, number formatting,
///    `parseInt`/`parseFloat`/`Number`, `JSON.stringify`, array `slice`/`map`/`filter`/`join`...)
///    -- a method by name, whatever the receiver, since the clone dispatches on the receiver's kind
///    and declines one the mark does not cover -- and a function of the program ([`CallGraph`])
///    whose own body satisfies these same rules (a `return` without a value allowed), followed to
///    [`PARALLEL_CALL_DEPTH`] levels and [`PARALLEL_CALLEES`] functions; a call back into a
///    function already being proven (recursion) is admitted, since every function of the cycle
///    is then proven by the same walk. A callee's captures are bindings written once, read on the
///    worker from the closure the call finds (whose body the clone checks), or handed down to a
///    closure written inside the callback that is only ever called. Anything else refuses:
///    [`Reason::CallsImpure`] naming the built-in, [`Reason::CallsUnproven`] naming the function
///    and why, [`Reason::CallsUnknown`], [`Reason::CallsTooDeep`].
pub fn heap_verdict(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    captures: &[Capture],
) -> Result<HeapShape, Refusal> {
    heap_verdict_in(graph, proto, program, captures, None)
}

/// How many functions deep tier 4 follows calls below the callback.
pub const PARALLEL_CALL_DEPTH: usize = 8;

/// The most functions one callback's proof admits: each gets a clone of its own.
pub const PARALLEL_CALLEES: usize = 32;

/// The most parameters a function the callback calls may declare.
pub const PARALLEL_CALLEE_PARAMETERS: u32 = 8;

/// What tier 4 needs to follow a call into the program's other functions: every proto's graph,
/// what the unit knows about each one's captures, which values are known closures, and which
/// proto the verdict is for (so a recursive call to it resolves).
pub struct CallGraph<'a> {
    pub graphs: &'a [FunctionGraph],
    pub captures: &'a [Vec<Capture>],
    /// The proto a value of proto `f`'s body is known to be the closure of.
    pub known: &'a dyn Fn(u32, crate::graph::ValueId) -> Option<u32>,
    pub root: u32,
}

/// [`heap_verdict`], with the program's call graph for tier 4's calls to its functions (without
/// one, every such call is [`Reason::CallsUnknown`]).
pub fn heap_verdict_in(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    captures: &[Capture],
    calls: Option<&CallGraph>,
) -> Result<HeapShape, Refusal> {
    if proto.generator || proto.asynchronous || proto.async_generator {
        return Err(Refusal::new(Reason::NotPlainFunction));
    }
    if proto
        .code
        .iter()
        .any(|i| matches!(i, kiln_ir::Instruction::RestArguments(_)))
    {
        return Err(Refusal::new(Reason::RestParameter));
    }
    if !(1..=3).contains(&proto.parameter_count) {
        return Err(Refusal::with(
            Reason::Arity,
            format!("{} parameters", proto.parameter_count),
        ));
    }
    let mut proof = Proof {
        program,
        calls,
        shape: HeapShape::default(),
        accessors: None,
        admitted: std::collections::HashSet::new(),
    };
    proof.body(None, graph, proto, captures, &[])?;
    let mut shape = proof.shape;
    // Rule 6: an answer the graph proves a string, an object or a function is one only `map`,
    // `filter` and `flatMap` keep; the site decides (`parallelReduce` and `parallelSort` refuse).
    let proven = crate::graph::proven_types(graph);
    for block in graph
        .blocks
        .iter()
        .filter(|b| b.is_reachable_from_entry(graph.entry_block))
    {
        if let Terminator::Return(Some(v)) = &block.terminator
            && matches!(
                proven.get(v),
                Some(Proven::String | Proven::Object | Proven::Callable)
            )
        {
            shape.answers_value = true;
        }
    }
    shape.captures.sort_unstable();
    shape.key = key_comparator(graph, proto);
    Ok(shape)
}

/// The display name of a built-in global in a reason.
fn builtin_name(builtin: Builtin) -> String {
    match builtin {
        Builtin::Json => "JSON".to_owned(),
        Builtin::ParseInt => "parseInt".to_owned(),
        Builtin::ParseFloat => "parseFloat".to_owned(),
        Builtin::GlobalThis => "globalThis".to_owned(),
        other => format!("{other:?}"),
    }
}

/// The callable globals a pure call may name (`Number(x)`, `parseInt(s, r)`), with their
/// [`kiln_builtin_surface::PURE_MEMBERS`] key.
pub fn pure_global(builtin: Builtin) -> Option<&'static str> {
    let key = match builtin {
        Builtin::Number => "Number",
        Builtin::String => "String",
        Builtin::Boolean => "Boolean",
        Builtin::ParseInt => "parseInt",
        Builtin::ParseFloat => "parseFloat",
        _ => return None,
    };
    kiln_builtin_surface::pure_member("globalThis", key).map(|m| m.key)
}

/// The name a person knows proto `index` by.
fn function_name(program: &Program, index: u32) -> String {
    program
        .functions
        .get(index as usize)
        .map(|f| f.display_name_units())
        .and_then(|units| units.to_scalar_string().ok())
        .filter(|name| !name.is_empty())
        .map_or_else(
            || "an anonymous function".to_owned(),
            |name| format!("`{name}`"),
        )
}

/// Tier 2-4's walk over one callback and every function it calls.
struct Proof<'a> {
    program: &'a Program,
    calls: Option<&'a CallGraph<'a>>,
    shape: HeapShape,
    accessors: Option<std::collections::HashSet<String>>,
    /// The protos admitted, or being admitted (a call back into one is recursion, admitted
    /// once the whole cycle is).
    admitted: std::collections::HashSet<u32>,
}

impl Proof<'_> {
    /// The rules over one body: the callback's (`owner` `None`) or a callee's, reached through
    /// `chain` (the callees above it). Its named reads take the next worker cache entries in
    /// emission order; its callees are walked after it, so each body's entries are contiguous.
    fn body(
        &mut self,
        owner: Option<u32>,
        graph: &FunctionGraph,
        proto: &FunctionProto,
        captures: &[Capture],
        chain: &[u32],
    ) -> Result<(), Refusal> {
        use crate::graph::ValueId;
        let program = self.program;
        let owner_code = owner.unwrap_or(u32::MAX);
        let math: std::collections::HashSet<ValueId> = graph
            .blocks
            .iter()
            .flat_map(|b| b.operations.iter())
            .filter_map(|(v, op)| matches!(op, GraphOp::LoadBuiltin(Builtin::Math)).then_some(*v))
            .collect();
        let label = |vid: &ValueId| {
            graph
                .origins
                .get(vid)
                .map(|o| o.label.clone())
                .unwrap_or_else(|| "operation".to_owned())
        };
        let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
            .blocks
            .iter()
            .flat_map(|b| b.operations.iter())
            .map(|(v, op)| (*v, op))
            .collect();
        // Values the clone never materialises: a built-in other than `Math` (its placeholder is
        // only ever called or used as a static's receiver) and a closure written here (only ever
        // called). Their uses are checked after the walk.
        let placeholders: std::collections::HashMap<ValueId, Option<Builtin>> = defs
            .iter()
            .filter_map(|(v, op)| match op {
                GraphOp::LoadBuiltin(Builtin::Math | Builtin::NotANumber | Builtin::Infinity) => {
                    None
                }
                GraphOp::LoadBuiltin(b) => Some((*v, Some(*b))),
                GraphOp::MakeFunction { .. } => Some((*v, None)),
                _ => None,
            })
            .collect();
        let builtin_of = |v: &ValueId| -> Option<Builtin> {
            match defs.get(v) {
                Some(GraphOp::LoadBuiltin(b)) => Some(*b),
                _ => None,
            }
        };
        // What a value called here is: a callable global, or a function of the program -- written
        // here, or a closure the unit knows.
        let proto_of = |v: &ValueId| -> Option<u32> {
            if let Some(GraphOp::MakeFunction { index, .. }) = defs.get(v) {
                return Some(*index);
            }
            let calls = self.calls?;
            (calls.known)(owner.unwrap_or(calls.root), *v)
        };
        let mut callees: Vec<u32> = Vec::new();
        // A member a literal defines: a constant name the sequential fast path defines by a
        // transition (listed, for the call site to intern, with a worker cache entry), or a computed
        // one (checked per element: an interned string that is a plain name, or the worker declines).
        let member = |key: &ValueId, shape: &mut HeapShape| -> Option<Refusal> {
            let Some(GraphOp::Const {
                index,
                constant: Constant::String(_),
            }) = defs.get(key)
            else {
                return None;
            };
            let name = constant_name(program, *index).unwrap_or_default();
            if !plain_member_name(&name) {
                return Some(Refusal::with(
                    Reason::Allocates,
                    format!("the member `{name}`"),
                ));
            }
            if !shape.defines.contains(index) {
                shape.defines.push(*index);
            }
            shape.sites += 1;
            None
        };
        for block in &graph.blocks {
            if block.exception_handler.is_some() {
                return Err(Refusal::new(Reason::ExceptionHandler));
            }
            let reachable = block.is_reachable_from_entry(graph.entry_block);
            match &block.terminator {
                Terminator::Throw(_) => return Err(Refusal::new(Reason::Throws)),
                Terminator::Return(None) if reachable && owner.is_none() => {
                    return Err(Refusal::new(Reason::ReturnsUndefined));
                }
                Terminator::Suspend { .. } => return Err(Refusal::new(Reason::NotPlainFunction)),
                _ => {}
            }
            for (vid, op) in &block.operations {
                let refusal = match op {
                    GraphOp::Const { index, constant } => match constant {
                        Constant::Number(_)
                        | Constant::Boolean(_)
                        | Constant::Undefined
                        | Constant::Null => None,
                        Constant::String(_) => {
                            if !self.shape.strings.contains(index) {
                                self.shape.strings.push(*index);
                            }
                            None
                        }
                        _ => Some(Refusal::new(Reason::NonNumericConstant)),
                    },
                    GraphOp::LoadLocal(_)
                    | GraphOp::StoreLocal { .. }
                    | GraphOp::Phi { .. }
                    | GraphOp::MarkUninitialized(_)
                    | GraphOp::CheckTdz { .. } => None,
                    // `Math` is a value; every other built-in is a placeholder, held to its uses.
                    GraphOp::LoadBuiltin(_) => None,
                    GraphOp::Unary { op, .. } => matches!(op, UnaryOp::TypeOf | UnaryOp::ToString)
                        .then(|| Refusal::with(Reason::UnsupportedOperation, format!("{op:?}"))),
                    GraphOp::Binary { op, .. } => {
                        matches!(op, kiln_ir::BinaryOp::In | kiln_ir::BinaryOp::InstanceOf)
                            .then(|| Refusal::with(Reason::UnsupportedOperation, format!("{op:?}")))
                    }
                    GraphOp::LoadUpvalue(index) => {
                        let name = upvalue_name(proto, program, *index)
                            .unwrap_or_else(|| format!("upvalue {index}"));
                        match captures.get(*index as usize) {
                            Some(Capture::Writes(1)) => {
                                if owner.is_none() && !self.shape.captures.contains(index) {
                                    self.shape.captures.push(*index);
                                }
                                None
                            }
                            Some(Capture::Writes(n)) => Some(Refusal {
                                writes: Some(*n),
                                ..Refusal::with(Reason::CapturesReassigned, name)
                            }),
                            _ => Some(Refusal::with(Reason::CapturesUnresolved, name)),
                        }
                    }
                    GraphOp::StoreUpvalue { index, .. } => Some(Refusal::with(
                        Reason::WritesCapture,
                        upvalue_name(proto, program, *index)
                            .unwrap_or_else(|| format!("upvalue {index}")),
                    )),
                    GraphOp::LoadGlobal(_) | GraphOp::StoreGlobal { .. } => {
                        Some(Refusal::new(Reason::ReadsGlobal))
                    }
                    GraphOp::LoadThis | GraphOp::LoadNewTarget => {
                        Some(Refusal::new(Reason::ReadsThis))
                    }
                    GraphOp::GetProperty { object, name, .. } => {
                        let key = constant_name(program, *name);
                        if math.contains(object)
                            && key
                                .as_deref()
                                .is_some_and(|k| MATH_CONSTANTS.iter().any(|(c, _)| *c == k))
                        {
                            None
                        } else if placeholders.contains_key(object) {
                            Some(Refusal::with(Reason::UnsupportedOperation, label(vid)))
                        } else {
                            match key {
                                None => Some(Refusal::new(Reason::ReadsProperty)),
                                Some(key) => {
                                    let accessors = self
                                        .accessors
                                        .get_or_insert_with(|| accessor_names(program));
                                    if key == "__proto__" || accessors.contains(&key) {
                                        Some(Refusal::with(Reason::ReadsAccessor, key))
                                    } else {
                                        if !self.shape.keys.contains(name) {
                                            self.shape.keys.push(*name);
                                        }
                                        self.shape.sites += 1;
                                        None
                                    }
                                }
                            }
                        }
                    }
                    GraphOp::GetComputed { .. } => None,
                    GraphOp::SetProperty { name, .. }
                    | GraphOp::SetPropertyStrict { name, .. }
                    | GraphOp::DefineOwnProperty { name, .. } => {
                        Some(match constant_name(program, *name) {
                            Some(n) => Refusal::with(Reason::WritesProperty, n),
                            None => Refusal::new(Reason::WritesProperty),
                        })
                    }
                    GraphOp::SetComputed { .. }
                    | GraphOp::SetComputedStrict { .. }
                    | GraphOp::DefineOwnComputed { .. } => {
                        Some(Refusal::new(Reason::WritesProperty))
                    }
                    // Tier 3: literals the worker builds in its arena, as the sequential literal
                    // builds them (`kiln_c99_par_object`, `kiln_c99_par_array`).
                    GraphOp::NewArray { elements, .. } => {
                        self.shape.allocates = true;
                        (elements.len() > PARALLEL_ARRAY_ELEMENTS).then(|| {
                            Refusal::with(
                                Reason::Allocates,
                                format!("an array literal of {} elements", elements.len()),
                            )
                        })
                    }
                    GraphOp::NewObject { pairs, .. } => {
                        self.shape.allocates = true;
                        if pairs.len() > PARALLEL_OBJECT_MEMBERS {
                            Some(Refusal::with(
                                Reason::Allocates,
                                format!("an object literal of {} members", pairs.len()),
                            ))
                        } else {
                            pairs
                                .iter()
                                .find_map(|(key, _)| member(key, &mut self.shape))
                        }
                    }
                    GraphOp::Bridge {
                        instruction: kiln_ir::Instruction::SpreadObject,
                        ..
                    } => {
                        self.shape.allocates = true;
                        None
                    }
                    GraphOp::Bridge {
                        instruction: kiln_ir::Instruction::DefineNamedProperty { .. },
                        operands,
                        ..
                    } if operands.len() == 3 => {
                        self.shape.allocates = true;
                        member(&operands[1], &mut self.shape)
                    }
                    // A closure written here that is only ever called (the uses check below):
                    // nothing is allocated; each call runs its clone, handed the captures it
                    // reads from this body's own.
                    GraphOp::MakeFunction { index, .. } => {
                        match program.functions.get(*index as usize) {
                            Some(inner) if self.calls.is_some() => inner
                                .upvalues
                                .iter()
                                .find_map(|up| match up {
                                    kiln_ir::Upvalue::ParentUpvalue(j)
                                    | kiln_ir::Upvalue::NamedParentUpvalue { index: j, .. } => {
                                        match captures.get(*j as usize) {
                                            Some(Capture::Writes(1)) => {
                                                if owner.is_none()
                                                    && !self.shape.captures.contains(j)
                                                {
                                                    self.shape.captures.push(*j);
                                                }
                                                None
                                            }
                                            _ => Some(Refusal::with(
                                                Reason::Allocates,
                                                "a closure capturing a binding written more than once",
                                            )),
                                        }
                                    }
                                    _ => Some(Refusal::with(
                                        Reason::Allocates,
                                        "a closure capturing a local of the callback",
                                    )),
                                }),
                            _ => Some(Refusal::with(Reason::Allocates, label(vid))),
                        }
                    }
                    GraphOp::MakeCell(_) => Some(Refusal::with(Reason::Allocates, label(vid))),
                    GraphOp::LoadCell(_) | GraphOp::StoreCell { .. } => {
                        Some(Refusal::with(Reason::Allocates, "a cell"))
                    }
                    GraphOp::CallMethod {
                        prepared_callee: Some(_),
                        ..
                    } => Some(Refusal::with(
                        Reason::CallsImpure,
                        "a method whose callee is saved before arguments",
                    )),
                    GraphOp::CallMethod {
                        object, name, args, ..
                    } => {
                        let key = constant_name(program, *name).unwrap_or_default();
                        if math.contains(object) {
                            let listed = PARALLEL_MATH
                                .iter()
                                .any(|(k, n)| key == *k && *n == args.len());
                            (!listed)
                                .then(|| Refusal::with(Reason::CallsImpure, format!("Math.{key}")))
                        } else if let Some(builtin) = builtin_of(object) {
                            let owner_name = builtin_name(builtin);
                            let pure = match builtin {
                                Builtin::Json => key == "stringify" && args.len() == 1,
                                Builtin::Number => {
                                    kiln_builtin_surface::pure_member("Number", &key).is_some()
                                        && args.len() <= 2
                                }
                                _ => false,
                            };
                            if pure {
                                self.shape.calls = true;
                                if builtin == Builtin::Json {
                                    self.shape.allocates = true;
                                }
                                None
                            } else {
                                Some(Refusal::with(
                                    Reason::CallsImpure,
                                    format!("{owner_name}.{key}"),
                                ))
                            }
                        } else if placeholders.contains_key(object) {
                            Some(Refusal::with(Reason::CallsImpure, format!(".{key}()")))
                        } else {
                            let rows: Vec<_> = kiln_builtin_surface::pure_methods(&key).collect();
                            if rows.is_empty() {
                                Some(Refusal::with(Reason::CallsImpure, format!(".{key}()")))
                            } else if rows.iter().any(|r| r.callback.is_some()) {
                                // `map`/`filter`/`some`/`every`: the callback is proven too.
                                if args.len() != 1 {
                                    Some(Refusal::with(
                                        Reason::CallsImpure,
                                        format!(".{key}() with other than one argument"),
                                    ))
                                } else if builtin_of(&args[0]).and_then(pure_global).is_some() {
                                    self.shape.calls = true;
                                    self.shape.allocates |= key == "map" || key == "filter";
                                    None
                                } else if let Some(target) = proto_of(&args[0]) {
                                    self.shape.calls = true;
                                    self.shape.allocates |= key == "map" || key == "filter";
                                    self.shape.targets.insert((owner_code, args[0].0), target);
                                    if !matches!(
                                        defs.get(&args[0]),
                                        Some(GraphOp::MakeFunction { .. })
                                    ) && !self.shape.bodies.contains(&target)
                                    {
                                        self.shape.bodies.push(target);
                                    }
                                    if !callees.contains(&target) {
                                        callees.push(target);
                                    }
                                    None
                                } else {
                                    Some(Refusal::with(
                                        Reason::CallsUnknown,
                                        format!("the callback of .{key}()"),
                                    ))
                                }
                            } else {
                                // `charCodeAt` is tier 2's; every other marked method is tier 4.
                                if key != "charCodeAt" {
                                    self.shape.calls = true;
                                }
                                if rows.iter().any(|r| {
                                    matches!(
                                        r.answers,
                                        kiln_builtin_surface::PureAnswer::String
                                            | kiln_builtin_surface::PureAnswer::Array
                                            | kiln_builtin_surface::PureAnswer::Value
                                    )
                                }) {
                                    self.shape.allocates = true;
                                }
                                None
                            }
                        }
                    }
                    GraphOp::Call { callee, args, .. } => {
                        if let Some(builtin) = builtin_of(callee) {
                            match builtin {
                                // `String(x)`: `ToString` of a primitive, into the arena.
                                Builtin::String if args.len() == 1 => {
                                    self.shape.allocates = true;
                                    None
                                }
                                b if pure_global(b).is_some() && b != Builtin::String => {
                                    self.shape.calls = true;
                                    None
                                }
                                b => Some(Refusal::with(
                                    Reason::CallsImpure,
                                    format!("{}()", builtin_name(b)),
                                )),
                            }
                        } else if let Some(target) = proto_of(callee) {
                            self.shape.calls = true;
                            self.shape.targets.insert((owner_code, callee.0), target);
                            if !matches!(defs.get(callee), Some(GraphOp::MakeFunction { .. }))
                                && !self.shape.bodies.contains(&target)
                            {
                                self.shape.bodies.push(target);
                            }
                            if !callees.contains(&target) {
                                callees.push(target);
                            }
                            None
                        } else {
                            // Name what was called: a captured binding by its name.
                            let upvalue = match defs.get(callee) {
                                Some(GraphOp::LoadUpvalue(k)) => Some(*k),
                                Some(GraphOp::CheckTdz { value, .. }) => match defs.get(value) {
                                    Some(GraphOp::LoadUpvalue(k)) => Some(*k),
                                    _ => None,
                                },
                                _ => None,
                            };
                            let named = upvalue
                                .and_then(|k| upvalue_name(proto, program, k))
                                .map_or_else(|| label(vid), |name| format!("`{name}`"));
                            Some(Refusal::with(Reason::CallsUnknown, named))
                        }
                    }
                    GraphOp::New { .. } => Some(Refusal::with(
                        Reason::CallsImpure,
                        format!("`new` ({})", label(vid)),
                    )),
                    GraphOp::Bridge { instruction, .. }
                        if matches!(instruction, kiln_ir::Instruction::LoadArguments) =>
                    {
                        Some(Refusal::with(Reason::ReadsThis, "arguments"))
                    }
                    _ => Some(Refusal::with(Reason::UnsupportedOperation, label(vid))),
                };
                if let Some(refusal) = refusal {
                    return Err(refusal);
                }
            }
        }
        // A placeholder is only ever called, or a static's receiver, or the callback of a pure
        // array method: any other use would read a value the clone never made. (A placeholder
        // called or used as a receiver the rules do not admit was refused above.)
        let array_slots = crate::graph::array_slots(graph, program);
        for (vid, op) in graph.blocks.iter().flat_map(|b| b.operations.iter()) {
            let misused = |v: &ValueId| placeholders.contains_key(v);
            let bad = match op {
                GraphOp::Call { args, .. } => args.iter().any(misused),
                // `const f = x => ...`: a local the graph forwards (every read of it is the
                // closure itself), unless the slot lives in the frame's array and is read back.
                GraphOp::StoreLocal { slot, .. } => {
                    array_slots.contains(slot) && op.operands().iter().any(misused)
                }
                GraphOp::CallMethod { name, args, .. } => {
                    let key = constant_name(program, *name).unwrap_or_default();
                    let callback = args.len() == 1
                        && kiln_builtin_surface::pure_methods(&key).any(|r| r.callback.is_some());
                    args.iter()
                        .enumerate()
                        .any(|(k, a)| misused(a) && !(k == 0 && callback))
                }
                other => other.operands().iter().any(misused),
            };
            if bad {
                let closure = op
                    .operands()
                    .iter()
                    .any(|v| matches!(placeholders.get(v), Some(None)));
                return Err(if closure {
                    Refusal::with(Reason::Allocates, "a closure that is not only called")
                } else {
                    Refusal::with(Reason::UnsupportedOperation, label(vid))
                });
            }
        }
        for block in graph.blocks.iter() {
            match &block.terminator {
                Terminator::Return(Some(v))
                | Terminator::Branch { condition: v, .. }
                | Terminator::BranchNotNullish { condition: v, .. }
                    if placeholders.contains_key(v) =>
                {
                    return Err(match placeholders.get(v) {
                        Some(Some(b)) => {
                            Refusal::with(Reason::UnsupportedOperation, builtin_name(*b))
                        }
                        _ => Refusal::with(Reason::Allocates, "a closure"),
                    });
                }
                _ => {}
            }
        }
        // A `+` of two operands not both proven numbers may join strings.
        let proven = crate::graph::proven_types(graph);
        self.shape.concat |=
            graph
                .blocks
                .iter()
                .flat_map(|b| b.operations.iter())
                .any(|(_, op)| match op {
                    GraphOp::Binary {
                        op: kiln_ir::BinaryOp::Add,
                        left,
                        right,
                    } => {
                        !(proven.get(left).is_some_and(Proven::is_number)
                            && proven.get(right).is_some_and(Proven::is_number))
                    }
                    _ => false,
                });
        // Tier 4: each function called here, after this body (so its worker cache entries follow
        // this body's), unless it is already admitted or being admitted (recursion).
        for target in callees {
            if !self.admitted.insert(target) {
                continue;
            }
            let mut path: Vec<u32> = chain.to_vec();
            path.push(target);
            let named = || {
                path.iter()
                    .map(|&t| function_name(program, t))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            };
            if path.len() > PARALLEL_CALL_DEPTH || self.admitted.len() > PARALLEL_CALLEES {
                return Err(Refusal::with(Reason::CallsTooDeep, named()));
            }
            let calls = self
                .calls
                .expect("a callee is resolved only through the call graph");
            let callee = &program.functions[target as usize];
            let unproven = |inner: Refusal| {
                let why = match &inner.detail {
                    Some(detail) => format!("{}: {detail}", inner.reason.code()),
                    None => inner.reason.code().to_owned(),
                };
                Refusal::with(Reason::CallsUnproven, format!("{} ({why})", named()))
            };
            if callee.generator || callee.asynchronous || callee.async_generator {
                return Err(unproven(Refusal::new(Reason::NotPlainFunction)));
            }
            if callee
                .code
                .iter()
                .any(|i| matches!(i, kiln_ir::Instruction::RestArguments(_)))
            {
                return Err(unproven(Refusal::new(Reason::RestParameter)));
            }
            if callee.parameter_count > PARALLEL_CALLEE_PARAMETERS {
                return Err(unproven(Refusal::with(
                    Reason::Arity,
                    format!("{} parameters", callee.parameter_count),
                )));
            }
            let Some(callee_graph) = calls.graphs.get(target as usize) else {
                return Err(Refusal::with(Reason::CallsUnknown, named()));
            };
            let callee_captures = calls
                .captures
                .get(target as usize)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let start = self.shape.sites;
            self.shape.callees.push((target, start));
            match self.body(Some(target), callee_graph, callee, callee_captures, &path) {
                Ok(()) => {}
                // Already about a call below: it names its own chain, which starts with this one.
                Err(inner)
                    if matches!(inner.reason, Reason::CallsUnproven | Reason::CallsTooDeep) =>
                {
                    return Err(inner);
                }
                Err(inner) => return Err(unproven(inner)),
            }
        }
        Ok(())
    }
}

/// The verdict of a callback tier 1 refused with `refusal`: tier 2's, when [`heap_verdict`]
/// admits it, as a [`ParallelShape`] carrying the [`HeapShape`]; otherwise tier 2's refusal,
/// since it is the more permissive tier and so names what really keeps the callback sequential.
pub fn tier_2(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    captures: &[Capture],
    refusal: Refusal,
    calls: Option<&CallGraph>,
) -> Result<ParallelShape, Refusal> {
    // What tier 2 cannot change: the function itself, and a callback that is fine by the source
    // rules but not numeric where tier 1 needs it (that is the site's or the plan's reason).
    if matches!(
        refusal.reason,
        Reason::NotPlainFunction | Reason::RestParameter | Reason::Arity
    ) {
        return Err(refusal);
    }
    heap_verdict_in(graph, proto, program, captures, calls).map(|heap| ParallelShape {
        heap: Some(heap),
        ..ParallelShape::default()
    })
}

/// The order a two-parameter comparator is when it returns the difference of one key read from
/// each element -- `(a, b) => a.k - b.k` (ascending) or `(a, b) => b.k - a.k` (descending) --
/// and the value of that key applied to the first parameter.
///
/// The body must be one straight-line block whose `return` is `L - R`, where `L` is computed
/// from one parameter and `R` from the other by the same operations (captures, constants and the
/// reads of rule 5 shared or equal), and neither side is the parameter itself (that is tier 1's
/// [`comparator_order`]). Then `compare(a, b)` is `K(a) - K(b)` for one pure `K`, and over keys
/// that are numbers and not `NaN` such a comparator is consistent exactly as `a - b` is over
/// numbers, so a stable sort by `K` is `sort`'s answer. The pool computes every key, declines
/// the call when one is not a number or is `NaN` (and when an element is `undefined` or a hole,
/// which `sort` never passes to a comparator), and sorts by key.
pub fn key_comparator(
    graph: &FunctionGraph,
    proto: &FunctionProto,
) -> Option<(SortOrder, crate::graph::ValueId)> {
    use crate::graph::ValueId;
    if proto.parameter_count != 2 {
        return None;
    }
    let mut reachable = graph
        .blocks
        .iter()
        .filter(|b| b.is_reachable_from_entry(graph.entry_block));
    let block = reachable.next()?;
    if reachable.next().is_some() {
        return None;
    }
    let Terminator::Return(Some(ret)) = &block.terminator else {
        return None;
    };
    let defs: std::collections::HashMap<ValueId, &GraphOp> =
        block.operations.iter().map(|(v, op)| (*v, op)).collect();
    if block
        .operations
        .iter()
        .any(|(_, op)| matches!(op, GraphOp::StoreLocal { slot, .. } if *slot < 2))
    {
        return None;
    }
    let parameter = |v: ValueId| -> Option<usize> {
        if let Some(k) = graph.initial_locals.iter().take(2).position(|p| *p == v) {
            return Some(k);
        }
        match defs.get(&v) {
            Some(GraphOp::LoadLocal(slot)) if *slot < 2 => Some(*slot as usize),
            _ => None,
        }
    };
    // Which parameters a value's computation reads: bit 0 the first, bit 1 the second, bit 2 a
    // value this rule does not follow (which disqualifies it).
    fn reads(
        v: ValueId,
        defs: &std::collections::HashMap<ValueId, &GraphOp>,
        parameter: &dyn Fn(ValueId) -> Option<usize>,
        depth: usize,
    ) -> u8 {
        if let Some(k) = parameter(v) {
            return 1 << k;
        }
        if depth > 64 {
            return 4;
        }
        match defs.get(&v) {
            Some(
                GraphOp::Const { .. }
                | GraphOp::LoadUpvalue(_)
                | GraphOp::LoadBuiltin(_)
                | GraphOp::LoadLocal(_),
            ) => 0,
            Some(
                op @ (GraphOp::GetProperty { .. }
                | GraphOp::GetComputed { .. }
                | GraphOp::Binary { .. }
                | GraphOp::Unary { .. }
                | GraphOp::CheckTdz { .. }
                | GraphOp::CallMethod { .. }),
            ) => op
                .operands()
                .into_iter()
                .fold(0, |acc, o| acc | reads(o, defs, parameter, depth + 1)),
            // Tier 4: a pure callable global (`parseInt(a) - parseInt(b)`), whose clone is a
            // function of its arguments alone.
            Some(op @ GraphOp::Call { callee, .. }) if matches!(defs.get(callee), Some(GraphOp::LoadBuiltin(b)) if pure_global(*b).is_some()) => {
                op.operands()
                    .into_iter()
                    .fold(0, |acc, o| acc | reads(o, defs, parameter, depth + 1))
            }
            _ => 4,
        }
    }
    fn same(
        l: ValueId,
        r: ValueId,
        defs: &std::collections::HashMap<ValueId, &GraphOp>,
        parameter: &dyn Fn(ValueId) -> Option<usize>,
        depth: usize,
    ) -> bool {
        if depth > 64 {
            return false;
        }
        match (parameter(l), parameter(r)) {
            (Some(0), Some(1)) => return true,
            (Some(_), _) | (_, Some(_)) => return false,
            _ => {}
        }
        let same_all = |a: &[ValueId], b: &[ValueId]| {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(x, y)| same(*x, *y, defs, parameter, depth + 1))
        };
        match (defs.get(&l), defs.get(&r)) {
            // A shared operand that reads no parameter (a capture, `Math`) is the same on both.
            _ if l == r => reads(l, defs, parameter, depth) == 0,
            (
                Some(GraphOp::Const { constant: a, .. }),
                Some(GraphOp::Const { constant: b, .. }),
            ) => a == b,
            (Some(GraphOp::LoadUpvalue(a)), Some(GraphOp::LoadUpvalue(b))) => a == b,
            (Some(GraphOp::LoadBuiltin(a)), Some(GraphOp::LoadBuiltin(b))) => a == b,
            (
                Some(GraphOp::GetProperty {
                    object: o1,
                    name: n1,
                    ..
                }),
                Some(GraphOp::GetProperty {
                    object: o2,
                    name: n2,
                    ..
                }),
            ) => n1 == n2 && same(*o1, *o2, defs, parameter, depth + 1),
            (Some(a @ GraphOp::GetComputed { .. }), Some(b @ GraphOp::GetComputed { .. }))
            | (Some(a @ GraphOp::CheckTdz { .. }), Some(b @ GraphOp::CheckTdz { .. })) => {
                same_all(&a.operands(), &b.operands())
            }
            (Some(a @ GraphOp::Binary { op: x, .. }), Some(b @ GraphOp::Binary { op: y, .. })) => {
                x == y && same_all(&a.operands(), &b.operands())
            }
            (Some(a @ GraphOp::Unary { op: x, .. }), Some(b @ GraphOp::Unary { op: y, .. })) => {
                x == y && same_all(&a.operands(), &b.operands())
            }
            (
                Some(a @ GraphOp::CallMethod { name: x, .. }),
                Some(b @ GraphOp::CallMethod { name: y, .. }),
            ) => x == y && same_all(&a.operands(), &b.operands()),
            (Some(a @ GraphOp::Call { .. }), Some(b @ GraphOp::Call { .. })) => {
                same_all(&a.operands(), &b.operands())
            }
            _ => false,
        }
    }
    let Some(GraphOp::Binary {
        op: kiln_ir::BinaryOp::Subtract,
        left,
        right,
    }) = defs.get(ret)
    else {
        return None;
    };
    if parameter(*left).is_some() || parameter(*right).is_some() {
        return None;
    }
    match (
        reads(*left, &defs, &parameter, 0),
        reads(*right, &defs, &parameter, 0),
    ) {
        (1, 2) if same(*left, *right, &defs, &parameter, 0) => Some((SortOrder::Ascending, *left)),
        (2, 1) if same(*right, *left, &defs, &parameter, 0) => {
            Some((SortOrder::Descending, *right))
        }
        _ => None,
    }
}

/// What every reachable `return` of a callback is proven to be, with its parameters and captures
/// seeded as numbers (the plan's settle): `Some(false)` numbers only, `Some(true)` numbers and
/// booleans with at least one boolean (a predicate), `None` anything else.
pub fn parallel_result(
    graph: &FunctionGraph,
    proven: &std::collections::HashMap<crate::graph::ValueId, Proven>,
) -> Option<bool> {
    let mut boolean = false;
    for block in graph
        .blocks
        .iter()
        .filter(|b| b.is_reachable_from_entry(graph.entry_block))
    {
        match &block.terminator {
            Terminator::Return(Some(v)) => match proven.get(v) {
                Some(k) if k.is_number() => {}
                Some(Proven::Boolean) => boolean = true,
                _ => return None,
            },
            Terminator::Return(None) => return None,
            _ => {}
        }
    }
    Some(boolean)
}

/// The kiln extensions with a parallel lowering (ADR 0058 application 2; ADR 0069; parallel
/// issue 10). Each has one sequential definition (`kiln-builtins`: `parallelMap` in the engines'
/// tables, the family in `parallel.ts`) and, at a call site whose callbacks this module proves
/// pure and numeric, a pooled lowering with the same answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParallelMethod {
    /// `map`, element for element.
    Map,
    /// `filter`: a mask computed in parallel, the kept elements copied in order.
    Filter,
    /// `flatMap` (arrays only): a numeric callback's result is never an array, so it is `map`.
    FlatMap,
    /// `parallelReduce(callback, combine, initial)`: leaves of [`REDUCE_LEAF`] folded in
    /// parallel, then combined pairwise level by level.
    Reduce,
    /// `sort`, stable: the typed-array default order, or a comparator proven consistent.
    Sort,
}

/// `parallelReduce`'s leaf (see `kiln_builtin_surface::PARALLEL_REDUCE_LEAF`).
pub const REDUCE_LEAF: usize = kiln_builtin_surface::PARALLEL_REDUCE_LEAF;

impl ParallelMethod {
    pub const ALL: [ParallelMethod; 5] = [
        ParallelMethod::Map,
        ParallelMethod::Filter,
        ParallelMethod::FlatMap,
        ParallelMethod::Reduce,
        ParallelMethod::Sort,
    ];

    /// The member name.
    pub const fn name(self) -> &'static str {
        match self {
            ParallelMethod::Map => "parallelMap",
            ParallelMethod::Filter => "parallelFilter",
            ParallelMethod::FlatMap => "parallelFlatMap",
            ParallelMethod::Reduce => "parallelReduce",
            ParallelMethod::Sort => "parallelSort",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }

    /// The arguments of a call with `argc` arguments that are callbacks the parallel lowering
    /// runs (and so need a verdict and a clone), and the parameter counts each may declare.
    /// Empty when a call of that shape has no parallel lowering.
    pub fn callbacks(self, argc: usize) -> &'static [(usize, &'static [u32])] {
        match (self, argc) {
            (ParallelMethod::Map | ParallelMethod::Filter | ParallelMethod::FlatMap, 1) => {
                &[(0, &[1, 2])]
            }
            (ParallelMethod::Reduce, 3) => &[(0, &[2, 3]), (1, &[2])],
            (ParallelMethod::Sort, 1) => &[(0, &[2])],
            _ => &[],
        }
    }
}

/// A `parallelSort` order the pool can run: the answer of a stable sort under a consistent
/// comparator is unique, so running it on the pool is `sort` (ADR 0068 rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// `(a, b) => a - b`.
    Ascending,
    /// `(a, b) => b - a`.
    Descending,
}

impl SortOrder {
    /// The provider's `order` argument (`kiln_parallel_sort`).
    pub const fn provider_code(self) -> i32 {
        match self {
            SortOrder::Ascending => 1,
            SortOrder::Descending => 2,
        }
    }
}

/// The order a two-parameter comparator is, when every reachable `return` returns the
/// difference of its two parameters -- `(a, b) => a - b` or `(a, b) => b - a`.
///
/// Over numbers none of which is `NaN` such a comparator is consistent (ECMA-262 23.1.3.30.2):
/// `a - b` has the sign of the comparison, `-0 - +0` and `Infinity - Infinity` read as equal, and
/// equal is an equivalence. The pool checks the `NaN` half at run time (`kiln_parallel_sort`
/// declines, and `sort` answers). Anything else -- a comparator with a branch, a capture, a
/// `Math` call -- may be consistent, but tier 1 does not prove it, so it is sequential.
pub fn comparator_order(graph: &FunctionGraph, proto: &FunctionProto) -> Option<SortOrder> {
    use crate::graph::ValueId;
    if proto.parameter_count != 2 {
        return None;
    }
    let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .map(|(v, op)| (*v, op))
        .collect();
    let stored: std::collections::HashSet<u32> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(_, op)| match op {
            GraphOp::StoreLocal { slot, .. } => Some(*slot),
            _ => None,
        })
        .collect();
    let parameter = |v: &ValueId| -> Option<usize> {
        if let Some(k) = graph.initial_locals.iter().take(2).position(|p| p == v) {
            return Some(k);
        }
        match defs.get(v) {
            Some(GraphOp::LoadLocal(slot)) if *slot < 2 && !stored.contains(slot) => {
                Some(*slot as usize)
            }
            _ => None,
        }
    };
    let mut order = None;
    for block in graph
        .blocks
        .iter()
        .filter(|b| b.is_reachable_from_entry(graph.entry_block))
    {
        if let Terminator::Return(value) = &block.terminator {
            let value = value.as_ref()?;
            let Some(GraphOp::Binary {
                op: kiln_ir::BinaryOp::Subtract,
                left,
                right,
            }) = defs.get(value)
            else {
                return None;
            };
            let this = match (parameter(left)?, parameter(right)?) {
                (0, 1) => SortOrder::Ascending,
                (1, 0) => SortOrder::Descending,
                _ => return None,
            };
            if order.is_some_and(|o| o != this) {
                return None;
            }
            order = Some(this);
        }
    }
    order
}

/// Whether `graph` (the body of `proto`) may be lowered to a `parallelMap` chunk, with no
/// knowledge of its captures: every captured binding counts as unresolved. The decision the
/// backend acts on is [`parallel_verdict`]; this is its `bool` for callers that have no binding
/// walk.
pub fn parallel_safe(graph: &FunctionGraph, proto: &FunctionProto, program: &Program) -> bool {
    parallel_verdict(graph, proto, program, &[]).is_ok()
}

/// The name of upvalue `index` of `proto`, when the compiler recorded one.
pub fn upvalue_name(proto: &FunctionProto, program: &Program, index: u32) -> Option<String> {
    let name = match proto.upvalues.get(index as usize)? {
        kiln_ir::Upvalue::NamedParentLocal { name, .. }
        | kiln_ir::Upvalue::NamedParentUpvalue { name, .. } => *name,
        _ => return None,
    };
    match program.constants.get(name as usize)? {
        Constant::String(s) => s.to_scalar_string().ok(),
        _ => None,
    }
}

fn constant_name(program: &Program, index: u32) -> Option<String> {
    match program.constants.get(index as usize)? {
        Constant::String(s) => s.to_scalar_string().ok(),
        _ => None,
    }
}

/// Whether `graph` (the body of `proto`) may be lowered to a `parallelMap` chunk, and if not,
/// the first rule it failed. `captures[k]` is what the unit knows about upvalue `k`; an index
/// past its end is [`Capture::Unresolved`].
///
/// The rules, each pinned by a test below and each failure by its [`Reason`]:
///
/// 1. The proto is a plain function or arrow: not a generator, not async, no rest parameter,
///    and declared with one or two parameters (the element, and optionally the index).
/// 2. No block has an exception handler and no terminator throws.
/// 3. Values come only from numeric constants, the parameters and locals (a `let`'s dead-zone mark
///    included, and a dead-zone check only of a value proven a number), the captures of rule
///    4, unary and binary arithmetic and comparison, `Phi`, and `Math.<name>` calls where
///    `<name>` with that arity is in [`PARALLEL_MATH`] and the receiver is the `Math` builtin.
/// 4. Nothing reads or writes beyond the frame, except (tier 1, ADR 0069) reading a captured
///    binding the unit writes exactly once. Its value cannot change while the call runs -- the
///    callback writes nothing and nothing else runs on the realm thread until it returns -- so
///    the call site reads it once, on the realm thread, guards that it is a number, and passes
///    it to every chunk as a `double`; workers still touch no heap. No cells, no globals, no
///    `this`, no `new.target`, no `arguments`, no write to any capture.
/// 5. No property access, no element access, no allocation, no call other than rule 3.
/// 6. Every reachable `return` returns a value (the plan then proves it a number, with the
///    captures of rule 4 assumed numbers; see `direct::DirectPlan::plan`).
///
/// The plan also requires that the parallel clone the emitter produced reaches nothing but its
/// parameters and captures -- the machine's own statement that the emitted C reaches no host,
/// receiver, upvalue table or shape row. That check lives in `lib.rs`, where the clone is
/// emitted; this function is the source-level half.
pub fn parallel_verdict(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    captures: &[Capture],
) -> Result<ParallelShape, Refusal> {
    if proto.generator || proto.asynchronous || proto.async_generator {
        return Err(Refusal::new(Reason::NotPlainFunction));
    }
    if proto
        .code
        .iter()
        .any(|i| matches!(i, kiln_ir::Instruction::RestArguments(_)))
    {
        return Err(Refusal::new(Reason::RestParameter));
    }
    // One or two (`map`, `filter`: the element and the index), up to three (`parallelReduce`:
    // the accumulator, the element and the index). Which a call site takes is the site's
    // question (`ParallelMethod::callbacks`); this is the body's.
    if !(1..=3).contains(&proto.parameter_count) {
        return Err(Refusal::with(
            Reason::Arity,
            format!("{} parameters", proto.parameter_count),
        ));
    }
    let math: std::collections::HashSet<_> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(v, op)| matches!(op, GraphOp::LoadBuiltin(Builtin::Math)).then_some(*v))
        .collect();
    // Rule 4's captures, before anything is proven: a read of a binding written once is a
    // number under the call site's guard, and is assumed one below exactly as the parameters
    // are.
    let mut shape = ParallelShape::default();
    for (vid, op) in graph.blocks.iter().flat_map(|b| b.operations.iter()) {
        if let GraphOp::LoadUpvalue(index) = op
            && matches!(captures.get(*index as usize), Some(Capture::Writes(1)))
        {
            shape.captured_values.push(*vid);
            if !shape.captures.contains(index) {
                shape.captures.push(*index);
            }
        }
    }
    shape.captures.sort_unstable();
    // Generic coercions are effectful, but parallelMap's guarded numeric clone may use the
    // corresponding machine operation when every operand is proven numeric under its numeric
    // entry assumptions. Keep that proof here rather than weakening GraphOp::effects globally.
    // The listed `Math` calls' own results are numbers too, as the plan's typed proof seeds
    // them (`direct::intrinsic_results`): the clone lowers them to bare libm, behind the call
    // site's check that no built-in was replaced.
    let mut assumptions: std::collections::HashMap<_, _> =
        crate::direct::intrinsic_results(graph, program);
    assumptions.extend(
        graph
            .initial_locals
            .iter()
            .take(proto.parameter_count as usize)
            .copied()
            .chain(shape.captured_values.iter().copied())
            .map(|value| (value, Proven::Number)),
    );
    let proven_numbers = proven_numbers_with_assumptions(graph, &assumptions);
    let label = |vid: &crate::graph::ValueId| {
        graph
            .origins
            .get(vid)
            .map(|o| o.label.clone())
            .unwrap_or_else(|| "operation".to_owned())
    };
    for block in &graph.blocks {
        if block.exception_handler.is_some() {
            return Err(Refusal::new(Reason::ExceptionHandler));
        }
        match &block.terminator {
            Terminator::Throw(_) => return Err(Refusal::new(Reason::Throws)),
            // An unreachable `return undefined` the compiler leaves after the last statement
            // is fine; one something reaches is not a number.
            Terminator::Return(None) if block.is_reachable_from_entry(graph.entry_block) => {
                return Err(Refusal::new(Reason::ReturnsUndefined));
            }
            _ => {}
        }
        for (vid, op) in &block.operations {
            let pure =
                op.effects_with_proven_numbers(&proven_numbers) == crate::graph::Effects::PURE;
            let refusal = match op {
                GraphOp::Const { constant, .. } => match constant {
                    Constant::Number(_)
                    | Constant::Boolean(_)
                    | Constant::Undefined
                    | Constant::Null => None,
                    _ => Some(Refusal::new(Reason::NonNumericConstant)),
                },
                GraphOp::LoadLocal(_) | GraphOp::StoreLocal { .. } | GraphOp::Phi { .. } => None,
                // A block-scoped `let` in the body (a loop counter, an accumulator): its dead-zone
                // mark writes the frame's own slot -- the callback has no cells, rule 5 -- and a
                // read of it needs no check once the value read is proven a number, which is
                // how the emitter lowers it too (a plain copy). A read that could still be in
                // its dead zone could throw on a worker, and is refused.
                GraphOp::MarkUninitialized(_) => None,
                GraphOp::CheckTdz { value, .. } => (!proven_numbers.contains(value))
                    .then(|| Refusal::with(Reason::UnsupportedOperation, "a dead-zone check")),
                GraphOp::LoadBuiltin(Builtin::Math) => None,
                GraphOp::LoadBuiltin(_) => {
                    Some(Refusal::with(Reason::UnsupportedOperation, label(vid)))
                }
                GraphOp::Unary { op, .. } if pure => (!matches!(
                    op,
                    UnaryOp::Plus
                        | UnaryOp::Negate
                        | UnaryOp::Not
                        | UnaryOp::BitwiseNot
                        | UnaryOp::ToNumeric
                        | UnaryOp::Increment
                        | UnaryOp::Decrement
                ))
                .then(|| Refusal::with(Reason::UnsupportedOperation, format!("{op:?}"))),
                GraphOp::Binary { .. } if pure => None,
                GraphOp::Unary { op, .. } => Some(Refusal::with(
                    Reason::UnsupportedOperation,
                    format!("{op:?} on a value not proven a number"),
                )),
                GraphOp::Binary { op, .. } => Some(Refusal::with(
                    Reason::UnsupportedOperation,
                    format!("{op:?} on a value not proven a number"),
                )),
                GraphOp::LoadUpvalue(index) => {
                    let name = upvalue_name(proto, program, *index)
                        .unwrap_or_else(|| format!("upvalue {index}"));
                    match captures.get(*index as usize) {
                        Some(Capture::Writes(1)) => None,
                        Some(Capture::Writes(n)) => Some(Refusal {
                            writes: Some(*n),
                            ..Refusal::with(Reason::CapturesReassigned, name)
                        }),
                        _ => Some(Refusal::with(Reason::CapturesUnresolved, name)),
                    }
                }
                GraphOp::StoreUpvalue { index, .. } => Some(Refusal::with(
                    Reason::WritesCapture,
                    upvalue_name(proto, program, *index)
                        .unwrap_or_else(|| format!("upvalue {index}")),
                )),
                GraphOp::LoadGlobal(_) | GraphOp::StoreGlobal { .. } => {
                    Some(Refusal::new(Reason::ReadsGlobal))
                }
                GraphOp::LoadThis | GraphOp::LoadNewTarget => Some(Refusal::new(Reason::ReadsThis)),
                GraphOp::GetProperty { name, .. } => Some(match constant_name(program, *name) {
                    Some(n) => Refusal::with(Reason::ReadsProperty, n),
                    None => Refusal::new(Reason::ReadsProperty),
                }),
                GraphOp::GetComputed { .. } => Some(Refusal::new(Reason::ReadsProperty)),
                GraphOp::SetProperty { name, .. }
                | GraphOp::SetPropertyStrict { name, .. }
                | GraphOp::DefineOwnProperty { name, .. } => {
                    Some(match constant_name(program, *name) {
                        Some(n) => Refusal::with(Reason::WritesProperty, n),
                        None => Refusal::new(Reason::WritesProperty),
                    })
                }
                GraphOp::SetComputed { .. }
                | GraphOp::SetComputedStrict { .. }
                | GraphOp::DefineOwnComputed { .. } => Some(Refusal::new(Reason::WritesProperty)),
                GraphOp::NewArray { .. }
                | GraphOp::NewObject { .. }
                | GraphOp::MakeFunction { .. }
                | GraphOp::MakeCell(_) => Some(Refusal::with(Reason::Allocates, label(vid))),
                GraphOp::LoadCell(_) | GraphOp::StoreCell { .. } => {
                    Some(Refusal::with(Reason::Allocates, "a cell"))
                }
                GraphOp::CallMethod {
                    object, name, args, ..
                } => {
                    let key = constant_name(program, *name);
                    let listed = math.contains(object)
                        && key.as_deref().is_some_and(|key| {
                            PARALLEL_MATH
                                .iter()
                                .any(|(k, n)| key == *k && *n == args.len())
                        });
                    // Whether the arguments are numbers is the plan's question (the typed
                    // proof, with the intrinsics' own results as seeds) and the clone check's:
                    // a call the clone cannot lower to bare libm names the realm and is refused.
                    if listed {
                        None
                    } else {
                        let callee = match key {
                            Some(k) if math.contains(object) => format!("Math.{k}"),
                            Some(k) => format!(".{k}()"),
                            None => "a method".to_owned(),
                        };
                        Some(Refusal::with(Reason::Calls, callee))
                    }
                }
                GraphOp::Call { .. } | GraphOp::New { .. } => {
                    Some(Refusal::with(Reason::Calls, label(vid)))
                }
                GraphOp::Bridge {
                    instruction: kiln_ir::Instruction::LoadArguments,
                    ..
                } => Some(Refusal::with(Reason::ReadsThis, "arguments")),
                _ => Some(Refusal::with(Reason::UnsupportedOperation, label(vid))),
            };
            if let Some(refusal) = refusal {
                return Err(refusal);
            }
            // Everything admitted above other than a capture read and a listed `Math` call is
            // admitted only as a pure operation, as it always was.
            if !pure
                && !matches!(
                    op,
                    GraphOp::LoadUpvalue(_)
                        | GraphOp::CallMethod { .. }
                        | GraphOp::MarkUninitialized(_)
                        | GraphOp::CheckTdz { .. }
                )
            {
                return Err(Refusal::with(Reason::UnsupportedOperation, label(vid)));
            }
        }
    }
    shape.comparator = comparator_order(graph, proto);
    Ok(shape)
}

/// One `parallelMap` call site as the emitter decided it: where it is, and either the parallel
/// lowering or the first rule that kept it sequential. The emitter writes it into the generated
/// C as a comment ([`SiteRecord::marker`]) next to the code it chose, and
/// `--explain-parallel` reads it back from there ([`SiteRecord::parse_all`]) -- so the report
/// is the decision, read off the artifact, and cannot disagree with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteRecord {
    /// Which method the site calls (`parallelMap` in a record written before the family).
    pub method: ParallelMethod,
    /// 1-based line and column of the call in the module's source (the `parallelMap` name
    /// before the callback, or the callback, or the enclosing function when neither is known).
    pub line: u32,
    pub column: u32,
    /// `None` when the site has a parallel lowering.
    pub refusal: Option<Refusal>,
    /// Tier 1: how many captured bindings a parallel callback reads.
    pub captures: usize,
    /// For a refusal about one captured binding: where the callback reads it.
    pub at: Option<(u32, u32)>,
    /// `parallelSort` with no comparator: parallel on typed arrays only.
    pub default_order: bool,
    /// Tier 2: a callback of the site reads the heap (its reads are guarded per element).
    pub heap: bool,
    /// Tier 3: the callback builds values, which its workers keep in arenas.
    pub alloc: bool,
    /// Tier 4: the callback calls pure built-ins or functions of the program.
    pub calls: bool,
}

thread_local! {
    /// The module's source and every proto's span as the program arrived, before emission packs
    /// the source for `toString` (`compact_closure_source`) and rewrites the spans into the pack:
    /// a site record names a position in the file a person has open, not in the pack. Set for
    /// the length of one unit's emission by [`with_original_source`].
    static ORIGINAL: std::cell::RefCell<Option<(std::sync::Arc<str>, Vec<kiln_ir::Span>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` -- one unit's emission -- with `program`'s source and spans recorded as the
/// positions site records report.
pub fn with_original_source<R>(program: &Program, f: impl FnOnce() -> R) -> R {
    let spans = program.functions.iter().map(|p| p.span).collect();
    let before = ORIGINAL.with(|o| o.replace(Some((program.source.clone(), spans))));
    let out = f();
    ORIGINAL.with(|o| *o.borrow_mut() = before);
    out
}

fn with_original<R>(program: &Program, f: impl FnOnce(&str, &[kiln_ir::Span]) -> R) -> R {
    ORIGINAL.with(|o| match &*o.borrow() {
        Some((source, spans)) => f(source, spans),
        None => {
            let spans: Vec<_> = program.functions.iter().map(|p| p.span).collect();
            f(&program.source, &spans)
        }
    })
}

const MARKER: &str = "/* kiln-parallel-site ";

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '%' | ' ' | '*' | '/' | '\n' | '\r' | '\t' | '=' => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 1-based line and column (in characters) of byte `offset` in `source`.
pub fn line_column(source: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(source.len());
    let mut offset = offset;
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &source[..offset];
    let line = before.matches('\n').count() as u32 + 1;
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    let column = before[line_start..].chars().count() as u32 + 1;
    (line, column)
}

impl SiteRecord {
    /// The site of a call whose callback is proto `callback` (when the compiler knows it)
    /// inside proto `enclosing` (`None` for the unit's top level), in the module's original
    /// source.
    pub fn locate(
        program: &Program,
        method: ParallelMethod,
        enclosing: Option<u32>,
        callback: Option<u32>,
    ) -> (u32, u32) {
        with_original(program, |source, spans| {
            let span = |index: u32| {
                spans
                    .get(index as usize)
                    .filter(|s| s.start <= s.end && s.end as usize <= source.len())
            };
            let at = match callback.and_then(span) {
                Some(cb) => {
                    let start = cb.start as usize;
                    // The member name ahead of the callback: `xs.parallelMap(` -- searched a
                    // short way back only, so a callback held in a variable is not given some
                    // other call's position.
                    let mut from = start.saturating_sub(96);
                    while !source.is_char_boundary(from) {
                        from += 1;
                    }
                    source[from..start]
                        .rfind(method.name())
                        .map_or(start, |i| from + i)
                }
                None => enclosing.and_then(span).map_or(0, |s| s.start as usize),
            };
            line_column(source, at)
        })
    }

    /// The site of the `ordinal`-th call of `method` that passes no callback (`parallelSort()`)
    /// inside proto `enclosing` (`None` for the top level): its name, found by counting the
    /// calls written `.{method}()` in the enclosing source, since an instruction carries no span.
    pub fn locate_bare(
        program: &Program,
        method: ParallelMethod,
        enclosing: Option<u32>,
        ordinal: usize,
    ) -> (u32, u32) {
        with_original(program, |source, spans| {
            let (start, end) = match enclosing.and_then(|i| spans.get(i as usize)) {
                Some(s) if s.start <= s.end && s.end as usize <= source.len() => {
                    (s.start as usize, s.end as usize)
                }
                _ => (0, source.len()),
            };
            let text = &source[start..end];
            let needle = format!("{}()", method.name());
            let at = text
                .match_indices(&needle)
                .nth(ordinal)
                .or_else(|| text.match_indices(&needle).next())
                .map_or(start, |(i, _)| start + i);
            line_column(source, at)
        })
    }

    /// Where the body of proto `callback` first names `name`, for a refusal about a captured
    /// binding.
    pub fn find_name(program: &Program, callback: u32, name: &str) -> Option<(u32, u32)> {
        with_original(program, |source, spans| {
            let span = spans.get(callback as usize)?;
            let (start, end) = (span.start as usize, span.end as usize);
            let text = source.get(start..end)?;
            let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
            // Skip the parameter list: the first `=>` or `{` ends it.
            let mut search = text.find("=>").or_else(|| text.find('{')).unwrap_or(0);
            while let Some(i) = text[search..].find(name) {
                let at = search + i;
                let before = text[..at].chars().next_back();
                let after = text[at + name.len()..].chars().next();
                if !before.is_some_and(ident) && !after.is_some_and(ident) {
                    return Some(line_column(source, start + at));
                }
                search = at + name.len();
            }
            None
        })
    }

    /// The comment the emitter writes at the site.
    pub fn marker(&self) -> String {
        let mut out = format!("{MARKER}line={} column={}", self.line, self.column);
        if self.method != ParallelMethod::Map {
            out.push_str(&format!(" method={}", self.method.name()));
        }
        match &self.refusal {
            None => out.push_str(&format!(" verdict=parallel captures={}", self.captures)),
            Some(r) => {
                out.push_str(&format!(" verdict=sequential reason={}", r.reason.code()));
                if let Some(detail) = &r.detail {
                    out.push_str(&format!(" detail={}", escape(detail)));
                }
                if let Some(writes) = r.writes {
                    out.push_str(&format!(" writes={writes}"));
                }
            }
        }
        if let Some((line, column)) = self.at {
            out.push_str(&format!(" at={line}:{column}"));
        }
        if self.default_order {
            out.push_str(" order=default");
        }
        if self.calls {
            out.push_str(" tier=4");
            if self.alloc {
                out.push_str(" builds=1");
            }
        } else if self.alloc {
            out.push_str(" tier=3");
        } else if self.heap {
            out.push_str(" tier=2");
        }
        out.push_str(" */");
        out
    }

    /// Every site record in a generated C source, in order, duplicates removed (a site is
    /// emitted once per clone of its body).
    pub fn parse_all(c_source: &str) -> Vec<SiteRecord> {
        let mut out: Vec<SiteRecord> = Vec::new();
        for (at, _) in c_source.match_indices(MARKER) {
            let rest = &c_source[at + MARKER.len()..];
            let Some(end) = rest.find(" */") else {
                continue;
            };
            if let Some(record) = Self::parse(&rest[..end])
                && !out.contains(&record)
            {
                out.push(record);
            }
        }
        out
    }

    fn parse(fields: &str) -> Option<SiteRecord> {
        let mut record = SiteRecord {
            method: ParallelMethod::Map,
            line: 0,
            column: 0,
            refusal: None,
            captures: 0,
            at: None,
            default_order: false,
            heap: false,
            alloc: false,
            calls: false,
        };
        let mut parallel = None;
        let mut reason = None;
        let mut detail = None;
        let mut writes = None;
        for field in fields.split(' ') {
            let (key, value) = field.split_once('=')?;
            match key {
                "method" => record.method = ParallelMethod::from_name(value)?,
                "line" => record.line = value.parse().ok()?,
                "column" => record.column = value.parse().ok()?,
                "verdict" => parallel = Some(value == "parallel"),
                "captures" => record.captures = value.parse().ok()?,
                "reason" => reason = Reason::from_code(value),
                "detail" => detail = Some(unescape(value)),
                "writes" => writes = value.parse().ok(),
                "order" => record.default_order = value == "default",
                "tier" => {
                    record.heap = value == "2" || value == "3" || value == "4";
                    record.alloc = value == "3";
                    record.calls = value == "4";
                }
                "builds" => record.alloc = value == "1",
                "at" => {
                    let (l, c) = value.split_once(':')?;
                    record.at = Some((l.parse().ok()?, c.parse().ok()?));
                }
                _ => {}
            }
        }
        if !parallel? {
            record.refusal = Some(Refusal {
                reason: reason?,
                detail,
                writes,
            });
        }
        Some(record)
    }

    /// The report lines for this site in `path`:
    ///
    /// ```text
    /// src/report.ts:42:18  parallelMap  sequential
    ///   captures-reassigned: captures `total` (src/report.ts:43:20), which is assigned 2 times
    /// ```
    pub fn render(&self, path: &str) -> String {
        let head = format!(
            "{path}:{}:{}  {}  ",
            self.line,
            self.column,
            self.method.name()
        );
        match &self.refusal {
            None if self.calls => format!("{head}parallel (tier 4: pure calls)\n"),
            None if self.alloc => {
                format!("{head}parallel (tier 3: builds values in worker arenas)\n")
            }
            None if self.heap => format!("{head}parallel (tier 2: reads the heap)\n"),
            None if self.captures > 0 => {
                format!("{head}parallel (tier 1: reads captured values)\n")
            }
            // A sort with no comparator has a parallel order only on a typed array: an array's
            // default order compares strings, and that call runs `sort`.
            None if self.default_order => {
                format!("{head}parallel (typed arrays: the default numeric order)\n")
            }
            None => format!("{head}parallel\n"),
            Some(r) => {
                let at = self
                    .at
                    .map(|(l, c)| format!(" ({path}:{l}:{c})"))
                    .unwrap_or_default();
                let why = match (r.reason, &r.detail) {
                    (Reason::CapturesReassigned, Some(name)) => format!(
                        "captures `{name}`{at}, which is assigned {} times",
                        r.writes.unwrap_or(2)
                    ),
                    (Reason::CapturesUnresolved, Some(name)) => {
                        format!("captures `{name}`{at}, whose writes cannot be traced")
                    }
                    (Reason::WritesCapture, Some(name)) => format!("assigns captured `{name}`{at}"),
                    (Reason::ReadsProperty, Some(name)) => format!("reads property `{name}`"),
                    (Reason::ReadsAccessor, Some(name)) if name == "__proto__" => {
                        "reads `__proto__`, which is an accessor".to_owned()
                    }
                    (Reason::ReadsAccessor, Some(name)) => {
                        format!("reads `{name}`, which this module defines with a getter or setter")
                    }
                    (Reason::WritesProperty, Some(name)) => format!("writes property `{name}`"),
                    (Reason::Calls, Some(callee)) => format!("calls {callee}"),
                    (Reason::CallsImpure, Some(callee)) => {
                        format!("calls {callee}, which is not marked pure")
                    }
                    (Reason::CallsUnproven, Some(chain)) => {
                        format!("calls {chain}, which is not proven pure")
                    }
                    (Reason::CallsUnknown, Some(callee)) => {
                        format!(
                            "calls {callee}, which the compiler cannot identify as one function"
                        )
                    }
                    (Reason::CallsTooDeep, Some(chain)) => format!(
                        "calls {chain}: deeper than {PARALLEL_CALL_DEPTH} functions, or more than {PARALLEL_CALLEES}"
                    ),
                    (_, Some(detail)) => format!("{} ({detail})", r.reason.describe()),
                    (_, None) => r.reason.describe().to_owned(),
                };
                format!("{head}sequential\n  {}: {why}\n", r.reason.code())
            }
        }
    }
}

/// Render a whole report: one entry per site, per module, in the order given. `<main>`, the
/// name a single-program build gives its one module, is shown as `main_path`.
pub fn render_report(modules: &[(String, Vec<SiteRecord>)], main_path: &str) -> String {
    let mut out = String::new();
    for (path, sites) in modules {
        let path = if path == "<main>" { main_path } else { path };
        let mut sites = sites.clone();
        sites.sort_by_key(|s| (s.line, s.column));
        for site in &sites {
            out.push_str(&site.render(path));
        }
    }
    out
}

/// The `parallel:` section of `kiln build --explain`, which `--explain-parallel` prints alone:
/// the report of [`render_report`], indented under its heading.
pub fn format_explain_section(modules: &[(String, Vec<SiteRecord>)], main_path: &str) -> String {
    let report = render_report(modules, main_path);
    let mut out = String::from("parallel:\n");
    if report.is_empty() {
        out.push_str("  no parallel call sites\n");
    }
    for line in report.lines() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_function_graph;
    use kiln_ir::{BYTECODE_VERSION, BinaryOp, Instruction};

    fn proto(code: Vec<Instruction>, parameter_count: u32) -> FunctionProto {
        FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            declared_length: parameter_count,
            name: Some("cb".into()),
            parameter_count,
            local_count: parameter_count + 1,
            code,
            arrow: true,
            generator: false,
            asynchronous: false,
            async_generator: false,
            is_method: false,
            is_class: false,
            mapped_arguments: false,
            upvalues: Vec::new(),
            type_annotations: None,
        }
    }

    fn program(proto: &FunctionProto) -> Program {
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            constants: vec![
                Constant::Number(1.0f64.to_bits()),
                Constant::String("sqrt".into()),
                Constant::String("sin".into()),
                Constant::String("length".into()),
                Constant::String("x".into()),
                Constant::String("random".into()),
                Constant::String("s".into()),
            ],
            code: vec![Instruction::Return],
            local_count: 0,
            functions: vec![proto.clone()],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        }
    }

    fn safe(proto: FunctionProto) -> bool {
        let program = program(&proto);
        let graph = build_function_graph(&proto, &program).unwrap();
        parallel_safe(&graph, &proto, &program)
    }

    /// The reason `proto` is refused, given what the unit knows about its captures.
    fn why_with(proto: FunctionProto, captures: &[Capture]) -> Option<Reason> {
        let program = program(&proto);
        let graph = build_function_graph(&proto, &program).unwrap();
        parallel_verdict(&graph, &proto, &program, captures)
            .err()
            .map(|r| r.reason)
    }

    fn why(proto: FunctionProto) -> Option<Reason> {
        why_with(proto, &[])
    }

    // `x => x * 2 + 1` (the constant is 1; the shape is what matters).
    fn arithmetic() -> Vec<Instruction> {
        vec![
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ]
    }

    #[test]
    fn arithmetic_over_the_parameter_qualifies() {
        assert!(safe(proto(arithmetic(), 1)));
        assert!(safe(proto(arithmetic(), 2)));
    }

    #[test]
    fn rule_1_generators_async_rest_and_arity_do_not() {
        let mut g = proto(arithmetic(), 1);
        g.generator = true;
        assert!(!safe(g));
        let mut a = proto(arithmetic(), 1);
        a.asynchronous = true;
        assert!(!safe(a));
        assert!(!safe(proto(arithmetic(), 0)));
        // Three: `parallelReduce`'s accumulator, element and index. Which a site takes is the
        // site's question (`ParallelMethod::callbacks`).
        assert!(safe(proto(arithmetic(), 3)));
        assert!(!safe(proto(arithmetic(), 4)));
        let mut rest = arithmetic();
        rest.insert(0, Instruction::RestArguments(0));
        rest.insert(1, Instruction::Pop);
        assert!(!safe(proto(rest, 1)));
    }

    #[test]
    fn rule_2_a_throw_does_not() {
        let code = vec![Instruction::Const(0), Instruction::Throw];
        assert!(!safe(proto(code, 1)));
    }

    #[test]
    fn rule_3_allowlisted_math_qualifies_and_other_math_does_not() {
        let sqrt = vec![
            Instruction::LoadIntrinsicBuiltin(Builtin::Math),
            Instruction::LoadLocal(0),
            Instruction::CallMethod { name: 1, argc: 1 },
            Instruction::Return,
        ];
        assert!(safe(proto(sqrt, 1)));
        let sin = vec![
            Instruction::LoadIntrinsicBuiltin(Builtin::Math),
            Instruction::LoadLocal(0),
            Instruction::CallMethod { name: 2, argc: 1 },
            Instruction::Return,
        ];
        assert!(safe(proto(sin, 1)));
        let random = vec![
            Instruction::LoadIntrinsicBuiltin(Builtin::Math),
            Instruction::CallMethod { name: 5, argc: 0 },
            Instruction::Return,
        ];
        assert!(!safe(proto(random, 1)));
    }

    #[test]
    fn rule_4_an_upvalue_read_does_not() {
        let code = vec![
            Instruction::LoadLocal(0),
            Instruction::LoadUpvalue(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ];
        assert!(!safe(proto(code, 1)));
    }

    #[test]
    fn rule_4_this_does_not() {
        let code = vec![Instruction::LoadThis, Instruction::Return];
        assert!(!safe(proto(code, 1)));
    }

    #[test]
    fn rule_5_property_access_and_allocation_do_not() {
        let length = vec![
            Instruction::LoadLocal(0),
            Instruction::GetProperty(3),
            Instruction::Return,
        ];
        assert!(!safe(proto(length, 1)));
        let array = vec![
            Instruction::LoadLocal(0),
            Instruction::NewArray { count: 1 },
            Instruction::Return,
        ];
        assert!(!safe(proto(array, 1)));
    }

    #[test]
    fn rule_6_a_reachable_return_without_a_value_does_not() {
        let code = vec![Instruction::Const(0), Instruction::Pop, Instruction::Return];
        // `Return` with nothing on the stack is `return undefined` reachable from the entry.
        assert!(!safe(proto(code, 1)));
    }

    // One test per reason code the source rules produce (issue 03). The plan and site reasons
    // (`returns-non-number`, `not-typed`, `clone-rejected`, `callback-unknown`,
    // `extra-arguments`, `dev-profile`) are pinned where they are decided, in `parallel_tests.rs`.

    #[test]
    fn reason_not_plain_function() {
        let mut g = proto(arithmetic(), 1);
        g.generator = true;
        assert_eq!(why(g), Some(Reason::NotPlainFunction));
        let mut a = proto(arithmetic(), 1);
        a.asynchronous = true;
        assert_eq!(why(a), Some(Reason::NotPlainFunction));
    }

    #[test]
    fn reason_arity() {
        assert_eq!(why(proto(arithmetic(), 0)), Some(Reason::Arity));
        assert_eq!(why(proto(arithmetic(), 4)), Some(Reason::Arity));
    }

    #[test]
    fn reason_rest_parameter() {
        let mut rest = arithmetic();
        rest.insert(0, Instruction::RestArguments(0));
        rest.insert(1, Instruction::Pop);
        assert_eq!(why(proto(rest, 1)), Some(Reason::RestParameter));
    }

    #[test]
    fn reason_exception_handler() {
        let code = vec![
            Instruction::PushHandler(4),
            Instruction::LoadLocal(0),
            Instruction::PopHandler,
            Instruction::Return,
            Instruction::Return,
        ];
        assert_eq!(why(proto(code, 1)), Some(Reason::ExceptionHandler));
    }

    #[test]
    fn reason_throws() {
        let code = vec![Instruction::Const(0), Instruction::Throw];
        assert_eq!(why(proto(code, 1)), Some(Reason::Throws));
    }

    #[test]
    fn reason_non_numeric_constant() {
        let code = vec![
            Instruction::Const(6),
            Instruction::LoadLocal(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ];
        assert_eq!(why(proto(code, 1)), Some(Reason::NonNumericConstant));
    }

    #[test]
    fn reason_unsupported_operation() {
        let code = vec![
            Instruction::LoadLocal(0),
            Instruction::Unary(UnaryOp::TypeOf),
            Instruction::Return,
        ];
        assert_eq!(why(proto(code, 1)), Some(Reason::UnsupportedOperation));
    }

    fn reads_capture() -> Vec<Instruction> {
        vec![
            Instruction::LoadLocal(0),
            Instruction::LoadUpvalue(0),
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::Return,
        ]
    }

    #[test]
    fn tier_1_a_capture_written_once_qualifies() {
        let program = program(&proto(reads_capture(), 1));
        let callback = proto(reads_capture(), 1);
        let graph = build_function_graph(&callback, &program).unwrap();
        let shape = parallel_verdict(&graph, &callback, &program, &[Capture::Writes(1)])
            .expect("a capture written once is read once, on the realm thread");
        assert_eq!(shape.captures, vec![0]);
        assert_eq!(shape.captured_values.len(), 1);
    }

    #[test]
    fn reason_captures_reassigned() {
        assert_eq!(
            why_with(proto(reads_capture(), 1), &[Capture::Writes(2)]),
            Some(Reason::CapturesReassigned)
        );
        // Written nowhere the unit can see is not written once either.
        assert_eq!(
            why_with(proto(reads_capture(), 1), &[Capture::Writes(0)]),
            Some(Reason::CapturesReassigned)
        );
    }

    #[test]
    fn reason_captures_unresolved() {
        assert_eq!(
            why_with(proto(reads_capture(), 1), &[Capture::Unresolved]),
            Some(Reason::CapturesUnresolved)
        );
        // No binding walk at all: today's rule, every capture refused.
        assert_eq!(
            why(proto(reads_capture(), 1)),
            Some(Reason::CapturesUnresolved)
        );
    }

    #[test]
    fn reason_writes_capture() {
        let code = vec![
            Instruction::LoadLocal(0),
            Instruction::StoreUpvalue(0),
            Instruction::LoadLocal(0),
            Instruction::Return,
        ];
        assert_eq!(
            why_with(proto(code, 1), &[Capture::Writes(1)]),
            Some(Reason::WritesCapture)
        );
    }

    #[test]
    fn reason_reads_global() {
        let code = vec![
            Instruction::LoadGlobal(0),
            Instruction::LoadLocal(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ];
        assert_eq!(why(proto(code, 1)), Some(Reason::ReadsGlobal));
    }

    #[test]
    fn reason_reads_this() {
        let code = vec![Instruction::LoadThis, Instruction::Return];
        assert_eq!(why(proto(code, 1)), Some(Reason::ReadsThis));
    }

    #[test]
    fn reason_reads_property() {
        let length = vec![
            Instruction::LoadLocal(0),
            Instruction::GetProperty(3),
            Instruction::Return,
        ];
        assert_eq!(why(proto(length, 1)), Some(Reason::ReadsProperty));
    }

    #[test]
    fn reason_writes_property() {
        let code = vec![
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(0),
            Instruction::SetProperty(4),
            Instruction::Return,
        ];
        assert_eq!(why(proto(code, 1)), Some(Reason::WritesProperty));
    }

    #[test]
    fn reason_allocates() {
        let array = vec![
            Instruction::LoadLocal(0),
            Instruction::NewArray { count: 1 },
            Instruction::Return,
        ];
        assert_eq!(why(proto(array, 1)), Some(Reason::Allocates));
    }

    #[test]
    fn reason_calls() {
        let random = vec![
            Instruction::LoadIntrinsicBuiltin(Builtin::Math),
            Instruction::CallMethod { name: 5, argc: 0 },
            Instruction::Return,
        ];
        assert_eq!(why(proto(random, 1)), Some(Reason::Calls));
    }

    #[test]
    fn reason_returns_undefined() {
        let code = vec![Instruction::Const(0), Instruction::Pop, Instruction::Return];
        assert_eq!(why(proto(code, 1)), Some(Reason::ReturnsUndefined));
    }

    #[test]
    fn a_let_binding_in_the_body_qualifies() {
        // `x => { let s; s = x; return s; }`: the dead-zone mark writes the frame's own slot.
        let code = vec![
            Instruction::MarkUninitialized(1),
            Instruction::LoadLocal(0),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(1),
            Instruction::Return,
        ];
        assert_eq!(why(proto(code, 1)), None);
    }

    #[test]
    fn reason_codes_are_distinct_and_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for reason in Reason::ALL {
            assert!(
                seen.insert(reason.code()),
                "duplicate code {}",
                reason.code()
            );
            assert_eq!(Reason::from_code(reason.code()), Some(*reason));
        }
    }

    fn comparator(code: Vec<Instruction>) -> Option<SortOrder> {
        let proto = proto(code, 2);
        let program = program(&proto);
        let graph = build_function_graph(&proto, &program).unwrap();
        parallel_verdict(&graph, &proto, &program, &[])
            .ok()
            .and_then(|shape| shape.comparator)
    }

    #[test]
    fn a_comparator_is_proven_consistent_only_as_a_difference_of_its_parameters() {
        let sub = |a, b| {
            vec![
                Instruction::LoadLocal(a),
                Instruction::LoadLocal(b),
                Instruction::Binary(BinaryOp::Subtract),
                Instruction::Return,
            ]
        };
        assert_eq!(comparator(sub(0, 1)), Some(SortOrder::Ascending));
        assert_eq!(comparator(sub(1, 0)), Some(SortOrder::Descending));
        assert_eq!(comparator(sub(0, 0)), None);
        // `a < b ? -1 : 1`-like shapes, a product, a difference of something else: unproven.
        let product = vec![
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(1),
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::Return,
        ];
        assert_eq!(comparator(product), None);
        let shifted = vec![
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::LoadLocal(1),
            Instruction::Binary(BinaryOp::Subtract),
            Instruction::Return,
        ];
        assert_eq!(comparator(shifted), None);
    }

    #[test]
    fn the_family_names_its_callbacks_by_position_and_arity() {
        for method in ParallelMethod::ALL {
            assert_eq!(ParallelMethod::from_name(method.name()), Some(method));
        }
        assert_eq!(ParallelMethod::Map.callbacks(1), &[(0, &[1u32, 2][..])]);
        assert!(ParallelMethod::Filter.callbacks(2).is_empty(), "a thisArg");
        assert_eq!(
            ParallelMethod::Reduce.callbacks(3),
            &[(0, &[2u32, 3][..]), (1, &[2u32][..])]
        );
        assert!(ParallelMethod::Reduce.callbacks(2).is_empty());
        assert_eq!(ParallelMethod::Sort.callbacks(1), &[(0, &[2u32][..])]);
        assert!(
            ParallelMethod::Sort.callbacks(0).is_empty(),
            "the default order has none"
        );
        assert_eq!(REDUCE_LEAF, 256);
    }

    #[test]
    fn site_records_round_trip_through_the_generated_c() {
        let records = vec![
            SiteRecord {
                method: ParallelMethod::Map,
                line: 6,
                column: 23,
                refusal: None,
                captures: 1,
                at: None,
                default_order: false,
                heap: false,
                alloc: false,
                calls: false,
            },
            SiteRecord {
                method: ParallelMethod::Map,
                line: 7,
                column: 23,
                refusal: Some(Refusal {
                    writes: Some(2),
                    ..Refusal::with(Reason::CapturesReassigned, "bump */ x=y%")
                }),
                captures: 0,
                at: Some((7, 54)),
                default_order: false,
                heap: false,
                alloc: false,
                calls: false,
            },
            SiteRecord {
                method: ParallelMethod::Sort,
                line: 8,
                column: 3,
                refusal: None,
                captures: 0,
                at: None,
                default_order: true,
                heap: false,
                alloc: false,
                calls: false,
            },
            SiteRecord {
                method: ParallelMethod::Reduce,
                line: 9,
                column: 3,
                refusal: Some(Refusal::new(Reason::ArgumentShape)),
                captures: 0,
                at: None,
                default_order: false,
                heap: false,
                alloc: false,
                calls: false,
            },
            SiteRecord {
                method: ParallelMethod::Filter,
                line: 10,
                column: 5,
                refusal: None,
                captures: 2,
                at: None,
                default_order: false,
                heap: true,
                alloc: false,
                calls: false,
            },
        ];
        let c: String = records
            .iter()
            .map(|r| format!("    {}\n    x = 1;\n", r.marker()))
            .collect();
        // A site emitted in two clones of one body is reported once.
        let doubled = format!("{c}{c}");
        assert_eq!(SiteRecord::parse_all(&doubled), records);
        let report = render_report(&[("<main>".to_owned(), records)], "src/a.ts");
        assert_eq!(
            report,
            "src/a.ts:6:23  parallelMap  parallel (tier 1: reads captured values)\n\
             src/a.ts:7:23  parallelMap  sequential\n  \
             captures-reassigned: captures `bump */ x=y%` (src/a.ts:7:54), which is assigned 2 times\n\
             src/a.ts:8:3  parallelSort  parallel (typed arrays: the default numeric order)\n\
             src/a.ts:9:3  parallelReduce  sequential\n  \
             argument-shape: the call does not pass the arguments the parallel lowering takes \
             (parallelReduce: callback, combine, initial)\n\
             src/a.ts:10:5  parallelFilter  parallel (tier 2: reads the heap)\n"
        );
    }

    /// Tier 2's verdict for `proto` in `program` (built from [`program`] when `None`).
    fn heap(proto: FunctionProto, program: Option<Program>) -> Result<HeapShape, Reason> {
        let program = program.unwrap_or_else(|| self::program(&proto));
        let graph = build_function_graph(&proto, &program).unwrap();
        heap_verdict(&graph, &proto, &program, &[]).map_err(|r| r.reason)
    }

    #[test]
    fn tier_2_reads_by_name_by_index_and_compares_strings() {
        // `x => (x.x === "s") + x.x * x[1] + x.length`
        let code = vec![
            Instruction::LoadLocal(0),
            Instruction::GetProperty(4),
            Instruction::Const(6),
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::LoadLocal(0),
            Instruction::GetProperty(4),
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::GetComputed,
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::Binary(BinaryOp::Add),
            Instruction::LoadLocal(0),
            Instruction::GetProperty(3),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ];
        // Tier 1 refuses the read; tier 2 admits it, and hands the site what the clone reads.
        assert_eq!(why(proto(code.clone(), 1)), Some(Reason::ReadsProperty));
        let shape = heap(proto(code.clone(), 1), None).expect("tier 2 admits reads");
        assert_eq!(shape.keys, vec![4, 3]);
        assert_eq!(shape.strings, vec![6]);
        assert_eq!(shape.sites, 3);
        assert_eq!(shape.key, None);
        let callback = proto(code, 1);
        let program = program(&callback);
        let graph = build_function_graph(&callback, &program).unwrap();
        let refusal = parallel_verdict(&graph, &callback, &program, &[]).unwrap_err();
        let verdict = tier_2(&graph, &callback, &program, &[], refusal, None).unwrap();
        assert_eq!(verdict.heap, Some(shape));
        assert!(verdict.captures.is_empty());
    }

    #[test]
    fn tier_2_captures_of_any_kind_written_once() {
        let mut callback = proto(
            vec![
                Instruction::LoadUpvalue(0),
                Instruction::LoadLocal(0),
                Instruction::GetComputed,
                Instruction::Return,
            ],
            1,
        );
        callback.upvalues = vec![kiln_ir::Upvalue::ParentLocal(0)];
        let program = program(&callback);
        let graph = build_function_graph(&callback, &program).unwrap();
        let shape = heap_verdict(&graph, &callback, &program, &[Capture::Writes(1)]).unwrap();
        assert_eq!(shape.captures, vec![0]);
        assert_eq!(
            heap_verdict(&graph, &callback, &program, &[Capture::Writes(2)])
                .unwrap_err()
                .reason,
            Reason::CapturesReassigned
        );
    }

    #[test]
    fn tier_2_still_refuses_writes_allocation_calls_and_this() {
        let refused = |code: Vec<Instruction>| heap(proto(code, 1), None).err();
        assert_eq!(
            refused(vec![
                Instruction::LoadLocal(0),
                Instruction::Const(0),
                Instruction::SetProperty(4),
                Instruction::Return,
            ]),
            Some(Reason::WritesProperty)
        );
        // A closure is an allocation tier 3 does not build.
        assert_eq!(
            refused(vec![Instruction::MakeFunction(0), Instruction::Return]),
            Some(Reason::Allocates)
        );
        // `x.random()`: no prototype marks `random` pure (tier 4 names it).
        assert_eq!(
            refused(vec![
                Instruction::LoadLocal(0),
                Instruction::CallMethod { name: 5, argc: 0 },
                Instruction::Return,
            ]),
            Some(Reason::CallsImpure)
        );
        assert_eq!(
            refused(vec![Instruction::LoadThis, Instruction::Return]),
            Some(Reason::ReadsThis)
        );
        assert_eq!(
            refused(vec![
                Instruction::LoadLocal(0),
                Instruction::Unary(UnaryOp::TypeOf),
                Instruction::Return,
            ]),
            Some(Reason::UnsupportedOperation)
        );
        // An answer proven a string is one only `map`, `filter` and `flatMap` keep: the verdict
        // says so, and the site decides.
        let string = heap(
            proto(vec![Instruction::Const(6), Instruction::Return], 1),
            None,
        )
        .expect("the site decides what a string answer means");
        assert!(string.answers_value && !string.allocates);
    }

    #[test]
    fn tier_3_builds_literals_spreads_and_strings() {
        // `r => ({ ...r, x: r.x + "s" })`
        let code = vec![
            Instruction::NewObject { count: 0 },
            Instruction::LoadLocal(0),
            Instruction::SpreadObject,
            Instruction::Const(4),
            Instruction::LoadLocal(0),
            Instruction::GetProperty(4),
            Instruction::Const(6),
            Instruction::Binary(BinaryOp::Add),
            Instruction::DefineNamedProperty {
                name_function: true,
            },
            Instruction::Return,
        ];
        assert_eq!(why(proto(code.clone(), 1)), Some(Reason::Allocates));
        let shape = heap(proto(code, 1), None).expect("tier 3 admits the literal");
        assert!(shape.allocates && shape.concat && shape.builds());
        assert_eq!(shape.defines, vec![4], "the member the site interns");
        assert_eq!(shape.sites, 2, "one read, one definition");
        // `r => [r.x, String(r.x)]`
        let listed = heap(
            proto(
                vec![
                    Instruction::LoadLocal(0),
                    Instruction::GetProperty(4),
                    Instruction::LoadIntrinsicBuiltin(Builtin::String),
                    Instruction::LoadLocal(0),
                    Instruction::GetProperty(4),
                    Instruction::Call { argc: 1 },
                    Instruction::NewArray { count: 2 },
                    Instruction::Return,
                ],
                1,
            ),
            None,
        )
        .expect("an array literal and String()");
        assert!(listed.allocates && listed.defines.is_empty());
        // `String` used as anything but the callee of `String(x)` is a built-in the rules do
        // not list.
        assert_eq!(
            heap(
                proto(
                    vec![
                        Instruction::LoadIntrinsicBuiltin(Builtin::String),
                        Instruction::Return
                    ],
                    1
                ),
                None
            )
            .err(),
            Some(Reason::UnsupportedOperation)
        );
        // More members than an object's inline slots, and a member named like an index.
        let mut wide = Vec::new();
        for _ in 0..17 {
            wide.extend([Instruction::Const(4), Instruction::Const(0)]);
        }
        wide.extend([Instruction::NewObject { count: 17 }, Instruction::Return]);
        assert_eq!(heap(proto(wide, 1), None).err(), Some(Reason::Allocates));
        let mut index_named = program(&proto(Vec::new(), 1));
        index_named.constants.push(Constant::String("0".into()));
        let key = index_named.constants.len() as u32 - 1;
        let callback = proto(
            vec![
                Instruction::Const(key),
                Instruction::LoadLocal(0),
                Instruction::NewObject { count: 1 },
                Instruction::Return,
            ],
            1,
        );
        index_named.functions = vec![callback.clone()];
        assert_eq!(
            heap(callback, Some(index_named)).err(),
            Some(Reason::Allocates)
        );
        assert!(plain_member_name("total") && !plain_member_name("12") && !plain_member_name(""));
        assert!(plain_member_name("012") && !plain_member_name("__proto__"));
    }

    #[test]
    fn reason_allocates_for_method() {
        assert_eq!(
            Reason::from_code("allocates-for-method"),
            Some(Reason::AllocatesForMethod)
        );
        let record = SiteRecord {
            method: ParallelMethod::Map,
            line: 3,
            column: 9,
            refusal: None,
            captures: 0,
            at: None,
            default_order: false,
            heap: true,
            alloc: true,
            calls: false,
        };
        let parsed = SiteRecord::parse_all(&record.marker());
        assert_eq!(
            parsed,
            vec![record.clone()],
            "tier 3 round-trips through the marker"
        );
        assert!(
            record
                .render("m.js")
                .contains("parallel (tier 3: builds values in worker arenas)")
        );
    }

    #[test]
    fn reason_reads_accessor() {
        // The module defines `get x() {}` somewhere: a read of `x` would call it.
        let callback = proto(
            vec![
                Instruction::LoadLocal(0),
                Instruction::GetProperty(4),
                Instruction::Return,
            ],
            1,
        );
        let mut with_getter = program(&callback);
        with_getter.code = vec![
            Instruction::NewObject { count: 0 },
            Instruction::Const(4),
            Instruction::MakeFunction(0),
            Instruction::DefineAccessor {
                setter: false,
                enumerable: true,
            },
            Instruction::Return,
        ];
        assert_eq!(
            heap(callback.clone(), Some(with_getter)).err(),
            Some(Reason::ReadsAccessor)
        );
        assert!(heap(callback, None).is_ok(), "no accessor named `x`");
        // `__proto__` is an accessor of `Object.prototype` whatever the module defines.
        let mut constants = program(&proto(Vec::new(), 1)).constants;
        constants.push(Constant::String("__proto__".into()));
        let proto_read = proto(
            vec![
                Instruction::LoadLocal(0),
                Instruction::GetProperty(constants.len() as u32 - 1),
                Instruction::Return,
            ],
            1,
        );
        let mut reads_proto = program(&proto_read);
        reads_proto.constants = constants;
        assert_eq!(
            heap(proto_read, Some(reads_proto)).err(),
            Some(Reason::ReadsAccessor)
        );
    }

    #[test]
    fn a_key_comparator_is_a_difference_of_one_key_read_from_each_side() {
        let key = |code: Vec<Instruction>| {
            let callback = proto(code, 2);
            let program = program(&callback);
            let graph = build_function_graph(&callback, &program).unwrap();
            key_comparator(&graph, &callback).map(|(order, _)| order)
        };
        let difference = |left: Vec<Instruction>, right: Vec<Instruction>| {
            let mut code = left;
            code.extend(right);
            code.push(Instruction::Binary(BinaryOp::Subtract));
            code.push(Instruction::Return);
            code
        };
        let read = |parameter: u32, name: u32| {
            vec![
                Instruction::LoadLocal(parameter),
                Instruction::GetProperty(name),
            ]
        };
        // `(a, b) => a.x - b.x` and `(a, b) => b.x - a.x`.
        assert_eq!(
            key(difference(read(0, 4), read(1, 4))),
            Some(SortOrder::Ascending)
        );
        assert_eq!(
            key(difference(read(1, 4), read(0, 4))),
            Some(SortOrder::Descending)
        );
        // Different keys on the two sides, one side's key twice, and tier 1's plain difference.
        assert_eq!(key(difference(read(0, 4), read(1, 3))), None);
        assert_eq!(key(difference(read(0, 4), read(0, 4))), None);
        assert_eq!(
            key(difference(
                vec![Instruction::LoadLocal(0)],
                vec![Instruction::LoadLocal(1)]
            )),
            None
        );
        // `(a, b) => a.x * 2 - b.x * 2`: the same computation on each side.
        let scaled = |parameter| {
            let mut code = read(parameter, 4);
            code.extend([
                Instruction::Const(0),
                Instruction::Binary(BinaryOp::Multiply),
            ]);
            code
        };
        assert_eq!(
            key(difference(scaled(0), scaled(1))),
            Some(SortOrder::Ascending)
        );
        // The verdict carries it for `parallelSort`.
        let callback = proto(difference(read(0, 4), read(1, 4)), 2);
        let program = program(&callback);
        let graph = build_function_graph(&callback, &program).unwrap();
        let shape = heap_verdict(&graph, &callback, &program, &[]).unwrap();
        assert!(matches!(shape.key, Some((SortOrder::Ascending, _))));
    }

    /// A program of `functions` over `constants`, for tier 4's tests.
    fn unit(functions: Vec<FunctionProto>, constants: &[&str]) -> Program {
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            constants: constants
                .iter()
                .map(|c| Constant::String((*c).into()))
                .collect(),
            code: vec![Instruction::Return],
            local_count: 0,
            functions,
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        }
    }

    /// Tier 4's verdict for function 0 of `program`, where every captured read of a function's
    /// body is the closure `closures[f][k]` (and every capture is written once).
    fn tier_4(program: &Program, closures: &[Vec<u32>]) -> Result<HeapShape, Refusal> {
        let graphs: Vec<FunctionGraph> = program
            .functions
            .iter()
            .map(|f| build_function_graph(f, program).unwrap())
            .collect();
        let captures: Vec<Vec<Capture>> = program
            .functions
            .iter()
            .map(|f| vec![Capture::Writes(1); f.upvalues.len()])
            .collect();
        let known = |f: u32, v: crate::graph::ValueId| -> Option<u32> {
            graphs[f as usize]
                .blocks
                .iter()
                .flat_map(|b| b.operations.iter())
                .find_map(|(id, op)| match op {
                    GraphOp::LoadUpvalue(k) if *id == v => {
                        closures.get(f as usize)?.get(*k as usize).copied()
                    }
                    _ => None,
                })
        };
        let calls = CallGraph {
            graphs: &graphs,
            captures: &captures,
            known: &known,
            root: 0,
        };
        heap_verdict_in(
            &graphs[0],
            &program.functions[0],
            program,
            &captures[0],
            Some(&calls),
        )
    }

    fn function(code: Vec<Instruction>, parameters: u32, upvalues: u32) -> FunctionProto {
        let mut f = proto(code, parameters);
        f.upvalues = (0..upvalues).map(kiln_ir::Upvalue::ParentLocal).collect();
        f
    }

    #[test]
    fn tier_4_calls_built_ins_marked_pure() {
        // `l => l.trim().split(",").map(Number)`: the acceptance shape.
        let program = unit(
            vec![function(
                vec![
                    Instruction::LoadLocal(0),
                    Instruction::CallMethod { name: 0, argc: 0 },
                    Instruction::Const(2),
                    Instruction::CallMethod { name: 1, argc: 1 },
                    Instruction::LoadIntrinsicBuiltin(Builtin::Number),
                    Instruction::CallMethod { name: 3, argc: 1 },
                    Instruction::Return,
                ],
                1,
                0,
            )],
            &["trim", "split", ",", "map"],
        );
        let shape = tier_4(&program, &[]).expect("pure built-ins qualify");
        assert!(shape.calls && shape.allocates, "{shape:?}");
        assert!(shape.callees.is_empty() && shape.bodies.is_empty());
        // Without a call graph, the same holds: no function of the program is called.
        let graph = build_function_graph(&program.functions[0], &program).unwrap();
        assert!(heap_verdict(&graph, &program.functions[0], &program, &[]).is_ok());
    }

    #[test]
    fn tier_4_names_a_built_in_that_is_not_marked_pure() {
        let refused = |code: Vec<Instruction>| {
            let program = unit(
                vec![function(code, 1, 0)],
                &["push", "random", "now", "trim"],
            );
            tier_4(&program, &[]).unwrap_err()
        };
        let push = refused(vec![
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::CallMethod { name: 0, argc: 1 },
            Instruction::Return,
        ]);
        assert_eq!(
            (push.reason, push.detail.as_deref()),
            (Reason::CallsImpure, Some(".push()"))
        );
        let random = refused(vec![
            Instruction::LoadIntrinsicBuiltin(Builtin::Math),
            Instruction::CallMethod { name: 1, argc: 0 },
            Instruction::Return,
        ]);
        assert_eq!(random.detail.as_deref(), Some("Math.random"));
        let now = refused(vec![
            Instruction::LoadIntrinsicBuiltin(Builtin::Date),
            Instruction::CallMethod { name: 2, argc: 0 },
            Instruction::Return,
        ]);
        assert_eq!(
            (now.reason, now.detail.as_deref()),
            (Reason::CallsImpure, Some("Date.now"))
        );
        // A pure global passed anywhere but as a callback is a value the clone never makes.
        let passed = refused(vec![
            Instruction::LoadIntrinsicBuiltin(Builtin::Number),
            Instruction::Return,
        ]);
        assert_eq!(passed.reason, Reason::UnsupportedOperation);
    }

    #[test]
    fn tier_4_follows_calls_into_the_program() {
        let constants = &["trim", "n"];
        // 0: `x => f(x)`, capturing `f`; 1: `f = y => y.trim()`.
        let caller = || {
            function(
                vec![
                    Instruction::LoadUpvalue(0),
                    Instruction::LoadLocal(0),
                    Instruction::Call { argc: 1 },
                    Instruction::Return,
                ],
                1,
                1,
            )
        };
        let trims = function(
            vec![
                Instruction::LoadLocal(0),
                Instruction::CallMethod { name: 0, argc: 0 },
                Instruction::Return,
            ],
            1,
            0,
        );
        let program = unit(vec![caller(), trims], constants);
        let shape = tier_4(&program, &[vec![1]]).expect("a pure helper qualifies");
        assert_eq!(shape.callees, vec![(1, 0)]);
        assert_eq!(shape.bodies, vec![1]);
        assert!(shape.calls);

        // A helper that writes refuses, naming it and why.
        let writes = function(
            vec![
                Instruction::LoadLocal(0),
                Instruction::Const(0),
                Instruction::SetProperty(1),
                Instruction::LoadLocal(0),
                Instruction::Return,
            ],
            1,
            0,
        );
        let program = unit(vec![caller(), writes], constants);
        let refusal = tier_4(&program, &[vec![1]]).unwrap_err();
        assert_eq!(refusal.reason, Reason::CallsUnproven);
        assert!(
            refusal
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("writes-property: n")),
            "{refusal:?}"
        );

        // A call the compiler cannot place: no closure known for the capture.
        let program = unit(
            vec![
                caller(),
                function(vec![Instruction::LoadLocal(0), Instruction::Return], 1, 0),
            ],
            constants,
        );
        assert_eq!(
            tier_4(&program, &[]).unwrap_err().reason,
            Reason::CallsUnknown
        );
    }

    #[test]
    fn tier_4_admits_recursion_when_the_cycle_is_proven_and_bounds_depth() {
        // 0: `x => f(x)`; 1: `f = k => f(k)` (capturing itself).
        let recursive = function(
            vec![
                Instruction::LoadUpvalue(0),
                Instruction::LoadLocal(0),
                Instruction::Call { argc: 1 },
                Instruction::Return,
            ],
            1,
            1,
        );
        let program = unit(vec![recursive.clone(), recursive.clone()], &[]);
        let shape = tier_4(&program, &[vec![1], vec![1]]).expect("recursion is proven");
        assert_eq!(shape.callees, vec![(1, 0)]);
        // A chain of distinct functions past the bound: 0 calls 1 calls 2 ... calls 11.
        let chain: Vec<FunctionProto> = (0..12).map(|_| recursive.clone()).collect();
        let closures: Vec<Vec<u32>> = (0..12u32).map(|f| vec![(f + 1).min(11)]).collect();
        let program = unit(chain, &[]);
        let refusal = tier_4(&program, &closures).unwrap_err();
        assert_eq!(refusal.reason, Reason::CallsTooDeep, "{refusal:?}");
    }

    #[test]
    fn tier_4_calls_a_closure_written_in_the_callback() {
        // `l => [l].map(x => x.trim())`
        let program = unit(
            vec![
                function(
                    vec![
                        Instruction::LoadLocal(0),
                        Instruction::NewArray { count: 1 },
                        Instruction::MakeFunction(1),
                        Instruction::CallMethod { name: 1, argc: 1 },
                        Instruction::Return,
                    ],
                    1,
                    0,
                ),
                function(
                    vec![
                        Instruction::LoadLocal(0),
                        Instruction::CallMethod { name: 0, argc: 0 },
                        Instruction::Return,
                    ],
                    1,
                    0,
                ),
            ],
            &["trim", "map"],
        );
        let shape = tier_4(&program, &[]).expect("a closure only called qualifies");
        assert_eq!(shape.callees, vec![(1, 0)]);
        assert!(
            shape.bodies.is_empty(),
            "no closure is made, so none is checked"
        );
        // Returned rather than called, it is an allocation the pool does not make.
        let program = unit(
            vec![
                function(
                    vec![Instruction::MakeFunction(1), Instruction::Return],
                    1,
                    0,
                ),
                function(vec![Instruction::LoadLocal(0), Instruction::Return], 1, 0),
            ],
            &[],
        );
        assert_eq!(tier_4(&program, &[]).unwrap_err().reason, Reason::Allocates);
    }

    #[test]
    fn every_pure_member_has_a_worker_path_and_every_path_is_marked() {
        let runtime = include_str!("runtime/boxed.c");
        for member in kiln_builtin_surface::PURE_MEMBERS {
            let path =
                crate::parallel_heap::pure_path(member.owner, member.key).unwrap_or_else(|| {
                    panic!(
                        "{}.{} is marked pure with no worker path",
                        member.owner, member.key
                    )
                });
            let defined = match path {
                crate::parallel_heap::PurePath::Function(name) => {
                    format!("static int kiln_c99_par_f_{name}(")
                }
                crate::parallel_heap::PurePath::Method(name) => {
                    format!("static int kiln_c99_par_m_{name}(")
                }
                _ => continue,
            };
            assert!(runtime.contains(&defined), "{defined} is not in boxed.c");
        }
        for key in crate::parallel_heap::PURE_METHODS {
            assert!(
                kiln_builtin_surface::pure_methods(key).next().is_some(),
                "kiln_c99_par_m_{key} serves no member marked pure"
            );
        }
        for (name, _) in PARALLEL_MATH {
            assert!(
                kiln_builtin_surface::pure_member("Math", name).is_some(),
                "Math.{name}"
            );
        }
    }

    #[test]
    fn tier_4_sites_say_so() {
        let record = SiteRecord {
            method: ParallelMethod::Map,
            line: 3,
            column: 7,
            refusal: None,
            captures: 0,
            at: None,
            default_order: false,
            heap: true,
            alloc: true,
            calls: true,
        };
        let parsed = SiteRecord::parse_all(&record.marker());
        assert_eq!(parsed, vec![record.clone()]);
        assert_eq!(
            record.render("a.js"),
            "a.js:3:7  parallelMap  parallel (tier 4: pure calls)\n"
        );
        let refused = SiteRecord {
            refusal: Some(Refusal::with(Reason::CallsImpure, "Math.random")),
            calls: false,
            heap: false,
            alloc: false,
            ..record
        };
        assert_eq!(
            refused.render("a.js"),
            "a.js:3:7  parallelMap  sequential\n  calls-impure: calls Math.random, which is not marked pure\n"
        );
    }

    #[test]
    fn tier_4_a_key_comparator_may_call_a_pure_global() {
        // `(a, b) => parseInt(a) - parseInt(b)`: a key, read from each side the same way.
        let code = vec![
            Instruction::LoadIntrinsicBuiltin(Builtin::ParseInt),
            Instruction::LoadLocal(0),
            Instruction::Call { argc: 1 },
            Instruction::LoadIntrinsicBuiltin(Builtin::ParseInt),
            Instruction::LoadLocal(1),
            Instruction::Call { argc: 1 },
            Instruction::Binary(BinaryOp::Subtract),
            Instruction::Return,
        ];
        let comparator = proto(code, 2);
        let program = program(&comparator);
        let graph = build_function_graph(&comparator, &program).unwrap();
        assert!(matches!(
            key_comparator(&graph, &comparator),
            Some((SortOrder::Ascending, _))
        ));
        // A call to anything else is not a key.
        let other = proto(
            vec![
                Instruction::LoadIntrinsicBuiltin(Builtin::Date),
                Instruction::LoadLocal(0),
                Instruction::Call { argc: 1 },
                Instruction::LoadIntrinsicBuiltin(Builtin::Date),
                Instruction::LoadLocal(1),
                Instruction::Call { argc: 1 },
                Instruction::Binary(BinaryOp::Subtract),
                Instruction::Return,
            ],
            2,
        );
        let graph = build_function_graph(&other, &program).unwrap();
        assert_eq!(key_comparator(&graph, &other), None);
    }
}
