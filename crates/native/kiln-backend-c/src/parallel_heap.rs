//! Tier 2 of the parallel built-ins (ADR 0069, parallel issue 07): the clone of a callback that
//! reads the heap, and the chunk workers that run it on the pool.
//!
//! A tier-1 clone (`_p`) is the typed entry emitted once more, taking and returning doubles, and
//! is admitted only if its text reaches nothing a realm owns. A tier-2 callback reads objects,
//! arrays and strings, so its values are boxed; its clone (`_h`) is emitted here, from the value
//! graph, by a lowering that knows only what [`crate::parallel::heap_verdict`] admits. Every
//! value is a `kiln_val`; every read goes through the guarded readers in `prelude_types.h`
//! (`kiln_c99_par_*`), which answer exactly what the ordinary read answers or decline; every
//! operator converts only what converts without running code. A decline anywhere makes the clone
//! answer `KILN_VAL_TYPED_FAIL`, the worker sets the call's failure flag, and the call site runs
//! the sequential method from the start -- which is the same answer, with the same effects,
//! because nothing the clone did is observable.
//!
//! The clone's signature is `kiln_val {name}(const kiln_c99_par_env *env, kiln_c99_par_site
//! *sites, kiln_val p0, ...)`: `env` carries the captures, keys and string literals the realm
//! thread handed over (`HeapShape`'s order), `sites` the worker's own read cache.

use crate::graph::{FunctionGraph, GraphOp, Terminator, ValueId};
use crate::parallel::{HeapShape, MATH_CONSTANTS};
use kiln_ir::{BinaryOp, Builtin, Constant, FunctionProto, Program, UnaryOp};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

/// The suffixes of a tier-2 callback's chunk workers, `{symbol}_pmap{suffix}_{index}`, in the
/// order they are declared. Which a proto gets follows from what it can be called as
/// ([`heap_chunk_kinds`]); their contracts are on [`emit_heap_workers`].
pub const HEAP_CHUNK_KINDS: [&str; 9] = [
    "_hmap",
    "_hmap_typed",
    "_hfilt",
    "_hfilt_typed",
    "_hfold",
    "_hfold_typed",
    "_hcomb",
    "_hkey",
    "_hrank",
];

/// The workers a tier-2 proto declaring `parameter_count` parameters gets: `map`'s and
/// `filter`'s for one or two (the element, the index), `parallelReduce`'s fold for two or three
/// (the accumulator, the element, the index) and its combine for two, and a key comparator's
/// key and rank workers.
///
/// A callback that builds values (tier 3, [`crate::parallel::HeapShape::allocates`]) gets only the
/// array workers, whose chunks have arenas: every other worker would decline at its first
/// allocation, and the sites that would call them refuse it at compile time.
pub fn heap_chunk_kinds(parameter_count: u32, key: bool, allocates: bool) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    if (1..=2).contains(&parameter_count) {
        kinds.extend(["_hmap", "_hfilt"]);
        if !allocates {
            kinds.extend(["_hmap_typed", "_hfilt_typed"]);
        }
    }
    if allocates {
        return kinds;
    }
    if (2..=3).contains(&parameter_count) {
        kinds.extend(["_hfold", "_hfold_typed"]);
    }
    if parameter_count == 2 {
        kinds.push("_hcomb");
    }
    if key {
        kinds.extend(["_hkey", "_hrank"]);
    }
    kinds
}

/// The C expression of a listed `Math` function over the doubles `a` and `b` (`b` unused for a
/// one-argument function): the expressions the tier-1 clone and the guarded intrinsics compute,
/// so the three agree bit for bit on every edge.
pub fn math_expression(name: &str, a: &str, b: &str) -> Option<String> {
    Some(match name {
        "sqrt" => format!("sqrt({a})"),
        "abs" => format!("fabs({a})"),
        "floor" => format!("floor({a})"),
        "ceil" => format!("ceil({a})"),
        "trunc" => format!("trunc({a})"),
        "round" => format!(
            "({{ double r = isfinite({a}) ? ({a} - floor({a}) >= 0.5 ? floor({a}) + 1.0 : floor({a})) : {a}; if (r == 0.0 && signbit({a})) r = -0.0; r; }})"
        ),
        "sign" => format!("(isnan({a}) || {a} == 0.0 ? {a} : ({a} < 0.0 ? -1.0 : 1.0))"),
        "pow" => format!("kiln_c99_js_pow({a}, {b})"),
        "min" => format!(
            "((isnan({a}) || isnan({b})) ? NAN : (({a} < {b} || ({a} == 0.0 && {b} == 0.0 && signbit({a}))) ? {a} : {b}))"
        ),
        "max" => format!(
            "((isnan({a}) || isnan({b})) ? NAN : (({a} > {b} || ({a} == 0.0 && {b} == 0.0 && !signbit({a}))) ? {a} : {b}))"
        ),
        "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "sinh" | "cosh" | "tanh" | "asinh"
        | "acosh" | "atanh" | "exp" | "expm1" | "log" | "log2" | "log10" | "log1p" | "cbrt" => {
            format!("{name}({a})")
        }
        "atan2" => format!("atan2({a}, {b})"),
        "hypot" => format!("hypot({a}, {b})"),
        _ => return None,
    })
}

fn constant_text(program: &Program, index: u32) -> Option<String> {
    match program.constants.get(index as usize)? {
        Constant::String(s) => s.to_scalar_string().ok(),
        _ => None,
    }
}

fn number_literal(value: f64) -> String {
    if value.is_nan() {
        "KILN_QNAN_BASE".to_owned()
    } else {
        format!("UINT64_C({})", value.to_bits())
    }
}

/// The values `key`'s computation reads, `key` included: the operations a key comparator's key
/// clone emits.
fn cone(graph: &FunctionGraph, key: ValueId) -> HashSet<ValueId> {
    let defs: HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .map(|(v, op)| (*v, op))
        .collect();
    let mut seen = HashSet::new();
    let mut work = vec![key];
    while let Some(v) = work.pop() {
        if !seen.insert(v) {
            continue;
        }
        if let Some(op) = defs.get(&v) {
            work.extend(op.operands());
        }
    }
    seen
}

/// Emit the tier-2 clone of `graph` (the body of `proto`) as the C function `name`.
///
/// With `key`, the key comparator's key: only the computation of that value (one straight-line
/// block, [`crate::parallel::key_comparator`]), taking one element, answering the key.
/// Which body a heap clone is emitted for (tier 4, parallel issue 09): the callback's own, or one
/// of the functions it calls, each of which gets a clone of its own per callback.
#[derive(Debug, Clone, Copy)]
pub struct CloneContext<'a> {
    /// The unit's C symbol prefix (`{symbol}_fn_{index}` are its bodies).
    pub symbol: &'a str,
    /// The callback's proto, which names its callees' clones.
    pub root: u32,
    /// `Some(t)` for callee `t`'s clone, `None` for the callback's.
    pub callee: Option<u32>,
}

