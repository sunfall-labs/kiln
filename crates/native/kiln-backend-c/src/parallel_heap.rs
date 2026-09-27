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
            "({{ double r = isfinite({a}) ? floor({a} + 0.5) : {a}; if (r == 0.0 && signbit({a})) r = -0.0; r; }})"
        ),
        "sign" => format!("(isnan({a}) || {a} == 0.0 ? {a} : ({a} < 0.0 ? -1.0 : 1.0))"),
        "pow" => format!("pow({a}, {b})"),
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
pub fn emit_heap_clone(
    output: &mut String,
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    name: &str,
    shape: &HeapShape,
    key: Option<ValueId>,
) {
    let only = key.map(|k| cone(graph, k));
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
    let mut params = String::from(
        "const kiln_c99_par_env * restrict env, kiln_c99_par_site * restrict sites, kiln_c99_par_arena * restrict arena",
    );
    for k in 0..parameters {
        write!(params, ", kiln_val p{k}").unwrap();
    }
    writeln!(
        output,
        "static __attribute__((unused)) kiln_val {name}({params}) {{"
    )
    .unwrap();
    output.push_str("    (void)env; (void)sites; (void)arena;\n");
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
    let mut site = 0usize;
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
                // `String` is only ever the callee of `String(x)`, emitted below.
                GraphOp::LoadBuiltin(Builtin::String) => {
                    writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap()
                }
                GraphOp::LoadBuiltin(_) => writeln!(
                    output,
                    "    v{v} = KILN_VAL_BUILTIN(KILN_C99_BUILTIN_MATH);"
                )
                .unwrap(),
                GraphOp::Call { args, .. } => writeln!(
                    output,
                    "    if (KILN_UNLIKELY(!kiln_c99_par_to_string(arena, v{x}, &v{v}))) goto kiln_hfail;",
                    x = args[0].0
                )
                .unwrap(),
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
                    instruction: kiln_ir::Instruction::DefineNamedProperty,
                    operands,
                    ..
                } => {
                    let t = operands[0].0;
                    define(output, &mut site, t, operands[1].0, operands[2].0);
                    writeln!(output, "    v{v} = v{t};").unwrap();
                }
                GraphOp::LoadUpvalue(index) => {
                    let j = shape
                        .capture_slot(*index)
                        .expect("the verdict lists every capture");
                    writeln!(output, "    v{v} = env->caps[{j}];").unwrap();
                }
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
                        BinaryOp::Exponentiate => numeric("kiln_c99_val_from_double(pow(a, b))"),
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
                    } else {
                        // `charCodeAt`, the one string method the verdict admits.
                        let position = args
                            .first()
                            .map_or("KILN_VAL_UNDEFINED".to_owned(), |x| format!("v{}", x.0));
                        writeln!(
                            output,
                            "    if (KILN_UNLIKELY(!kiln_c99_par_char_code_at(v{o}, {position}, &v{v}))) goto kiln_hfail;",
                            o = object.0
                        )
                        .unwrap();
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
