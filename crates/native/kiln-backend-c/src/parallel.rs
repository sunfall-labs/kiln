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
    /// The callback has no typed entry: not directly callable, or too large.
    NotTyped,
    /// The emitted parallel clone reaches realm state (host, heap, upvalue table), so the
    /// machine check in `lib.rs` rejected it.
    CloneRejected,
    // The call site.
    /// The callback is not one function the compiler can identify at this site.
    CallbackUnknown,
    /// More than the callback was passed (a `thisArg`).
    ExtraArguments,
    /// The module is emitted with generic bodies only and without the per-site parallel
    /// lowering. No build profile produces this any more (issue 13: release modules past the
    /// specialization limit still lower their qualifying sites, and the development profile
    /// says [`Reason::DevProfile`]); the code stays so a record in an older artifact parses.
    GenericOnly,
    /// A development build (`native-dev`, `check`): it emits no typed or parallel code, by
    /// decision -- the edit-run loop stays proportional to the program, and the answer is
    /// identical either way. `--profile native-release` gives the site its parallel lowering.
    DevProfile,
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
            Reason::Arity => "the callback does not declare one or two parameters",
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
            Reason::Allocates => "the callback allocates",
            Reason::Calls => "the callback calls something other than a listed Math function",
            Reason::ReturnsUndefined => "the callback can return undefined",
            Reason::ReturnsNonNumber => "the callback is not proven to return a number",
            Reason::NotTyped => "the callback has no typed entry",
            Reason::CloneRejected => "the emitted parallel body reaches realm state",
            Reason::CallbackUnknown => {
                "the callback is not a function literal the compiler can identify here"
            }
            Reason::ExtraArguments => "parallelMap is passed more than the callback",
            Reason::GenericOnly => {
                "the module is compiled with generic bodies only and no parallel lowering"
            }
            Reason::DevProfile => {
                "this is a development build, which emits no parallel code; build with `--profile native-release`"
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
    if !(1..=2).contains(&proto.parameter_count) {
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
                GraphOp::SetProperty { name, .. } | GraphOp::SetPropertyStrict { name, .. } => {
                    Some(match constant_name(program, *name) {
                        Some(n) => Refusal::with(Reason::WritesProperty, n),
                        None => Refusal::new(Reason::WritesProperty),
                    })
                }
                GraphOp::SetComputed { .. } | GraphOp::SetComputedStrict { .. } => {
                    Some(Refusal::new(Reason::WritesProperty))
                }
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
    Ok(shape)
}

/// One `parallelMap` call site as the emitter decided it: where it is, and either the parallel
/// lowering or the first rule that kept it sequential. The emitter writes it into the generated
/// C as a comment ([`SiteRecord::marker`]) next to the code it chose, and
/// `--explain-parallel` reads it back from there ([`SiteRecord::parse_all`]) -- so the report
/// is the decision, read off the artifact, and cannot disagree with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteRecord {
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
    pub fn locate(program: &Program, enclosing: Option<u32>, callback: Option<u32>) -> (u32, u32) {
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
                        .rfind("parallelMap")
                        .map_or(start, |i| from + i)
                }
                None => enclosing.and_then(span).map_or(0, |s| s.start as usize),
            };
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
            line: 0,
            column: 0,
            refusal: None,
            captures: 0,
            at: None,
        };
        let mut parallel = None;
        let mut reason = None;
        let mut detail = None;
        let mut writes = None;
        for field in fields.split(' ') {
            let (key, value) = field.split_once('=')?;
            match key {
                "line" => record.line = value.parse().ok()?,
                "column" => record.column = value.parse().ok()?,
                "verdict" => parallel = Some(value == "parallel"),
                "captures" => record.captures = value.parse().ok()?,
                "reason" => reason = Reason::from_code(value),
                "detail" => detail = Some(unescape(value)),
                "writes" => writes = value.parse().ok(),
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
        let head = format!("{path}:{}:{}  parallelMap  ", self.line, self.column);
        match &self.refusal {
            None if self.captures > 0 => {
                format!("{head}parallel (tier 1: reads captured values)\n")
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
                    (Reason::WritesProperty, Some(name)) => format!("writes property `{name}`"),
                    (Reason::Calls, Some(callee)) => format!("calls {callee}"),
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
        out.push_str("  no parallelMap call sites\n");
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
            mapped_arguments: false,
            upvalues: Vec::new(),
            type_annotations: None,
        }
    }

    fn program(proto: &FunctionProto) -> Program {
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
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
        assert!(!safe(proto(arithmetic(), 3)));
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
            Instruction::LoadBuiltin(Builtin::Math),
            Instruction::LoadLocal(0),
            Instruction::CallMethod { name: 1, argc: 1 },
            Instruction::Return,
        ];
        assert!(safe(proto(sqrt, 1)));
        let sin = vec![
            Instruction::LoadBuiltin(Builtin::Math),
            Instruction::LoadLocal(0),
            Instruction::CallMethod { name: 2, argc: 1 },
            Instruction::Return,
        ];
        assert!(safe(proto(sin, 1)));
        let random = vec![
            Instruction::LoadBuiltin(Builtin::Math),
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
        assert_eq!(why(proto(arithmetic(), 3)), Some(Reason::Arity));
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
            Instruction::LoadBuiltin(Builtin::Math),
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

    #[test]
    fn site_records_round_trip_through_the_generated_c() {
        let records = vec![
            SiteRecord {
                line: 6,
                column: 23,
                refusal: None,
                captures: 1,
                at: None,
            },
            SiteRecord {
                line: 7,
                column: 23,
                refusal: Some(Refusal {
                    writes: Some(2),
                    ..Refusal::with(Reason::CapturesReassigned, "bump */ x=y%")
                }),
                captures: 0,
                at: Some((7, 54)),
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
             captures-reassigned: captures `bump */ x=y%` (src/a.ts:7:54), which is assigned 2 times\n"
        );
    }
}