/// The C name of callee `target`'s clone for the callback `root`.
pub fn callee_clone_name(symbol: &str, root: u32, target: u32) -> String {
    format!("{symbol}_fn_{root}_hc{target}")
}

/// The parameter list of a callee clone taking `parameters` arguments: the callback's env, sites
/// and arena, the captures its closure holds (`ups`, by upvalue index), and the call depth.
fn callee_parameters(parameters: u32) -> String {
    let mut params = String::from(
        "const kiln_c99_par_env *env, kiln_c99_par_site *sites, kiln_c99_par_arena *arena, const kiln_val *ups, unsigned depth",
    );
    for k in 0..parameters {
        write!(params, ", kiln_val p{k}").unwrap();
    }
    params
}

/// The clones of every function the callback `root` calls (tier 4), each declared first so they
/// may call each other in any order, recursion included.
pub fn emit_heap_callees(
    output: &mut String,
    graphs: &[FunctionGraph],
    program: &Program,
    symbol: &str,
    root: u32,
    shape: &HeapShape,
) {
    for &(target, _) in &shape.callees {
        let proto = &program.functions[target as usize];
        writeln!(
            output,
            "static __attribute__((unused)) kiln_val {}({});",
            callee_clone_name(symbol, root, target),
            callee_parameters(proto.parameter_count)
        )
        .unwrap();
    }
    for &(target, _) in &shape.callees {
        emit_heap_clone(
            output,
            &graphs[target as usize],
            &program.functions[target as usize],
            program,
            &callee_clone_name(symbol, root, target),
            shape,
            None,
            &CloneContext {
                symbol,
                root,
                callee: Some(target),
            },
        );
    }
}

/// The C function a pure built-in runs as on a pool thread (tier 4): every
/// `kiln_builtin_surface::PURE_MEMBERS` row has one, and a test holds the two to each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurePath {
    /// A listed `Math` function, inline ([`math_expression`]).
    Math,
    /// `String.prototype.charCodeAt`, tier 2's inline reader.
    CharCodeAt,
    /// `kiln_c99_par_f_<name>(env, arena, argc, args, &out)`.
    Function(&'static str),
    /// `kiln_c99_par_m_<key>(env, arena, self, argc, args, &out)`, dispatched on the receiver.
    Method(&'static str),
    /// `map`, `filter`, `some`, `every`: a loop in the clone, calling the callback's clone.
    Loop(&'static str),
}

/// The worker path of the pure member `owner.key`.
pub fn pure_path(owner: &str, key: &str) -> Option<PurePath> {
    use PurePath::*;
    let path = match (owner, key) {
        ("Math", k) => {
            return crate::parallel::PARALLEL_MATH
                .iter()
                .any(|(name, _)| *name == k)
                .then_some(Math);
        }
        ("globalThis", "Number") => Function("Number"),
        ("globalThis", "String") => Function("String"),
        ("globalThis", "Boolean") => Function("Boolean"),
        ("globalThis", "parseInt") => Function("parseInt"),
        ("globalThis", "parseFloat") => Function("parseFloat"),
        ("Number", "isInteger") => Function("Number_isInteger"),
        ("Number", "isSafeInteger") => Function("Number_isSafeInteger"),
        ("Number", "isNaN") => Function("Number_isNaN"),
        ("Number", "isFinite") => Function("Number_isFinite"),
        ("Number", "parseInt") => Function("Number_parseInt"),
        ("Number", "parseFloat") => Function("Number_parseFloat"),
        ("JSON", "stringify") => Function("JSON_stringify"),
        ("String.prototype", "charCodeAt") => CharCodeAt,
        ("Array.prototype", "map") => Loop("map"),
        ("Array.prototype", "filter") => Loop("filter"),
        ("Array.prototype", "some") => Loop("some"),
        ("Array.prototype", "every") => Loop("every"),
        ("String.prototype" | "Number.prototype" | "Array.prototype", k) => {
            return PURE_METHODS.iter().find(|m| **m == k).map(|m| Method(m));
        }
        _ => return None,
    };
    Some(path)
}

/// The keys `kiln_c99_par_m_<key>` exists for (`boxed.c`).
pub const PURE_METHODS: &[&str] = &[
    "trim",
    "trimStart",
    "trimEnd",
    "toUpperCase",
    "toLowerCase",
    "at",
    "charAt",
    "codePointAt",
    "substring",
    "slice",
    "indexOf",
    "includes",
    "lastIndexOf",
    "startsWith",
    "endsWith",
    "padStart",
    "padEnd",
    "repeat",
    "split",
    "toString",
    "toFixed",
    "join",
];

/// The `kiln_c99_par_f_*` name of a callable global's pure path.
fn global_function(builtin: Builtin) -> Option<&'static str> {
    let key = crate::parallel::pure_global(builtin)?;
    match pure_path("globalThis", key)? {
        PurePath::Function(name) => Some(name),
        _ => None,
    }
}

/// A C array literal of `args` (`NULL, 0` when there are none): `(decl, pointer, count)`.
fn argument_list(args: &[String], name: &str) -> (String, String, usize) {
    if args.is_empty() {
        (String::new(), "NULL".to_owned(), 0)
    } else {
        (
            format!(
                "const kiln_val {name}[{}] = {{ {} }}; ",
                args.len(),
                args.join(", ")
            ),
            name.to_owned(),
            args.len(),
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn emit_heap_clone(
    output: &mut String,
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    name: &str,
    shape: &HeapShape,
    key: Option<ValueId>,
    ctx: &CloneContext,
) {
    let only = key.map(|k| cone(graph, k));
    let owner_code = ctx.callee.unwrap_or(u32::MAX);
    let parameters = if key.is_some() {
        1
    } else {
        proto.parameter_count
    };
    let defs: HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .map(|(v, op)| (*v, op))
        .collect();
    let math: HashSet<ValueId> = defs
        .iter()
        .filter(|(_, op)| matches!(op, GraphOp::LoadBuiltin(Builtin::Math)))
        .map(|(v, _)| *v)
        .collect();
    let blocks: Vec<_> = graph
        .blocks
        .iter()
        .filter(|b| b.is_reachable_from_entry(graph.entry_block))
        .filter(|b| key.is_none() || b.id == graph.entry_block)
        .collect();
    let array_slots: HashSet<u32> = if key.is_some() {
        HashSet::new()
    } else {
        crate::graph::array_slots(graph, program)
    };
    let emits = |v: &ValueId| only.as_ref().is_none_or(|set| set.contains(v));
    let mut values: BTreeSet<u32> = BTreeSet::new();
    for block in &blocks {
        for (v, _) in block.operations.iter().filter(|(v, _)| emits(v)) {
            values.insert(v.0);
        }
    }
    let params = if ctx.callee.is_some() {
        callee_parameters(parameters)
    } else {
        let mut params = String::from(
            "const kiln_c99_par_env * restrict env, kiln_c99_par_site * restrict sites, kiln_c99_par_arena * restrict arena",
        );
        for k in 0..parameters {
            write!(params, ", kiln_val p{k}").unwrap();
        }
        params
    };
    writeln!(
        output,
        "static __attribute__((unused)) kiln_val {name}({params}) {{"
    )
    .unwrap();
    output.push_str("    (void)env; (void)sites; (void)arena;\n");
    if ctx.callee.is_some() {
        output.push_str(
            "    (void)ups;\n    if (KILN_UNLIKELY(depth > KILN_C99_PAR_CALL_DEPTH)) return KILN_VAL_TYPED_FAIL;\n",
        );
    } else {
        output.push_str("    const unsigned depth = 0; (void)depth;\n");
    }
    for v in &values {
        writeln!(output, "    kiln_val v{v} = KILN_VAL_UNDEFINED;").unwrap();
    }
    for slot in array_slots.iter().collect::<BTreeSet<_>>() {
        let initial = if *slot < parameters {
            format!("p{slot}")
        } else {
            "KILN_VAL_UNDEFINED".to_owned()
        };
        writeln!(output, "    kiln_val l{slot} = {initial};").unwrap();
    }
    // Named reads and member definitions are numbered in emission order, one worker cache entry
    // each.
    let mut site = ctx.callee.and_then(|t| shape.callee_base(t)).unwrap_or(0);
    // The captures a closure written here needs, from this body's own: the callback's through
    // the call site's `env->caps`, a callee's from its `ups`.
    let enclosing_capture = |j: u32| -> String {
        match ctx.callee {
            Some(_) => format!("ups[{j}]"),
            None => format!(
                "env->caps[{}]",
                shape
                    .capture_slot(j)
                    .expect("the verdict lists every capture a closure written here reads")
            ),
        }
    };
    // A call to callee `target` through the value `callee` with the C expressions `args`, its
    // answer into `result`.
    let user_call = |output: &mut String,
                     target: u32,
                     callee: ValueId,
                     args: &[String],
                     result: &str| {
        let inner = &program.functions[target as usize];
        let ups = inner.upvalues.len();
        let mut text = format!("    {{ kiln_val u_c[{}]; (void)u_c;", ups.max(1));
        if let Some(GraphOp::MakeFunction { .. }) = defs.get(&callee) {
            for (i, up) in inner.upvalues.iter().enumerate() {
                let j = match up {
                    kiln_ir::Upvalue::ParentUpvalue(j)
                    | kiln_ir::Upvalue::NamedParentUpvalue { index: j, .. } => *j,
                    _ => unreachable!("the verdict admits a closure capturing only captures"),
                };
                write!(text, " u_c[{i}] = {};", enclosing_capture(j)).unwrap();
            }
        } else {
            let b = shape
                .body_slot(target)
                .expect("the verdict lists every body called through a closure");
            write!(
                text,
                " if (KILN_UNLIKELY(!KILN_VAL_IS_CLOSURE(v{c}) || ((const kiln_closure *)KILN_VAL_PTR(v{c}))->body != env->bodies[{b}])) goto kiln_hfail;",
                c = callee.0
            )
            .unwrap();
            if ups > 0 {
                write!(
                    text,
                    " {{ const kiln_closure *cl = (const kiln_closure *)KILN_VAL_PTR(v{c});",
                    c = callee.0
                )
                .unwrap();
                for i in 0..ups {
                    write!(
                        text,
                        " u_c[{i}] = *(const kiln_val *)KILN_VAL_PTR(cl->upvalues[{i}]);"
                    )
                    .unwrap();
                }
                text.push_str(" }");
            }
        }
        let mut call_args = String::new();
        for k in 0..inner.parameter_count as usize {
            let arg = args
                .get(k)
                .cloned()
                .unwrap_or_else(|| "KILN_VAL_UNDEFINED".to_owned());
            write!(call_args, ", {arg}").unwrap();
        }
        writeln!(
            text,
            " kiln_val r_c = {}(env, sites, arena, u_c, depth + 1u{call_args}); if (KILN_UNLIKELY(r_c == KILN_VAL_TYPED_FAIL)) goto kiln_hfail; {result} = r_c; }}",
            callee_clone_name(ctx.symbol, ctx.root, target)
        )
        .unwrap();
        output.push_str(&text);
    };
    // A member definition (tier 3): a constant name the call site interned, through the worker's
    // cache entry for it, or a computed key looked up per element.
    let define = |output: &mut String, site: &mut usize, target: u32, key: u32, value: u32| {
        let constant = defs.get(&ValueId(key)).and_then(|op| match op {
            GraphOp::Const {
                index,
                constant: Constant::String(_),
            } => shape.define_slot(*index),
            _ => None,
        });
        match constant {
            Some(j) => {
                writeln!(
                    output,
                    "    if (KILN_UNLIKELY(!kiln_c99_par_define(arena, &sites[{s}], v{target}, env->defines[{j}], v{value}))) goto kiln_hfail;",
                    s = *site
                )
                .unwrap();
                *site += 1;
            }
            None => writeln!(
                output,
                "    if (KILN_UNLIKELY(!kiln_c99_par_define_computed(arena, v{target}, v{key}, v{value}))) goto kiln_hfail;"
            )
            .unwrap(),
        }
    };
    let entry = graph.entry_block;
    writeln!(output, "    goto kiln_hb_{};", entry.0).unwrap();
    for block in &blocks {
        writeln!(output, "kiln_hb_{}:;", block.id.0).unwrap();
        for (vid, op) in block.operations.iter().filter(|(v, _)| emits(v)) {
            let v = vid.0;
            match op {
                GraphOp::Const { index, constant } => match constant {
                    Constant::Number(bits) => writeln!(
                        output,
                        "    v{v} = {};",
                        number_literal(f64::from_bits(*bits))
                    )
                    .unwrap(),
                    Constant::Boolean(b) => writeln!(
                        output,
                        "    v{v} = {};",
                        if *b { "KILN_VAL_TRUE" } else { "KILN_VAL_FALSE" }
                    )
                    .unwrap(),
                    Constant::Undefined => writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap(),
                    Constant::Null => writeln!(output, "    v{v} = KILN_VAL_NULL;").unwrap(),
                    Constant::String(_) => {
                        let j = shape
                            .string_slot(*index)
                            .expect("the verdict lists every string literal");
                        writeln!(output, "    v{v} = env->strs[{j}];").unwrap();
                    }
                    _ => unreachable!("the verdict admits no other constant"),
                },
                GraphOp::LoadLocal(slot) => {
                    let value = if array_slots.contains(slot) {
                        format!("l{slot}")
                    } else if *slot < parameters {
                        format!("p{slot}")
                    } else {
                        "KILN_VAL_UNDEFINED".to_owned()
                    };
                    writeln!(output, "    v{v} = {value};").unwrap();
                }
                GraphOp::StoreLocal { slot, value } => {
                    if array_slots.contains(slot) {
                        writeln!(output, "    l{slot} = v{};", value.0).unwrap();
                    }
                    writeln!(output, "    v{v} = v{};", value.0).unwrap();
                }
                GraphOp::MarkUninitialized(slot) => {
                    if array_slots.contains(slot) {
                        writeln!(output, "    l{slot} = KILN_VAL_UNINITIALIZED;").unwrap();
                    }
                    writeln!(output, "    v{v} = KILN_VAL_UNINITIALIZED;").unwrap();
                }
                // Reading a binding in its dead zone throws, which the sequential run does.
                GraphOp::CheckTdz { value, .. } => writeln!(
                    output,
                    "    if (KILN_UNLIKELY(v{x} == KILN_VAL_UNINITIALIZED)) goto kiln_hfail; v{v} = v{x};",
                    x = value.0
                )
                .unwrap(),
                GraphOp::LoadBuiltin(Builtin::Math) => writeln!(
                    output,
                    "    v{v} = KILN_VAL_BUILTIN(KILN_C99_BUILTIN_MATH);"
                )
                .unwrap(),
                GraphOp::LoadBuiltin(Builtin::NotANumber) => {
                    writeln!(output, "    v{v} = {};", number_literal(f64::NAN)).unwrap()
                }
                GraphOp::LoadBuiltin(Builtin::Infinity) => {
                    writeln!(output, "    v{v} = {};", number_literal(f64::INFINITY)).unwrap()
                }
                // Every other built-in, and a closure written here, is a placeholder: only ever
                // called, a static's receiver, or a pure array method's callback (the verdict's
                // uses check), each emitted at its use.
                GraphOp::LoadBuiltin(_) | GraphOp::MakeFunction { .. } => {
                    writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap()
                }
                GraphOp::Call { callee, args, .. } => {
                    let texts: Vec<String> = args.iter().map(|a| format!("v{}", a.0)).collect();
                    match defs.get(callee) {
                        Some(GraphOp::LoadBuiltin(Builtin::String)) if args.len() == 1 => writeln!(
                            output,
                            "    if (KILN_UNLIKELY(!kiln_c99_par_to_string(arena, v{x}, &v{v}))) goto kiln_hfail;",
                            x = args[0].0
                        )
                        .unwrap(),
                        Some(GraphOp::LoadBuiltin(b)) => {
                            let function = global_function(*b)
                                .expect("the verdict admits only pure callable globals");
                            let (decl, pointer, count) = argument_list(&texts, "a_c");
                            writeln!(
                                output,
                                "    {{ {decl}if (KILN_UNLIKELY(!kiln_c99_par_f_{function}(env, arena, {count}, {pointer}, &v{v}))) goto kiln_hfail; }}"
                            )
                            .unwrap();
                        }
                        _ => {
                            let target = *shape
                                .targets
                                .get(&(owner_code, callee.0))
                                .expect("the verdict resolves every call");
                            user_call(output, target, *callee, &texts, &format!("v{v}"));
                        }
                    }
                }
                GraphOp::NewObject { pairs, .. } => {
                    writeln!(
                        output,
                        "    if (KILN_UNLIKELY(!kiln_c99_par_object(arena, &v{v}))) goto kiln_hfail;"
                    )
                    .unwrap();
                    for (key, value) in pairs {
                        define(output, &mut site, v, key.0, value.0);
                    }
                }
                GraphOp::NewArray { elements, .. } => {
                    if elements.is_empty() {
                        writeln!(
                            output,
                            "    if (KILN_UNLIKELY(!kiln_c99_par_array(arena, NULL, 0, &v{v}))) goto kiln_hfail;"
                        )
                        .unwrap();
                    } else {
                        let list = elements
                            .iter()
                            .map(|e| format!("v{}", e.0))
                            .collect::<Vec<_>>()
                            .join(", ");
                        writeln!(
                            output,
                            "    {{ const kiln_val e[{n}] = {{ {list} }}; if (KILN_UNLIKELY(!kiln_c99_par_array(arena, e, {n}, &v{v}))) goto kiln_hfail; }}",
                            n = elements.len()
                        )
                        .unwrap();
                    }
                }
                GraphOp::Bridge {
                    instruction: kiln_ir::Instruction::SpreadObject,
                    operands,
                    ..
                } => writeln!(
                    output,
                    "    if (KILN_UNLIKELY(!kiln_c99_par_spread(arena, v{t}, v{x}))) goto kiln_hfail; v{v} = v{t};",
                    t = operands[0].0,
                    x = operands[1].0
                )
                .unwrap(),
                GraphOp::Bridge {
                    instruction: kiln_ir::Instruction::DefineNamedProperty { .. },
                    operands,
                    ..
                } => {
                    let t = operands[0].0;
                    define(output, &mut site, t, operands[1].0, operands[2].0);
                    writeln!(output, "    v{v} = v{t};").unwrap();
                }
                GraphOp::LoadUpvalue(index) => match ctx.callee {
                    None => {
                        let j = shape
                            .capture_slot(*index)
                            .expect("the verdict lists every capture");
                        writeln!(output, "    v{v} = env->caps[{j}];").unwrap();
                    }
                    // A callee's captures come from its closure, read on this thread: a binding
                    // written once, which may still be in its dead zone.
                    Some(_) => writeln!(
                        output,
                        "    v{v} = ups[{index}]; if (KILN_UNLIKELY(v{v} == KILN_VAL_UNINITIALIZED)) goto kiln_hfail;"
                    )
                    .unwrap(),
                },
                GraphOp::Unary { op, value } => {
                    let x = value.0;
                    let numeric = |expr: &str| {
                        format!(
                            "    {{ double a; if (KILN_UNLIKELY(!kiln_c99_par_number(v{x}, &a))) goto kiln_hfail; v{v} = {expr}; }}"
                        )
                    };
                    let line = match op {
                        UnaryOp::Not => format!(
                            "    v{v} = kiln_c99_val_truthy(v{x}) ? KILN_VAL_FALSE : KILN_VAL_TRUE;"
                        ),
                        UnaryOp::Void => format!("    v{v} = KILN_VAL_UNDEFINED;"),
                        UnaryOp::Plus | UnaryOp::ToNumeric => {
                            numeric("kiln_c99_val_from_double(a)")
                        }
                        UnaryOp::Negate => numeric("kiln_c99_val_from_double(-a)"),
                        UnaryOp::Increment => numeric("kiln_c99_val_from_double(a + 1.0)"),
                        UnaryOp::Decrement => numeric("kiln_c99_val_from_double(a - 1.0)"),
                        UnaryOp::BitwiseNot => {
                            numeric("kiln_c99_val_unary_fast(5, kiln_c99_val_from_double(a))")
                        }
                        UnaryOp::TypeOf => unreachable!("the verdict refuses typeof"),
                        UnaryOp::ToString => unreachable!("the verdict refuses abstract ToString"),
                    };
                    writeln!(output, "{line}").unwrap();
                }
                GraphOp::Binary { op, left, right } => {
                    let (l, r) = (left.0, right.0);
                    let numeric = |expr: &str| {
                        format!(
                            "    {{ double a, b; if (KILN_UNLIKELY(!kiln_c99_par_number(v{l}, &a) || !kiln_c99_par_number(v{r}, &b))) goto kiln_hfail; v{v} = {expr}; }}"
                        )
                    };
                    let answer = |call: &str, yes: &str| {
                        format!(
                            "    {{ int c = {call}; if (KILN_UNLIKELY(c < 0)) goto kiln_hfail; v{v} = (c == {yes}) ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}"
                        )
                    };
                    let line = match op {
                        // A string joins in the arena (tier 3), or declines without one.
                        BinaryOp::Add => format!(
                            "    if (KILN_UNLIKELY(!kiln_c99_par_add(arena, v{l}, v{r}, &v{v}))) goto kiln_hfail;"
                        ),
                        BinaryOp::Subtract => numeric("kiln_c99_val_from_double(a - b)"),
                        BinaryOp::Multiply => numeric("kiln_c99_val_from_double(a * b)"),
                        BinaryOp::Divide => numeric("kiln_c99_val_from_double(a / b)"),
                        BinaryOp::Exponentiate => numeric("kiln_c99_val_from_double(kiln_c99_js_pow(a, b))"),
                        BinaryOp::Remainder => numeric(
                            "kiln_c99_val_from_double((b > 0.0 && b <= 9007199254740992.0 && a >= 0.0 && a <= 9007199254740992.0 && a == (double)(int64_t)a && b == (double)(int64_t)b) ? (double)((int64_t)a % (int64_t)b) : fmod(a, b))",
                        ),
                        BinaryOp::BitwiseAnd
                        | BinaryOp::BitwiseOr
                        | BinaryOp::BitwiseXor
                        | BinaryOp::ShiftLeft
                        | BinaryOp::ShiftRight
                        | BinaryOp::UnsignedShiftRight => {
                            // The prelude's operation codes (`kiln_c99_val_binary_fast`).
                            let code = match op {
                                BinaryOp::BitwiseAnd => 17,
                                BinaryOp::BitwiseOr => 18,
                                BinaryOp::BitwiseXor => 19,
                                BinaryOp::ShiftLeft => 20,
                                BinaryOp::ShiftRight => 21,
                                _ => 22,
                            };
                            numeric(&format!(
                                "kiln_c99_val_binary_fast({code}, kiln_c99_val_from_double(a), kiln_c99_val_from_double(b))"
                            ))
                        }
                        BinaryOp::LessThan => {
                            answer(&format!("kiln_c99_par_relational(9, v{l}, v{r})"), "1")
                        }
                        BinaryOp::LessThanOrEqual => {
                            answer(&format!("kiln_c99_par_relational(10, v{l}, v{r})"), "1")
                        }
                        BinaryOp::GreaterThan => {
                            answer(&format!("kiln_c99_par_relational(11, v{l}, v{r})"), "1")
                        }
                        BinaryOp::GreaterThanOrEqual => {
                            answer(&format!("kiln_c99_par_relational(12, v{l}, v{r})"), "1")
                        }
                        BinaryOp::StrictEqual => {
                            answer(&format!("kiln_c99_par_strict_equal(v{l}, v{r})"), "1")
                        }
                        BinaryOp::StrictNotEqual => {
                            answer(&format!("kiln_c99_par_strict_equal(v{l}, v{r})"), "0")
                        }
                        BinaryOp::LooseEqual => {
                            answer(&format!("kiln_c99_par_loose_equal(v{l}, v{r})"), "1")
                        }
                        BinaryOp::LooseNotEqual => {
                            answer(&format!("kiln_c99_par_loose_equal(v{l}, v{r})"), "0")
                        }
                        BinaryOp::In | BinaryOp::InstanceOf => {
                            unreachable!("the verdict refuses `in` and `instanceof`")
                        }
                    };
                    writeln!(output, "{line}").unwrap();
                }
                GraphOp::GetProperty { object, name, .. } => {
                    let key = constant_text(program, *name).unwrap_or_default();
                    if math.contains(object)
                        && let Some((_, value)) = MATH_CONSTANTS.iter().find(|(c, _)| *c == key)
                    {
                        writeln!(output, "    v{v} = {};", number_literal(*value)).unwrap();
                        continue;
                    }
                    let j = shape
                        .key_slot(*name)
                        .expect("the verdict lists every name read");
                    let reader = if key == "length" {
                        "kiln_c99_par_length"
                    } else {
                        "kiln_c99_par_get"
                    };
                    writeln!(
                        output,
                        "    if (KILN_UNLIKELY(!{reader}(env, &sites[{site}], v{o}, env->keys[{j}], &v{v}))) goto kiln_hfail;",
                        o = object.0
                    )
                    .unwrap();
                    site += 1;
                }
                GraphOp::GetComputed { object, key, .. } => writeln!(
                    output,
                    "    if (KILN_UNLIKELY(!kiln_c99_par_index(env, v{o}, v{k}, &v{v}))) goto kiln_hfail;",
                    o = object.0,
                    k = key.0
                )
                .unwrap(),
                GraphOp::CallMethod {
                    object, name, args, ..
                } => {
                    let key = constant_text(program, *name).unwrap_or_default();
                    if math.contains(object) {
                        let a = args.first().map(|x| x.0);
                        let b = args.get(1).map(|x| x.0);
                        let mut load = String::new();
                        if let Some(a) = a {
                            write!(load, "!kiln_c99_par_number(v{a}, &a)").unwrap();
                        }
                        if let Some(b) = b {
                            write!(load, " || !kiln_c99_par_number(v{b}, &b)").unwrap();
                        }
                        let expr = math_expression(&key, "a", "b")
                            .expect("the verdict lists only the listed Math functions");
                        writeln!(
                            output,
                            "    {{ double a = 0.0, b = 0.0; (void)b; if (KILN_UNLIKELY({load})) goto kiln_hfail; v{v} = kiln_c99_val_from_double({expr}); }}"
                        )
                        .unwrap();
                    } else if key == "charCodeAt" && args.len() <= 1 {
                        // Tier 2's string reader.
                        let position = args
                            .first()
                            .map_or("KILN_VAL_UNDEFINED".to_owned(), |x| format!("v{}", x.0));
                        writeln!(
                            output,
                            "    if (KILN_UNLIKELY(!kiln_c99_par_char_code_at(v{o}, {position}, &v{v}))) goto kiln_hfail;",
                            o = object.0
                        )
                        .unwrap();
                    } else {
                        let texts: Vec<String> = args.iter().map(|a| format!("v{}", a.0)).collect();
                        let (decl, pointer, count) = argument_list(&texts, "a_m");
                        let path = match defs.get(object) {
                            Some(GraphOp::LoadBuiltin(Builtin::Json)) => pure_path("JSON", &key),
                            Some(GraphOp::LoadBuiltin(Builtin::Number)) => {
                                pure_path("Number", &key)
                            }
                            _ => ["String.prototype", "Number.prototype", "Array.prototype"]
                                .iter()
                                .find_map(|owner| pure_path(owner, &key)),
                        };
                        match path.expect("the verdict admits only pure members") {
                            PurePath::Function(function) => writeln!(
                                output,
                                "    {{ {decl}if (KILN_UNLIKELY(!kiln_c99_par_f_{function}(env, arena, {count}, {pointer}, &v{v}))) goto kiln_hfail; }}"
                            )
                            .unwrap(),
                            PurePath::Method(method) => writeln!(
                                output,
                                "    {{ {decl}if (KILN_UNLIKELY(!kiln_c99_par_m_{method}(env, arena, v{o}, {count}, {pointer}, &v{v}))) goto kiln_hfail; }}",
                                o = object.0
                            )
                            .unwrap(),
                            PurePath::Loop(method) => {
                                let callback = args[0];
                                let o = object.0;
                                writeln!(
                                    output,
                                    "    {{ const kiln_array *src_l; if (KILN_UNLIKELY(!kiln_c99_par_plain_array(v{o}, &src_l))) goto kiln_hfail; size_t n_l = src_l->count;"
                                )
                                .unwrap();
                                match method {
                                    "map" | "filter" => writeln!(
                                        output,
                                        "    kiln_array *out_l; if (KILN_UNLIKELY(!kiln_c99_par_array_sized(arena, n_l, &v{v}, &out_l))) goto kiln_hfail; size_t kept_l = 0; (void)kept_l;"
                                    )
                                    .unwrap(),
                                    "some" => writeln!(output, "    v{v} = KILN_VAL_FALSE;").unwrap(),
                                    _ => writeln!(output, "    v{v} = KILN_VAL_TRUE;").unwrap(),
                                }
                                writeln!(
                                    output,
                                    "    for (size_t i_l = 0; i_l < n_l; ++i_l) {{ kiln_val e_l = src_l->elements[i_l]; if (e_l == KILN_VAL_HOLE) continue; kiln_val r_l = KILN_VAL_UNDEFINED; const kiln_val cb_l[3] = {{ e_l, kiln_c99_val_from_double((double)i_l), v{o} }}; (void)cb_l;"
                                )
                                .unwrap();
                                match defs.get(&callback) {
                                    Some(GraphOp::LoadBuiltin(b)) => {
                                        let function = global_function(*b)
                                            .expect("the verdict admits only pure callable globals");
                                        writeln!(
                                            output,
                                            "    if (KILN_UNLIKELY(!kiln_c99_par_f_{function}(env, arena, 3, cb_l, &r_l))) goto kiln_hfail;"
                                        )
                                        .unwrap();
                                    }
                                    _ => {
                                        let target = *shape
                                            .targets
                                            .get(&(owner_code, callback.0))
                                            .expect("the verdict resolves every callback");
                                        let cb_args: Vec<String> =
                                            (0..3).map(|k| format!("cb_l[{k}]")).collect();
                                        user_call(output, target, callback, &cb_args, "r_l");
                                    }
                                }
                                let step = match method {
                                    "map" => "out_l->elements[i_l] = r_l;".to_owned(),
                                    "filter" => {
                                        "if (kiln_c99_val_truthy(r_l)) out_l->elements[kept_l++] = e_l;"
                                            .to_owned()
                                    }
                                    "some" => format!(
                                        "if (kiln_c99_val_truthy(r_l)) {{ v{v} = KILN_VAL_TRUE; break; }}"
                                    ),
                                    _ => format!(
                                        "if (!kiln_c99_val_truthy(r_l)) {{ v{v} = KILN_VAL_FALSE; break; }}"
                                    ),
                                };
                                writeln!(output, "    {step} }}").unwrap();
                                match method {
                                    "map" => writeln!(output, "    kiln_c99_par_array_seal(out_l); }}").unwrap(),
                                    "filter" => writeln!(
                                        output,
                                        "    out_l->count = kept_l; kiln_c99_par_array_seal(out_l); }}"
                                    )
                                    .unwrap(),
                                    _ => writeln!(output, "    }}").unwrap(),
                                }
                            }
                            PurePath::Math | PurePath::CharCodeAt => {
                                unreachable!("emitted above")
                            }
                        }
                    }
                }
                // Assigned on the edge into the block.
                GraphOp::Phi { .. } => {}
                other => unreachable!("the verdict refuses {other:?}"),
            }
        }
        match (&block.terminator, key) {
            (_, Some(k)) => writeln!(output, "    return v{};", k.0).unwrap(),
            (Terminator::Jump(target), None) => {
                phi_copies(output, graph, block.id, *target);
                writeln!(output, "    goto kiln_hb_{};", target.0).unwrap();
            }
            (
                Terminator::Branch {
                    condition,
                    true_block,
                    false_block,
                },
                None,
            ) => {
                writeln!(output, "    if (kiln_c99_val_truthy(v{})) {{", condition.0).unwrap();
                phi_copies(output, graph, block.id, *true_block);
                writeln!(output, "        goto kiln_hb_{};\n    }}", true_block.0).unwrap();
                phi_copies(output, graph, block.id, *false_block);
                writeln!(output, "    goto kiln_hb_{};", false_block.0).unwrap();
            }
            (
                Terminator::BranchNotNullish {
                    condition,
                    true_block,
                    false_block,
                },
                None,
            ) => {
                writeln!(output, "    if (!KILN_VAL_IS_NULLISH(v{})) {{", condition.0).unwrap();
                phi_copies(output, graph, block.id, *true_block);
                writeln!(output, "        goto kiln_hb_{};\n    }}", true_block.0).unwrap();
                phi_copies(output, graph, block.id, *false_block);
                writeln!(output, "    goto kiln_hb_{};", false_block.0).unwrap();
            }
            (Terminator::Return(Some(value)), None) => {
                writeln!(output, "    return v{};", value.0).unwrap()
            }
            (_, None) => output.push_str("    goto kiln_hfail;\n"),
        }
    }
    output.push_str("kiln_hfail: __attribute__((unused));\n    return KILN_VAL_TYPED_FAIL;\n}\n");
}

/// The phis of `to` assigned for the edge from `from`, as one parallel copy.
fn phi_copies(
    output: &mut String,
    graph: &FunctionGraph,
    from: crate::graph::BlockId,
    to: crate::graph::BlockId,
) {
    let Some(target) = graph.blocks.iter().find(|b| b.id == to) else {
        return;
    };
    let copies: Vec<(u32, u32)> = target
        .operations
        .iter()
        .filter_map(|(v, op)| match op {
            GraphOp::Phi { inputs } => inputs
                .iter()
                .find(|(b, _)| *b == from)
                .map(|(_, x)| (v.0, x.0)),
            _ => None,
        })
        .collect();
    if copies.is_empty() {
        return;
    }
    output.push_str("    {");
    for (k, (_, x)) in copies.iter().enumerate() {
        write!(output, " kiln_val t{k} = v{x};").unwrap();
    }
    for (k, (v, _)) in copies.iter().enumerate() {
        write!(output, " v{v} = t{k};").unwrap();
    }
    output.push_str(" }\n");
}

/// The chunk workers of a tier-2 callback, one per kind in `kinds`, named
/// `{symbol}_pmap{kind}_{index}` and calling the clones `{clone_symbol}_fn_{index}_h` (the whole
/// body) and `{clone_symbol}_fn_{index}_hk` (a key comparator's key). Every worker reads in
/// no-write mode for its whole range, keeps its own read cache, and on a decline sets the call's
/// failure flag and stops. Their contracts, which the call sites in `call_method.rs` keep:
///
/// - `_hmap`: `in` an array's elements, `out` the result's; a hole stays a hole (`map` does not
///   call the callback there), an answer is any value -- one the clone built lives in the chunk's
///   arena (tier 3, `kiln_c99_par_arena_claim`) -- and anything but a number sets `env->mixed`.
/// - `_hmap_typed`: `in` and `out` views; each answer converted as `ToNumber` converts it.
/// - `_hfilt`/`_hfilt_typed`: `out` one byte per element, the answer's `ToBoolean` (0 for a hole);
///   `_hfilt`'s chunks have an arena for what the predicate builds.
///
/// Every other worker passes the clone no arena, and an allocation there declines.
/// - `_hfold`/`_hfold_typed`: `parallelReduce`'s leaves (`in` a `struct { const void *elems;
///   size_t count; double initial; }`), holes skipped; every answer must be a number.
/// - `_hcomb`: `out[k] = combine(in[2k], in[2k + 1])`, over doubles; the answer must be a number.
/// - `_hkey`: a key comparator's key per element, `out` doubles; `undefined`, a hole, a key that
///   is not a number or is `NaN` declines (`sort` would not call the comparator on the first two,
///   and the last two make the comparator inconsistent).
/// - `_hrank`: no heap at all: `out[i]` the lower bound of `in[i]` in the sorted keys `ctx`
///   (`struct { const double *sorted; size_t count; }`).
#[allow(clippy::too_many_arguments)]
pub fn emit_heap_workers(
    output: &mut String,
    symbol: &str,
    clone_symbol: &str,
    index: usize,
    parameter_count: u32,
    kinds: &[&str],
    sites: usize,
    linkage: &str,
) {
    let clone = format!("{clone_symbol}_fn_{index}_h");
    let key_clone = format!("{clone_symbol}_fn_{index}_hk");
    let sites = sites.max(1);
    let head = |kind: &str| {
        format!(
            "{linkage}void __attribute__((unused)) {symbol}_pmap{kind}_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    kiln_c99_par_env *env = (kiln_c99_par_env *)ctx;\n\
    if (KILN_UNLIKELY(__atomic_load_n(&env->failed, __ATOMIC_RELAXED))) return;\n\
    kiln_c99_par_site sites[{sites}];\n\
    memset(sites, 0, sizeof sites);\n\
    kiln_c99_par_frame frame;\n\
    kiln_c99_par_enter(env, &frame);\n\
    kiln_c99_par_arena *arena = {claim};\n\
    int failed = 0;\n",
            claim = if matches!(kind, "_hmap" | "_hfilt") {
                "kiln_c99_par_arena_claim(env)"
            } else {
                "NULL"
            }
        )
    };
    let tail = "    kiln_c99_par_leave(&frame);\n\
    if (KILN_UNLIKELY(failed)) __atomic_store_n(&env->failed, 1, __ATOMIC_RELAXED);\n}\n";
    let index_arg = if parameter_count == 2 {
        ", kiln_c99_val_from_double((double)i)"
    } else {
        ""
    };
    for kind in kinds {
        match *kind {
            "_hmap" => write!(
                output,
                "{}\
    const kiln_val * restrict in_arr = (const kiln_val * restrict)in;\n\
    kiln_val * restrict out_arr = (kiln_val * restrict)out;\n\
    int mixed = 0;\n\
    for (size_t i = start; i < end; ++i) {{\n\
        kiln_val e = in_arr[i];\n\
        if (e == KILN_VAL_HOLE) {{ out_arr[i] = KILN_VAL_HOLE; mixed = 1; continue; }}\n\
        kiln_val r = {clone}(env, sites, arena, e{index_arg});\n\
        if (KILN_UNLIKELY(r == KILN_VAL_TYPED_FAIL)) {{ failed = 1; break; }}\n\
        mixed |= !KILN_VAL_IS_NUMBER(r);\n\
        out_arr[i] = r;\n\
    }}\n\
    if (mixed) __atomic_store_n(&env->mixed, 1, __ATOMIC_RELAXED);\n{tail}",
                head(kind)
            )
            .unwrap(),
            "_hmap_typed" => write!(
                output,
                "{}\
    const kiln_typed_array *in_view = (const kiln_typed_array *)in;\n\
    kiln_typed_array *out_view = (kiln_typed_array *)out;\n\
    for (size_t i = start; i < end; ++i) {{\n\
        kiln_val r = {clone}(env, sites, arena, kiln_c99_val_from_double(kiln_typed_load(in_view, i)){index_arg});\n\
        double d;\n\
        if (KILN_UNLIKELY(r == KILN_VAL_TYPED_FAIL || !kiln_c99_par_number(r, &d))) {{ failed = 1; break; }}\n\
        kiln_c99_typed_set(out_view, i, d);\n\
    }}\n{tail}",
                head(kind)
            )
            .unwrap(),
            "_hfilt" => write!(
                output,
                "{}\
    const kiln_val * restrict in_arr = (const kiln_val * restrict)in;\n\
    uint8_t * restrict keep = (uint8_t * restrict)out;\n\
    for (size_t i = start; i < end; ++i) {{\n\
        kiln_val e = in_arr[i];\n\
        if (e == KILN_VAL_HOLE) {{ keep[i] = 0; continue; }}\n\
        kiln_val r = {clone}(env, sites, arena, e{index_arg});\n\
        if (KILN_UNLIKELY(r == KILN_VAL_TYPED_FAIL)) {{ failed = 1; break; }}\n\
        keep[i] = (uint8_t)kiln_c99_val_truthy(r);\n\
    }}\n{tail}",
                head(kind)
            )
            .unwrap(),
            "_hfilt_typed" => write!(
                output,
                "{}\
    const kiln_typed_array *in_view = (const kiln_typed_array *)in;\n\
    uint8_t * restrict keep = (uint8_t * restrict)out;\n\
    for (size_t i = start; i < end; ++i) {{\n\
        kiln_val r = {clone}(env, sites, arena, kiln_c99_val_from_double(kiln_typed_load(in_view, i)){index_arg});\n\
        if (KILN_UNLIKELY(r == KILN_VAL_TYPED_FAIL)) {{ failed = 1; break; }}\n\
        keep[i] = (uint8_t)kiln_c99_val_truthy(r);\n\
    }}\n{tail}",
                head(kind)
            )
            .unwrap(),
            "_hfold" | "_hfold_typed" => {
                let leaf = crate::parallel::REDUCE_LEAF;
                let element = if *kind == "_hfold" {
                    "((const kiln_val *)src->elems)[i]"
                } else {
                    "kiln_c99_val_from_double(kiln_typed_load((const kiln_typed_array *)src->elems, i))"
                };
                let fold_index = if parameter_count == 3 {
                    ", kiln_c99_val_from_double((double)i)"
                } else {
                    ""
                };
                write!(
                    output,
                    "{}\
    const struct {{ const void *elems; size_t count; double initial; }} *src = in;\n\
    double * restrict leaves = (double * restrict)out;\n\
    for (size_t l = start; l < end && !failed; ++l) {{\n\
        size_t first = l * {leaf};\n\
        size_t last = first + {leaf} < src->count ? first + {leaf} : src->count;\n\
        double acc = src->initial;\n\
        for (size_t i = first; i < last; ++i) {{\n\
            kiln_val e = {element};\n\
            if (e == KILN_VAL_HOLE) continue;\n\
            kiln_val r = {clone}(env, sites, arena, kiln_c99_val_from_double(acc), e{fold_index});\n\
            if (KILN_UNLIKELY(r == KILN_VAL_TYPED_FAIL || !KILN_VAL_IS_NUMBER(r))) {{ failed = 1; break; }}\n\
            memcpy(&acc, &r, sizeof acc);\n\
        }}\n\
        leaves[l] = acc;\n\
    }}\n{tail}",
                    head(kind)
                )
                .unwrap();
            }
            "_hcomb" => write!(
                output,
                "{}\
    const double * restrict level = (const double * restrict)in;\n\
    double * restrict next = (double * restrict)out;\n\
    for (size_t k = start; k < end; ++k) {{\n\
        kiln_val r = {clone}(env, sites, arena, kiln_c99_val_from_double(level[2 * k]), kiln_c99_val_from_double(level[2 * k + 1]));\n\
        if (KILN_UNLIKELY(r == KILN_VAL_TYPED_FAIL || !KILN_VAL_IS_NUMBER(r))) {{ failed = 1; break; }}\n\
        memcpy(&next[k], &r, sizeof next[k]);\n\
    }}\n{tail}",
                head(kind)
            )
            .unwrap(),
            "_hkey" => write!(
                output,
                "{}\
    const kiln_val * restrict in_arr = (const kiln_val * restrict)in;\n\
    double * restrict keys = (double * restrict)out;\n\
    for (size_t i = start; i < end; ++i) {{\n\
        kiln_val e = in_arr[i];\n\
        if (KILN_UNLIKELY(e == KILN_VAL_HOLE || e == KILN_VAL_UNDEFINED)) {{ failed = 1; break; }}\n\
        kiln_val r = {key_clone}(env, sites, arena, e);\n\
        if (KILN_UNLIKELY(r == KILN_VAL_TYPED_FAIL || !KILN_VAL_IS_NUMBER(r))) {{ failed = 1; break; }}\n\
        memcpy(&keys[i], &r, sizeof keys[i]);\n\
        if (KILN_UNLIKELY(isnan(keys[i]))) {{ failed = 1; break; }}\n\
    }}\n{tail}",
                head(kind)
            )
            .unwrap(),
            "_hrank" => write!(
                output,
                "{linkage}void __attribute__((unused)) {symbol}_pmap_hrank_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    const double * restrict keys = (const double * restrict)in;\n\
    uint32_t * restrict ranks = (uint32_t * restrict)out;\n\
    const struct {{ const double *sorted; size_t count; }} *order = ctx;\n\
    for (size_t i = start; i < end; ++i) {{\n\
        size_t lo = 0, hi = order->count;\n\
        while (lo < hi) {{ size_t mid = lo + (hi - lo) / 2; if (order->sorted[mid] < keys[i]) lo = mid + 1; else hi = mid; }}\n\
        ranks[i] = (uint32_t)lo;\n\
    }}\n}}\n"
            )
            .unwrap(),
            other => unreachable!("not a tier-2 worker kind: {other}"),
        }
    }
}
