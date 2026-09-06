use super::clones::typed_fail;
use super::method_candidates;
use super::{EntryKind, Speculation};
use crate::direct::{CallPlan, direct_callable};
use crate::graph::{
    FunctionGraph, GraphOp, InductionVariable, ValueId, as_safe_integer, resolve_const_string,
};
use crate::prelude::lookup_speculation_invariant;
use crate::tail::{TailContext, emit_tail_instruction};
use crate::{BuiltinLoad, builtin_load, closure_construction};
use kiln_ir::Upvalue;
use kiln_ir::{BinaryOp, Constant, FunctionProto, Program, UnaryOp};
use std::collections::HashSet;
use std::fmt::Write;

/// One status check after a runtime call. A throw unwinds; a refusal is attributed to this site
/// and reported through `kiln_refusal`, which carries the diagnostic in the result; anything
/// else returns as it is.
fn status_check(diagnostic_site: &str, pc: u32, operation: &str, unwind: &str) -> String {
    format!(
        "if (status == KILN_C99_THROWN) {unwind} if (status == KILN_C99_CONTROL_ERROR) {{ kiln_c99_note_refusal(\"{diagnostic_site}\", (size_t){pc}, \"{operation}\", (size_t)kiln_c99_diagnostic_active_depth); goto kiln_refusal; }} if (status != KILN_C99_OK) return status;"
    )
}

/// The numeric fast path of a binary operator on two unboxed doubles `a` and `b`, as the C
/// expression producing a `kiln_val`, or `None` for an operator with no numeric form.
/// The arithmetic of a binary operation on two doubles `a` and `b`, as a double.
fn binary_raw_body(op: BinaryOp) -> Option<&'static str> {
    Some(match op {
        BinaryOp::Add => "a + b",
        BinaryOp::Subtract => "a - b",
        BinaryOp::Multiply => "a * b",
        BinaryOp::Divide => "a / b",
        BinaryOp::Exponentiate => "pow(a, b)",
        _ => return None,
    })
}

fn binary_fast_body(op: BinaryOp) -> Option<&'static str> {
    Some(match op {
        BinaryOp::Add => "kiln_c99_val_from_double(a + b)",
        BinaryOp::Subtract => "kiln_c99_val_from_double(a - b)",
        BinaryOp::Multiply => "kiln_c99_val_from_double(a * b)",
        BinaryOp::Divide => "kiln_c99_val_from_double(a / b)",
        BinaryOp::Exponentiate => "kiln_c99_val_from_double(pow(a, b))",
        // `%` on two integers in the exact range is an integer remainder; anything else is fmod,
        // which is what the language specifies for every double.
        BinaryOp::Remainder => {
            "((b > 0.0 && b <= 9007199254740992.0 && a >= 0.0 && a <= 9007199254740992.0 && a == (double)(int64_t)a && b == (double)(int64_t)b) ? kiln_c99_val_from_double((double)((int64_t)a % (int64_t)b)) : kiln_c99_val_from_double(fmod(a, b)))"
        }
        BinaryOp::StrictEqual | BinaryOp::LooseEqual => {
            "((a == b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)"
        }
        BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => {
            "((a != b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)"
        }
        BinaryOp::LessThan => "((a < b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)",
        BinaryOp::LessThanOrEqual => "((a <= b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)",
        BinaryOp::GreaterThan => "((a > b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)",
        BinaryOp::GreaterThanOrEqual => "((a >= b) ? KILN_VAL_TRUE : KILN_VAL_FALSE)",
        // The 32-bit conversions live inside the prelude and are not exported to a module
        // unit; the pure helper does them exactly and never calls back into the program when
        // both operands are numbers, which the guard around this body has just established.
        BinaryOp::BitwiseAnd => "kiln_c99_val_binary_fast(17, LEFT, RIGHT)",
        BinaryOp::BitwiseOr => "kiln_c99_val_binary_fast(18, LEFT, RIGHT)",
        BinaryOp::BitwiseXor => "kiln_c99_val_binary_fast(19, LEFT, RIGHT)",
        BinaryOp::ShiftLeft => "kiln_c99_val_binary_fast(20, LEFT, RIGHT)",
        BinaryOp::ShiftRight => "kiln_c99_val_binary_fast(21, LEFT, RIGHT)",
        BinaryOp::UnsignedShiftRight => "kiln_c99_val_binary_fast(22, LEFT, RIGHT)",
        BinaryOp::In | BinaryOp::InstanceOf => return None,
    })
}

fn binary_code(op: BinaryOp) -> u16 {
    match op {
        BinaryOp::Add => 1,
        BinaryOp::Subtract => 2,
        BinaryOp::Multiply => 3,
        BinaryOp::Divide => 4,
        BinaryOp::Remainder => 5,
        BinaryOp::Exponentiate => 6,
        BinaryOp::StrictEqual => 7,
        BinaryOp::StrictNotEqual => 8,
        BinaryOp::LessThan => 9,
        BinaryOp::LessThanOrEqual => 10,
        BinaryOp::GreaterThan => 11,
        BinaryOp::GreaterThanOrEqual => 12,
        BinaryOp::In => 13,
        BinaryOp::InstanceOf => 14,
        BinaryOp::LooseEqual => 15,
        BinaryOp::LooseNotEqual => 16,
        BinaryOp::BitwiseAnd => 17,
        BinaryOp::BitwiseOr => 18,
        BinaryOp::BitwiseXor => 19,
        BinaryOp::ShiftLeft => 20,
        BinaryOp::ShiftRight => 21,
        BinaryOp::UnsignedShiftRight => 22,
    }
}

/// The numeric fast path of a unary operator on an unboxed double `a`, or `None` when the
/// operator never takes it.
fn unary_fast_body(op: UnaryOp) -> Option<&'static str> {
    Some(match op {
        UnaryOp::Plus | UnaryOp::ToNumeric => "value",
        UnaryOp::Negate => "kiln_c99_val_from_double(-a)",
        UnaryOp::BitwiseNot => "kiln_c99_val_unary_fast(5, value)",
        UnaryOp::Increment => "kiln_c99_val_from_double(a + 1.0)",
        UnaryOp::Decrement => "kiln_c99_val_from_double(a - 1.0)",
        UnaryOp::Not | UnaryOp::TypeOf | UnaryOp::Void => return None,
    })
}

fn unary_code(op: UnaryOp) -> u16 {
    match op {
        UnaryOp::Plus => 1,
        UnaryOp::Negate => 2,
        UnaryOp::Not => 3,
        UnaryOp::TypeOf => 4,
        UnaryOp::BitwiseNot => 5,
        UnaryOp::Void => 6,
        UnaryOp::ToNumeric => 7,
        UnaryOp::Increment => 8,
        UnaryOp::Decrement => 9,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_graph_op(
    output: &mut String,
    vid: ValueId,
    op: &GraphOp,
    graph: &FunctionGraph,
    program: &Program,
    symbol: &str,
    diagnostic_site: &str,
    scalar_proto: bool,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    ivs: &std::collections::HashMap<ValueId, InductionVariable>,
    array_slots: &HashSet<u32>,
    proto: &FunctionProto,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    origins: &std::collections::HashMap<ValueId, crate::graph::Origin>,
    unwind: &str,
    plan: &CallPlan,
    kind: EntryKind,
    self_index: Option<u32>,
    this_site: Option<usize>,
    spec: &Speculation,
) {
    let v = vid.0;
    let typed = numbers.contains(&vid);
    let typed_int = ints.contains(&vid);
    let in_array = |slot: &u32| array_slots.contains(slot);
    let (pc, label) = origins
        .get(&vid)
        .map(|o| (o.pc, o.label.as_str()))
        .unwrap_or((0, "Op"));
    match op {
        GraphOp::Const { index, constant } => match constant {
            Constant::Number(bits) => {
                writeln!(output, "    v{v} = UINT64_C({bits});").unwrap();
                if typed {
                    writeln!(output, "    memcpy(&d{v}, &v{v}, sizeof(d{v}));").unwrap();
                }
                if typed_int && let Some(ival) = as_safe_integer(constant) {
                    writeln!(output, "    i{v} = {ival}LL;").unwrap();
                }
            }
            Constant::Undefined => {
                writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
            }
            Constant::Null => {
                writeln!(output, "    v{v} = KILN_VAL_NULL;").unwrap();
            }
            Constant::Boolean(b) => {
                let val_str = if *b {
                    "KILN_VAL_TRUE"
                } else {
                    "KILN_VAL_FALSE"
                };
                writeln!(output, "    v{v} = {val_str};").unwrap();
            }
            // A NUL-free literal is a tagged pointer to
            // its static bytes; one with a NUL inside goes through the per-unit string cache; a
            // bigint is parsed once into a cached heap value.
            Constant::String(_) => {
                writeln!(
                    output,
                    "    v{v} = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &{symbol}_string_{index});"
                )
                .unwrap();
            }
            Constant::BigInt(digits) => {
                writeln!(
                    output,
                    "    {{ kiln_value tmp; int status = kiln_c99_bigint_constant(&{symbol}_bigint_{index}, {symbol}_constant_{index}, {}, &tmp); if (status != KILN_C99_OK) return status; v{v} = kiln_val_from_boundary(tmp); }}",
                    digits.len()
                )
                .unwrap();
            }
        },
        GraphOp::LoadLocal(slot) => {
            if in_array(slot) {
                writeln!(output, "    v{v} = locals[{slot}];").unwrap();
            } else if *slot < proto.parameter_count && matches!(kind, EntryKind::Typed { .. }) {
                // The typed entry's contract: exactly the parameter count of arguments, each a
                // number, arriving as the double itself. The box is made here and deleted by
                // the C compiler wherever nothing reads the value untyped.
                writeln!(
                    output,
                    "    d{v} = p{slot}; v{v} = kiln_c99_val_from_double(p{slot});"
                )
                .unwrap();
            } else if *slot < proto.parameter_count {
                writeln!(
                    output,
                    "    v{v} = (size_t){slot} < argc ? args[{slot}] : KILN_VAL_UNDEFINED;"
                )
                .unwrap();
            } else {
                writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
            }
            if typed && !(*slot < proto.parameter_count && matches!(kind, EntryKind::Typed { .. }))
            {
                writeln!(output, "    memcpy(&d{v}, &v{v}, sizeof(d{v}));").unwrap();
            }
            if typed_int {
                writeln!(output, "    i{v} = (int64_t)d{v};").unwrap();
            }
        }
        GraphOp::StoreLocal { slot, value } => {
            if in_array(slot) {
                writeln!(output, "    locals[{slot}] = v{};", value.0).unwrap();
            }
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
            if typed {
                writeln!(output, "    d{v} = d{};", value.0).unwrap();
            }
            if typed_int {
                writeln!(output, "    i{v} = i{};", value.0).unwrap();
            }
        }
        // A cell is a shared box: the load checks the temporal dead zone and reads through it,
        // the store writes through it.
        GraphOp::LoadCell(slot) if plan.unchecked.contains(&vid) => {
            // A binding the plan proved is never in its dead zone: the closure, or `undefined`
            // before it, and nothing can read it in between.
            writeln!(
                output,
                "    v{v} = *(const kiln_val *)KILN_VAL_PTR(locals[{slot}]);"
            )
            .unwrap();
        }
        GraphOp::LoadUpvalue(index) if plan.unchecked.contains(&vid) => {
            writeln!(
                output,
                "    v{v} = *(const kiln_val *)KILN_VAL_PTR(upvalues[{index}]);"
            )
            .unwrap();
        }
        GraphOp::LoadCell(slot) => {
            // A body that cannot hold a reference has no `thrown` slot; a dead-zone read there
            // is refused by the helper, loudly, rather than crashing or answering `undefined`.
            let thrown = if scalar_proto { "NULL" } else { "thrown" };
            let check = status_check(diagnostic_site, pc, label, unwind);
            writeln!(
                output,
                "    {{ const kiln_val *cell = (const kiln_val *)KILN_VAL_PTR(locals[{slot}]); int status = kiln_c99_val_check_tdz(*cell, {thrown}); {check} v{v} = *cell; }}"
            )
            .unwrap();
        }
        GraphOp::StoreCell { slot, value } => {
            writeln!(
                output,
                "    *(kiln_val *)KILN_VAL_PTR(locals[{slot}]) = v{};",
                value.0
            )
            .unwrap();
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
            if typed {
                writeln!(output, "    d{v} = d{};", value.0).unwrap();
            }
            if typed_int {
                writeln!(output, "    i{v} = i{};", value.0).unwrap();
            }
        }
        // An upvalue is a box the closure was handed; the load checks the temporal dead zone
        // the way a cell load does, and the store writes the box.
        GraphOp::LoadUpvalue(index) => {
            let check = status_check(diagnostic_site, pc, label, unwind);
            writeln!(
                output,
                "    {{ const kiln_val *cell = (const kiln_val *)KILN_VAL_PTR(upvalues[{index}]); int status = kiln_c99_val_check_tdz(*cell, thrown); {check} v{v} = *cell; }}"
            )
            .unwrap();
        }
        GraphOp::StoreUpvalue { index, value } => {
            writeln!(
                output,
                "    *(kiln_val *)KILN_VAL_PTR(upvalues[{index}]) = v{};",
                value.0
            )
            .unwrap();
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
        }
        GraphOp::LoadBuiltin(builtin) => match builtin_load(*builtin) {
            // A built-in object or method is a constant in the box, not a boundary value to
            // build and convert on every mention: `Math` inside a hot method was doing both.
            BuiltinLoad::Value(expr) if expr.starts_with("kiln_c99_builtin(") => {
                let id = &expr["kiln_c99_builtin(".len()..expr.len() - 1];
                writeln!(output, "    v{v} = KILN_VAL_BUILTIN({id});").unwrap();
            }
            BuiltinLoad::Value(expr) if expr.starts_with("kiln_c99_method(") => {
                let id = &expr["kiln_c99_method(".len()..expr.len() - 1];
                writeln!(output, "    v{v} = KILN_VAL_METHOD({id});").unwrap();
            }
            BuiltinLoad::Value(expr) => {
                writeln!(output, "    v{v} = kiln_val_from_boundary({expr});").unwrap();
            }
            BuiltinLoad::Build(function) => {
                writeln!(
                    output,
                    "    {{ kiln_value obj; int status = {function}(&obj); if (status != KILN_C99_OK) return status; v{v} = kiln_val_from_boundary(obj); }}"
                )
                .unwrap();
            }
        },
        GraphOp::CaughtValue => {
            // Landing in a handler: the throw is handled, and the value is the block's first.
            writeln!(
                output,
                "    kiln_c99_forget_handled_throw(); v{v} = kiln_val_from_boundary(*thrown);"
            )
            .unwrap();
        }
        GraphOp::Bridge {
            instruction,
            operands,
            pushes,
        } => {
            // The operands go onto a local stack of exactly the depth the instruction wants,
            // the tail's lowering (`tail.rs`) runs on it, and the results come back off the top.
            let depth = operands.len() + usize::from(*pushes) + 2;
            let mut push = String::new();
            for operand in operands {
                push.push_str(&format!(" stack[sp++] = v{};", operand.0));
            }
            writeln!(
                output,
                "    {{ kiln_val stack[{depth}]; size_t sp = 0;{push}"
            )
            .unwrap();
            emit_tail_instruction(
                output,
                instruction,
                pc,
                &TailContext {
                    constants: &program.constants,
                    symbol,
                    diagnostic_site,
                    callables: !scalar_proto,
                },
                unwind,
            );
            if *pushes > 0 {
                writeln!(output, "    v{v} = stack[sp - 1]; }}").unwrap();
            } else {
                writeln!(output, "    v{v} = KILN_VAL_UNDEFINED; }}").unwrap();
            }
        }
        GraphOp::MakeCell(slot) => {
            writeln!(
                output,
                "    {{ int status = kiln_c99_val_make_cell(&locals[{slot}]); if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
            writeln!(output, "    v{v} = locals[{slot}];").unwrap();
        }
        GraphOp::LoadGlobal(slot) => {
            writeln!(output, "    v{v} = {symbol}_globals[{slot}];").unwrap();
        }
        GraphOp::StoreGlobal { slot, value } => {
            writeln!(output, "    {symbol}_globals[{slot}] = v{};", value.0).unwrap();
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
            if typed {
                writeln!(output, "    d{v} = d{};", value.0).unwrap();
            }
            if typed_int {
                writeln!(output, "    i{v} = i{};", value.0).unwrap();
            }
        }
        GraphOp::LoadThis => {
            writeln!(output, "    v{v} = receiver;").unwrap();
        }
        GraphOp::LoadNewTarget => {
            writeln!(
                output,
                "    v{v} = kiln_val_from_boundary(kiln_c99_new_target);"
            )
            .unwrap();
        }
        GraphOp::MarkUninitialized(slot) if !in_array(slot) => {
            // No array behind this slot: the mark is the SSA value alone.
            writeln!(output, "    v{v} = KILN_VAL_UNINITIALIZED;").unwrap();
        }
        GraphOp::MarkUninitialized(slot) => {
            // A slot that holds a cell is marked through the cell: the binding is captured, and
            // its closures read the box, not the slot. Overwriting the slot would drop the cell
            // pointer and the next write through it would dereference the sentinel.
            writeln!(
                output,
                "    if ((locals[{slot}] & KILN_TAG_MASK) == KILN_TAG_CELL_PREFIX) {{ *(kiln_val *)KILN_VAL_PTR(locals[{slot}]) = KILN_VAL_UNINITIALIZED; }} else {{ locals[{slot}] = KILN_VAL_UNINITIALIZED; }}"
            )
            .unwrap();
            writeln!(output, "    v{v} = KILN_VAL_UNINITIALIZED;").unwrap();
        }
        GraphOp::CheckTdz { slot: _, value } => {
            // A proven number is initialized by definition: no check, just the copies.
            if graph.scalar_alias_map.contains_key(value) || numbers.contains(value) {
                writeln!(output, "    v{v} = v{};", value.0).unwrap();
                if typed {
                    writeln!(output, "    d{v} = d{};", value.0).unwrap();
                }
                if typed_int {
                    writeln!(output, "    i{v} = i{};", value.0).unwrap();
                }
                return;
            }
            let thrown = if scalar_proto { "NULL" } else { "thrown" };
            writeln!(
                output,
                "    {{ int status = kiln_c99_val_check_tdz(v{}, {thrown}); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}",
                value.0
            )
            .unwrap();
            writeln!(output, "    v{v} = v{};", value.0).unwrap();
            if typed {
                writeln!(output, "    d{v} = d{};", value.0).unwrap();
            }
            if typed_int {
                writeln!(output, "    i{v} = i{};", value.0).unwrap();
            }
        }
        GraphOp::Unary { op, value } if typed_int => {
            let x = value.0;
            match op {
                UnaryOp::Plus | UnaryOp::ToNumeric => {
                    if ints.contains(value) {
                        writeln!(output, "    i{v} = i{x}; d{v} = d{x}; v{v} = v{x};").unwrap();
                    } else {
                        writeln!(
                            output,
                            "    d{v} = d{x}; i{v} = (int64_t)d{v}; v{v} = v{x};"
                        )
                        .unwrap();
                    }
                }
                UnaryOp::Increment => {
                    if ivs.contains_key(&vid) {
                        writeln!(
                            output,
                            "    i{v} = i{x} + 1LL; d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});"
                        )
                        .unwrap();
                    } else if ints.contains(value) {
                        writeln!(
                            output,
                            "    {{ int64_t res; if (KILN_LIKELY(!__builtin_add_overflow(i{x}, 1LL, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res; d{v} = (double)res; }} else {{ d{v} = kiln_c99_int_slow((double)i{x} + 1.0); i{v} = (int64_t)d{v}; }} v{v} = kiln_c99_val_from_double(d{v}); }}"
                        )
                        .unwrap();
                    } else {
                        writeln!(
                            output,
                            "    d{v} = d{x} + 1.0; i{v} = (int64_t)d{v}; v{v} = kiln_c99_val_from_double(d{v});"
                        )
                        .unwrap();
                    }
                }
                UnaryOp::Decrement => {
                    if ivs.contains_key(&vid) {
                        writeln!(
                            output,
                            "    i{v} = i{x} - 1LL; d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});"
                        )
                        .unwrap();
                    } else if ints.contains(value) {
                        writeln!(
                            output,
                            "    {{ int64_t res; if (KILN_LIKELY(!__builtin_sub_overflow(i{x}, 1LL, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res; d{v} = (double)res; }} else {{ d{v} = kiln_c99_int_slow((double)i{x} - 1.0); i{v} = (int64_t)d{v}; }} v{v} = kiln_c99_val_from_double(d{v}); }}"
                        )
                        .unwrap();
                    } else {
                        writeln!(
                            output,
                            "    d{v} = d{x} - 1.0; i{v} = (int64_t)d{v}; v{v} = kiln_c99_val_from_double(d{v});"
                        )
                        .unwrap();
                    }
                }
                UnaryOp::BitwiseNot => {
                    if ints.contains(value) {
                        writeln!(
                            output,
                            "    i{v} = (int64_t)(~(int32_t)i{x}); d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});"
                        )
                        .unwrap();
                    } else {
                        writeln!(
                            output,
                            "    v{v} = kiln_c99_val_unary_fast(5, v{x}); memcpy(&d{v}, &v{v}, sizeof(d{v})); i{v} = (int64_t)d{v};"
                        )
                        .unwrap();
                    }
                }
                _ => {
                    let fast = unary_fast_body(*op)
                        .expect("a proven-int unary has a numeric body")
                        .replace("value", &format!("v{x}"))
                        .replace("kiln_c99_val_from_double(", "(");
                    let expr = fast.replace('a', &format!("d{x}"));
                    writeln!(
                        output,
                        "    d{v} = {expr}; i{v} = (int64_t)d{v}; v{v} = kiln_c99_val_from_double(d{v});"
                    )
                    .unwrap();
                }
            }
        }
        GraphOp::Unary { op, value } if typed => {
            // Proven number in, proven number out: the double is computed directly.
            let x = value.0;
            let fast = unary_fast_body(*op)
                .expect("a proven-number unary has a numeric body")
                .replace("value", &format!("v{x}"))
                .replace("kiln_c99_val_from_double(", "(");
            match op {
                UnaryOp::Plus | UnaryOp::ToNumeric => {
                    writeln!(output, "    d{v} = d{x}; v{v} = v{x};").unwrap();
                }
                UnaryOp::BitwiseNot => {
                    writeln!(output, "    v{v} = kiln_c99_val_unary_fast(5, v{x}); memcpy(&d{v}, &v{v}, sizeof(d{v}));").unwrap();
                }
                _ => {
                    let expr = fast.replace('a', &format!("d{x}"));
                    writeln!(
                        output,
                        "    d{v} = {expr}; v{v} = kiln_c99_val_from_double(d{v});"
                    )
                    .unwrap();
                }
            }
        }
        GraphOp::Unary { op, value } => {
            let x = value.0;
            let code = unary_code(*op);
            match op {
                UnaryOp::Void => writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap(),
                // `!` and `typeof` never call back into the program and never throw.
                UnaryOp::Not => writeln!(
                    output,
                    "    v{v} = kiln_c99_val_truthy(v{x}) ? KILN_VAL_FALSE : KILN_VAL_TRUE;"
                )
                .unwrap(),
                UnaryOp::TypeOf => {
                    writeln!(output, "    v{v} = kiln_c99_val_unary_fast({code}, v{x});").unwrap()
                }
                _ => {
                    let fast = unary_fast_body(*op)
                        .unwrap()
                        .replace("value", &format!("v{x}"));
                    // The slow path converts on the machine: an object operand's `valueOf` runs
                    // and may throw. A body that cannot produce a reference never gets there,
                    // and takes the pure helper.
                    let slow = if scalar_proto {
                        format!("v{v} = kiln_c99_val_unary_fast({code}, v{x});")
                    } else {
                        let check = status_check(diagnostic_site, pc, label, unwind);
                        format!(
                            "kiln_value ans; int status = kiln_c99_unary_checked({code}, host, kiln_val_to_boundary(v{x}), &ans, depth, thrown); {check} v{v} = kiln_val_from_boundary(ans);"
                        )
                    };
                    writeln!(
                        output,
                        "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{x}))) {{ double a; memcpy(&a, &v{x}, sizeof(a)); (void)a; v{v} = {fast}; }} else {{ {slow} }}"
                    )
                    .unwrap();
                }
            }
        }
        GraphOp::Binary { op, left, right }
            if ints.contains(left)
                && ints.contains(right)
                && !matches!(op, BinaryOp::In | BinaryOp::InstanceOf) =>
        {
            let (l, r) = (left.0, right.0);
            match op {
                BinaryOp::StrictEqual | BinaryOp::LooseEqual => {
                    writeln!(
                        output,
                        "    v{v} = (i{l} == i{r}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                    )
                    .unwrap();
                }
                BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => {
                    writeln!(
                        output,
                        "    v{v} = (i{l} != i{r}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                    )
                    .unwrap();
                }
                BinaryOp::LessThan => {
                    writeln!(
                        output,
                        "    v{v} = (i{l} < i{r}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                    )
                    .unwrap();
                }
                BinaryOp::LessThanOrEqual => {
                    writeln!(
                        output,
                        "    v{v} = (i{l} <= i{r}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                    )
                    .unwrap();
                }
                BinaryOp::GreaterThan => {
                    writeln!(
                        output,
                        "    v{v} = (i{l} > i{r}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                    )
                    .unwrap();
                }
                BinaryOp::GreaterThanOrEqual => {
                    writeln!(
                        output,
                        "    v{v} = (i{l} >= i{r}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                    )
                    .unwrap();
                }
                BinaryOp::BitwiseAnd => {
                    writeln!(output, "    i{v} = (int64_t)((int32_t)i{l} & (int32_t)i{r}); d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                }
                BinaryOp::BitwiseOr => {
                    writeln!(output, "    i{v} = (int64_t)((int32_t)i{l} | (int32_t)i{r}); d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                }
                BinaryOp::BitwiseXor => {
                    writeln!(output, "    i{v} = (int64_t)((int32_t)i{l} ^ (int32_t)i{r}); d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                }
                BinaryOp::ShiftLeft => {
                    writeln!(output, "    i{v} = (int64_t)(int32_t)((uint32_t)i{l} << ((uint32_t)i{r} & 31)); d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                }
                BinaryOp::ShiftRight => {
                    writeln!(output, "    i{v} = (int64_t)((int32_t)i{l} >> ((uint32_t)i{r} & 31)); d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                }
                BinaryOp::UnsignedShiftRight => {
                    writeln!(output, "    i{v} = (int64_t)((uint32_t)i{l} >> ((uint32_t)i{r} & 31)); d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                }
                BinaryOp::Remainder => {
                    let is_const_nonzero = defs.get(right).is_some_and(|op| match op {
                        GraphOp::Const { constant, .. } => {
                            as_safe_integer(constant).is_some_and(|k| k != 0)
                        }
                        _ => false,
                    });
                    let non_negative = ivs.get(left).is_some_and(|iv| iv.non_negative);
                    if is_const_nonzero {
                        if non_negative {
                            writeln!(output, "    i{v} = i{l} % i{r}; d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                        } else {
                            writeln!(output, "    i{v} = i{l} % i{r}; d{v} = (i{v} == 0 && i{l} < 0) ? -0.0 : (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                        }
                    } else {
                        writeln!(output, "    if (KILN_LIKELY(i{r} != 0)) {{ i{v} = i{l} % i{r}; d{v} = (i{v} == 0 && i{l} < 0) ? -0.0 : (double)i{v}; }} else {{ d{v} = NAN; i{v} = 0; }} v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                    }
                }
                BinaryOp::Add => {
                    if ivs.contains_key(&vid) {
                        writeln!(output, "    i{v} = i{l} + i{r}; d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                    } else if typed_int {
                        writeln!(
                            output,
                            "    {{ int64_t res; if (KILN_LIKELY(!__builtin_add_overflow(i{l}, i{r}, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res; d{v} = (double)res; }} else {{ d{v} = kiln_c99_int_slow((double)i{l} + (double)i{r}); i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0; }} v{v} = kiln_c99_val_from_double(d{v}); }}"
                        ).unwrap();
                    } else {
                        writeln!(output, "    d{v} = (double)i{l} + (double)i{r}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                    }
                }
                BinaryOp::Subtract => {
                    if ivs.contains_key(&vid) {
                        writeln!(output, "    i{v} = i{l} - i{r}; d{v} = (double)i{v}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                    } else if typed_int {
                        writeln!(
                            output,
                            "    {{ int64_t res; if (KILN_LIKELY(!__builtin_sub_overflow(i{l}, i{r}, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res; d{v} = (double)res; }} else {{ d{v} = kiln_c99_int_slow((double)i{l} - (double)i{r}); i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0; }} v{v} = kiln_c99_val_from_double(d{v}); }}"
                        ).unwrap();
                    } else {
                        writeln!(output, "    d{v} = (double)i{l} - (double)i{r}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                    }
                }
                BinaryOp::Multiply => {
                    writeln!(output, "    d{v} = (double)i{l} * (double)i{r}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                    if typed_int {
                        writeln!(output, "    i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0;").unwrap();
                    }
                }
                BinaryOp::Divide => {
                    writeln!(output, "    d{v} = (double)i{l} / (double)i{r}; v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                    if typed_int {
                        writeln!(output, "    i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0;").unwrap();
                    }
                }
                BinaryOp::Exponentiate => {
                    writeln!(output, "    d{v} = pow((double)i{l}, (double)i{r}); v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                    if typed_int {
                        writeln!(output, "    i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0;").unwrap();
                    }
                }
                BinaryOp::In | BinaryOp::InstanceOf => {
                    unreachable!("`in` and `instanceof` take the generic lowering")
                }
            }
        }
        GraphOp::Binary { op, left, right }
            if numbers.contains(left)
                && numbers.contains(right)
                && !matches!(op, BinaryOp::In | BinaryOp::InstanceOf) =>
        {
            // Both operands proven numbers: no tag test, no memcpy. A comparison yields the
            // boolean directly; arithmetic computes the double twin and boxes it. `in` and
            // `instanceof` on two numbers are a `TypeError` the generic lowering throws.
            let (l, r) = (left.0, right.0);
            let (a, b) = (format!("d{l}"), format!("d{r}"));
            match op {
                BinaryOp::StrictEqual | BinaryOp::LooseEqual => writeln!(
                    output,
                    "    v{v} = ({a} == {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                )
                .unwrap(),
                BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => writeln!(
                    output,
                    "    v{v} = ({a} != {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                )
                .unwrap(),
                BinaryOp::LessThan => writeln!(
                    output,
                    "    v{v} = ({a} < {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                )
                .unwrap(),
                BinaryOp::LessThanOrEqual => writeln!(
                    output,
                    "    v{v} = ({a} <= {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                )
                .unwrap(),
                BinaryOp::GreaterThan => writeln!(
                    output,
                    "    v{v} = ({a} > {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                )
                .unwrap(),
                BinaryOp::GreaterThanOrEqual => writeln!(
                    output,
                    "    v{v} = ({a} >= {b}) ? KILN_VAL_TRUE : KILN_VAL_FALSE;"
                )
                .unwrap(),
                BinaryOp::Add => {
                    writeln!(
                        output,
                        "    d{v} = {a} + {b}; v{v} = kiln_c99_val_from_double(d{v});"
                    )
                    .unwrap();
                    if typed_int {
                        writeln!(output, "    i{v} = (int64_t)d{v};").unwrap();
                    }
                }
                BinaryOp::Subtract => {
                    writeln!(
                        output,
                        "    d{v} = {a} - {b}; v{v} = kiln_c99_val_from_double(d{v});"
                    )
                    .unwrap();
                    if typed_int {
                        writeln!(output, "    i{v} = (int64_t)d{v};").unwrap();
                    }
                }
                BinaryOp::Multiply => {
                    writeln!(
                        output,
                        "    d{v} = {a} * {b}; v{v} = kiln_c99_val_from_double(d{v});"
                    )
                    .unwrap();
                    if typed_int {
                        writeln!(output, "    i{v} = (int64_t)d{v};").unwrap();
                    }
                }
                BinaryOp::Divide => {
                    writeln!(
                        output,
                        "    d{v} = {a} / {b}; v{v} = kiln_c99_val_from_double(d{v});"
                    )
                    .unwrap();
                    if typed_int {
                        writeln!(output, "    i{v} = (int64_t)d{v};").unwrap();
                    }
                }
                BinaryOp::Exponentiate => {
                    writeln!(
                        output,
                        "    d{v} = pow({a}, {b}); v{v} = kiln_c99_val_from_double(d{v});"
                    )
                    .unwrap();
                    if typed_int {
                        writeln!(output, "    i{v} = (int64_t)d{v};").unwrap();
                    }
                }
                BinaryOp::Remainder => {
                    writeln!(output, "    d{v} = ({b} > 0.0 && {b} <= 9007199254740992.0 && {a} >= 0.0 && {a} <= 9007199254740992.0 && {a} == (double)(int64_t){a} && {b} == (double)(int64_t){b}) ? (double)((int64_t){a} % (int64_t){b}) : fmod({a}, {b}); v{v} = kiln_c99_val_from_double(d{v});").unwrap();
                    if typed_int {
                        writeln!(output, "    i{v} = (int64_t)d{v};").unwrap();
                    }
                }
                BinaryOp::BitwiseAnd
                | BinaryOp::BitwiseOr
                | BinaryOp::BitwiseXor
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight
                | BinaryOp::UnsignedShiftRight => {
                    writeln!(output, "    v{v} = kiln_c99_val_binary_fast({}, v{l}, v{r}); memcpy(&d{v}, &v{v}, sizeof(d{v}));", binary_code(*op)).unwrap();
                    if typed_int {
                        writeln!(output, "    i{v} = (int64_t)d{v};").unwrap();
                    }
                }
                BinaryOp::In | BinaryOp::InstanceOf => {
                    unreachable!("`in` and `instanceof` take the generic lowering")
                }
            }
        }
        // `in` and `instanceof` are not arithmetic: they go inline against their own prelude
        // entry points and never to the binary helper, which answers something else for their
        // codes.
        GraphOp::Binary {
            op: BinaryOp::In,
            left,
            right,
        } => {
            writeln!(
                output,
                "    {{ kiln_value container = kiln_val_to_boundary(v{r}); int answer; if (!kiln_c99_is_reference(container) && container.tag != KILN_VALUE_DATE) {{ (void)kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, \"Cannot use 'in' to search for a property in a non-object\"); {unwind} }} int status = kiln_c99_has_computed(host, container, kiln_val_to_boundary(v{l}), &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; v{v} = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}",
                l = left.0,
                r = right.0
            )
            .unwrap();
        }
        GraphOp::Binary {
            op: BinaryOp::InstanceOf,
            left,
            right,
        } => {
            writeln!(
                output,
                "    {{ kiln_value constructor = kiln_val_to_boundary(v{r}); int answer; int status = kiln_c99_instance_of(kiln_val_to_boundary(v{l}), constructor, &answer); if (status != KILN_C99_OK) return status; v{v} = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}",
                l = left.0,
                r = right.0
            )
            .unwrap();
        }
        GraphOp::Binary { op, left, right } => {
            let (l, r) = (left.0, right.0);
            let code = binary_code(*op);
            // The slow path runs the program's own conversions and can throw; the pure helper
            // is only for a body that cannot hold a reference (`scalar_proto`).
            let slow = if scalar_proto {
                format!("v{v} = kiln_c99_val_binary_fast({code}, v{l}, v{r});")
            } else {
                let check = status_check(diagnostic_site, pc, label, unwind);
                format!(
                    "int status = KILN_C99_OK; v{v} = kiln_c99_val_binary({code}, host, v{l}, v{r}, &status, depth, thrown); {check}"
                )
            };
            let fast = binary_fast_body(*op).map(|f| {
                f.replace("LEFT", &format!("v{l}"))
                    .replace("RIGHT", &format!("v{r}"))
            });
            // A result proven a number -- by a restart, in a restartable body -- is computed
            // as its twin and boxed from it, so a box nothing reads costs nothing: the NaN
            // canonicalisation is the box's, not the arithmetic's. The slow path, where it
            // survives, keeps the twin in step.
            let raw = if typed && !typed_int {
                binary_raw_body(*op)
            } else {
                None
            };
            match (fast, raw) {
                (Some(_), Some(raw)) => writeln!(
                    output,
                    "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); d{v} = {raw}; v{v} = kiln_c99_val_from_double(d{v}); }} else {{ {slow} memcpy(&d{v}, &v{v}, sizeof(d{v})); }}"
                )
                .unwrap(),
                (Some(fast), None) => writeln!(
                    output,
                    "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = {fast}; }} else {{ {slow} }}"
                )
                .unwrap(),
                (None, _) => writeln!(output, "    {{ {slow} }}").unwrap(),
            }
            if typed_int {
                writeln!(output, "    if (KILN_VAL_IS_NUMBER(v{v})) {{ memcpy(&d{v}, &v{v}, sizeof(d{v})); i{v} = (int64_t)d{v}; }}").unwrap();
            } else if typed && raw.is_none() {
                // Proven a number by a restart: the guarded form ran, so the twin is the bits.
                writeln!(output, "    memcpy(&d{v}, &v{v}, sizeof(d{v}));").unwrap();
            }
        }
        // A call is the direct path on `kiln_val`: a plain closure that
        // publishes no frame is entered directly, a body this unit compiled is called by name so
        // the C compiler may inline it, and everything else goes through the runtime.
        GraphOp::Call { callee, args, .. }
            if plan.known.get(callee).is_some_and(|&target| {
                direct_callable(
                    &program.functions[target as usize],
                    plan.publishes_frame[target as usize],
                )
            }) =>
        {
            // The callee is the closure of a proto this unit compiled, so the body is called by
            // name: no callable test, no body-pointer comparison, and the receiver the language
            // gives a plain function, decided here. A site that proved every argument a number
            // enters the typed clone. What the direct path does not cover -- a `super()` in
            // flight, the depth limit -- takes the generic call, which answers the same way.
            let target = plan.known[callee];
            let target_proto = &program.functions[target as usize];
            let argc = args.len();
            let init = if args.is_empty() {
                "KILN_VAL_UNDEFINED".to_owned()
            } else {
                args.iter()
                    .map(|a| format!("v{}", a.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let in_typed = matches!(kind, EntryKind::Typed { .. });
            let sloppy = !strict_functions[target as usize];
            // A function calling itself through its own binding is calling the very closure
            // that is running: its upvalue table is this one, and the callee value need not be
            // read at all -- and, when the callee is an arrow, neither is its `this`: the
            // closure now running is the one being called, so its captured `this` is already
            // in scope as `receiver`.
            let own_call = matches!(self_index, Some(index) if index == target);
            let upvalues = if own_call {
                "upvalues".to_owned()
            } else {
                format!("((kiln_closure *)KILN_VAL_PTR(v{}))->upvalues", callee.0)
            };
            // An arrow has no `this` of its own: it always runs with the closure it is,
            // `captured_this`, exactly as `plain->arrow ? plain->captured_this : ..` decides it
            // for the runtime's `kiln_c99_invoke_closure` path -- this is where the direct path
            // decides the same thing. A plain function gets the language's ordinary receiver:
            // `undefined`, substituted with the global object in sloppy code.
            let receiver = if target_proto.arrow {
                if own_call {
                    "kiln_val recv = receiver;".to_owned()
                } else {
                    format!(
                        "kiln_val recv = ((kiln_closure *)KILN_VAL_PTR(v{}))->captured_this;",
                        callee.0
                    )
                }
            } else if sloppy {
                "kiln_val recv = KILN_VAL_UNDEFINED; (void)kiln_c99_global_object_val(&recv);"
                    .to_owned()
            } else {
                "kiln_val recv = KILN_VAL_UNDEFINED;".to_owned()
            };
            let check = status_check(diagnostic_site, pc, label, unwind);
            // Past the depth limit a boxed body takes the generic call, which throws the
            // `RangeError`; a typed entry leaves it pending and fails.
            let overflow = if in_typed {
                format!(
                    "{{ kiln_c99_typed_overflow(); return {}; }}",
                    typed_fail(kind)
                )
            } else {
                format!(
                    "{{ kiln_val call_args[{n}] = {{ {init} }}; int status = kiln_c99_val_call(host, v{c}, {argc} > 0 ? call_args : NULL, (size_t){argc}, depth, thrown, \"{diagnostic_site}\", (size_t){pc}, \"{label}\", &v{v}); {check} }}",
                    n = argc.max(1),
                    c = callee.0
                )
            };
            if plan.typed_sites.contains(&vid) {
                let needs = plan.typed_needs[target as usize];
                let mut call_args = String::from("depth + 1");
                if needs.upvalues {
                    call_args.push_str(&format!(", {upvalues}"));
                }
                if needs.receiver {
                    call_args.push_str(", recv");
                }
                if needs.row {
                    call_args.push_str(", NULL");
                }
                if needs.runtime {
                    call_args.push_str(", host, thrown");
                }
                for a in args {
                    call_args.push_str(&format!(", d{}", a.0));
                }
                let recv = if needs.receiver {
                    receiver.as_str()
                } else {
                    ""
                };
                // A failed typed callee has left its status pending. A typed caller with a
                // handler hands a throw to it; without one it fails on, the status still
                // pending. A boxed caller answers as it would to any status.
                let failed = if in_typed && unwind != "goto kiln_unwind;" {
                    format!(
                        "{{ if (kiln_c99_typed_status == KILN_C99_THROWN) {{ *thrown = kiln_c99_typed_thrown; {unwind} }} return {}; }}",
                        typed_fail(kind)
                    )
                } else if in_typed {
                    format!("return {};", typed_fail(kind))
                } else {
                    format!(
                        "{{ int status = kiln_c99_typed_status; if (status == KILN_C99_THROWN) {{ *thrown = kiln_c99_typed_thrown; {unwind} }} {check} }}"
                    )
                };
                if plan.typed_returns[target as usize] {
                    writeln!(
                        output,
                        "    if (KILN_UNLIKELY(depth >= KILN_C99_MAX_CALL_DEPTH)) {overflow} else {{ {recv} d{v} = {symbol}_fn_{target}_t({call_args}); if (KILN_UNLIKELY(kiln_c99_typed_failed(d{v}))) {failed} v{v} = kiln_c99_val_from_double(d{v}); }}"
                    )
                    .unwrap();
                } else {
                    let twin = if typed {
                        format!(" memcpy(&d{v}, &v{v}, sizeof(d{v}));")
                    } else {
                        String::new()
                    };
                    writeln!(
                        output,
                        "    if (KILN_UNLIKELY(depth >= KILN_C99_MAX_CALL_DEPTH)) {overflow} else {{ {recv} v{v} = {symbol}_fn_{target}_t({call_args}); if (KILN_UNLIKELY(v{v} == KILN_VAL_TYPED_FAIL)) {failed}{twin} }}"
                    )
                    .unwrap();
                }
            } else {
                let twin = if typed {
                    format!(" memcpy(&d{v}, &v{v}, sizeof(d{v}));")
                } else {
                    String::new()
                };
                writeln!(
                    output,
                    "    if (KILN_UNLIKELY(depth >= KILN_C99_MAX_CALL_DEPTH)) {overflow} else {{ kiln_val call_args[{n}] = {{ {init} }}; {receiver} kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; int status = {symbol}_fn_{target}{fast}(&ctx_call, &v{v}, {upvalues}, &recv, {argc} > 0 ? call_args : NULL, (size_t){argc}); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status;{twin} }}",
                    fast = if plan.fast.get(target as usize).copied().unwrap_or(false) { "_f" } else { "" },
                    n = argc.max(1)
                )
                .unwrap();
            }
        }
        GraphOp::Call { callee, args, .. } => {
            let argc = args.len();
            let mut init = format!("v{}", callee.0);
            for arg in args {
                init.push_str(&format!(", v{}", arg.0));
            }
            let mut static_branches = String::new();
            if program.functions.len() <= 32 {
                for i in 0..program.functions.len() {
                    let fn_name = format!("{symbol}_fn_{i}");
                    let entry = if plan.fast.get(i).copied().unwrap_or(false) {
                        format!("{fn_name}_f")
                    } else {
                        fn_name.clone()
                    };
                    let keyword = if static_branches.is_empty() {
                        "if"
                    } else {
                        " else if"
                    };
                    static_branches.push_str(&format!(
                        "{keyword} (plain->body == (kiln_body_fn){fn_name}) {{ status = {entry}(&ctx_call, &call_result, plain->upvalues, &recv, call_args, (size_t){argc}); }}"
                    ));
                }
            }
            let indirect = if static_branches.is_empty() {
                "status = plain->body(&ctx_call, &call_result, plain->upvalues, &recv, call_args, (size_t)ARGC);".replace("ARGC", &argc.to_string())
            } else {
                format!(
                    "{static_branches} else {{ status = plain->body(&ctx_call, &call_result, plain->upvalues, &recv, call_args, (size_t){argc}); }}"
                )
            };
            let check = status_check(diagnostic_site, pc, label, unwind);
            writeln!(
                output,
                "    {{ kiln_val call_stack[{n}] = {{ {init} }}; const kiln_val *call_args = {argc} > 0 ? &call_stack[1] : NULL; kiln_closure *plain = KILN_VAL_IS_CLOSURE(call_stack[0]) ? (kiln_closure *)KILN_VAL_PTR(call_stack[0]) : NULL; if (KILN_LIKELY(plain != NULL && !plain->publishes_frame && !plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH)) {{ kiln_val call_result = KILN_VAL_UNDEFINED; kiln_val recv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&recv); kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; int status; {indirect} if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; v{v} = call_result; }} else {{ int status = kiln_c99_val_call(host, call_stack[0], call_args, (size_t){argc}, depth, thrown, \"{diagnostic_site}\", (size_t){pc}, \"{label}\", &v{v}); {check} }} }}",
                n = argc + 1
            )
            .unwrap();
        }
        GraphOp::CallMethod {
            object, name, args, ..
        } => {
            let argc = args.len();
            let key_str = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.as_str(),
                _ => "",
            };
            let key_len = key_str.len();
            // The unit's own methods of this name: a cached callee whose body is one of them is
            // called by name, and the C compiler may inline it into this site.
            let candidates = method_candidates(program, key_str, &plan.publishes_frame);
            // A candidate with a typed entry, at a site that proved every argument a number,
            // is entered typed: the receiver is the object the cache matched, so what the
            // language decides about a method's `this` -- an arrow's captured one, a sloppy
            // body's global object -- is decided here, by the proto, and not read from the
            // closure. The generator, frame and arrow tests are the proto's too.
            let typed_candidates: Vec<u32> = candidates
                .iter()
                .copied()
                .filter(|&i| {
                    let callee = &program.functions[i as usize];
                    plan.typed_entries.get(i as usize).copied().unwrap_or(false)
                        && !callee.arrow
                        && callee.parameter_count as usize == argc
                        && args.iter().all(|a| numbers.contains(a))
                })
                .collect();
            let diag = format!(
                "if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) {{ if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(v{o}).tag; if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"CallMethod invocation refused\"; kiln_c99_note_refusal(\"{diagnostic_site}\", (size_t){pc}, \"{label}\", (size_t)kiln_c99_diagnostic_active_depth); }} else if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {{ if (kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(v{o}).tag; kiln_c99_diagnostic_detail = \"CallMethod invocation threw\"; }} kiln_c99_note_refusal(\"{diagnostic_site}\", (size_t){pc}, \"{label}\", (size_t)kiln_c99_diagnostic_active_depth); }}",
                o = object.0
            );
            let typed_check = status_check(diagnostic_site, pc, label, unwind);
            let typed_chain: String = typed_candidates
                .iter()
                .map(|&i| {
                    let needs = plan.typed_needs[i as usize];
                    let mut call_args = String::from("depth + 1");
                    if needs.upvalues {
                        call_args.push_str(", plain->upvalues");
                    }
                    if needs.receiver {
                        call_args.push_str(&format!(", v{}", object.0));
                    }
                    // The callee's row for this receiver's shape, remembered here from the
                    // callee's own table after the first call, so the callee does no lookup.
                    let (row_arg, row_learn) = if needs.row {
                        let columns = plan.receiver_columns.get(i as usize).copied().unwrap_or(0);
                        call_args.push_str(&format!(", obj->shape == kiln_typed_shape_{i} ? kiln_typed_row_{i} : NULL"));
                        (
                            String::new(),
                            format!(
                                " if (obj->shape != kiln_typed_shape_{i}) {{ kiln_typed_shape_{i} = obj->shape; kiln_typed_row_{i} = kiln_c99_recv_row({symbol}_fn_{i}_recv_shape, &{symbol}_fn_{i}_recv_slot[0][0], {columns}, obj->shape); }}"
                            ),
                        )
                    } else {
                        (String::new(), String::new())
                    };
                    let _ = row_arg;
                    if needs.runtime {
                        call_args.push_str(", host, thrown");
                    }
                    for a in args {
                        call_args.push_str(&format!(", d{}", a.0));
                    }
                    let failed = format!(
                        "{{ int status = kiln_c99_typed_status; if (status == KILN_C99_THROWN) *thrown = kiln_c99_typed_thrown; {diag} {typed_check} }}"
                    );
                    // The closure that last entered this candidate here is remembered, and a
                    // cached callee equal to it is that closure: the site just read it live
                    // from where the method lives. Its body is then known without a load.
                    let matched = format!(
                        "(cached_fn == kiln_typed_fn_{i} || (plain->body == (kiln_body_fn){symbol}_fn_{i} && (kiln_typed_fn_{i} = cached_fn) != 0)) && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH"
                    );
                    if plan.typed_returns[i as usize] {
                        let twin = if typed { format!(" d{v} = kiln_typed_d;") } else { String::new() };
                        format!(
                            "if ({matched}) {{ double kiln_typed_d = {symbol}_fn_{i}_t({call_args}); if (KILN_UNLIKELY(kiln_c99_typed_failed(kiln_typed_d))) {failed} else {{ v{v} = kiln_c99_val_from_double(kiln_typed_d);{twin}{row_learn} }} handled = 1; }} else "
                        )
                    } else {
                        format!(
                            "if ({matched}) {{ kiln_val kiln_typed_r = {symbol}_fn_{i}_t({call_args}); if (KILN_UNLIKELY(kiln_typed_r == KILN_VAL_TYPED_FAIL)) {failed} else {{ v{v} = kiln_typed_r;{row_learn} }} handled = 1; }} else "
                        )
                    }
                })
                .collect();
            let typed_remembered: String = typed_candidates
                .iter()
                .map(|i| {
                    if plan.typed_needs[*i as usize].row {
                        format!("static kiln_val kiln_typed_fn_{i} = 0; static const kiln_shape *kiln_typed_shape_{i} = NULL; static const uint32_t *kiln_typed_row_{i} = NULL; ")
                    } else {
                        format!("static kiln_val kiln_typed_fn_{i} = 0; ")
                    }
                })
                .collect();
            let by_name: String = candidates
                .iter()
                .filter(|i| !typed_candidates.contains(i))
                .enumerate()
                .map(|(k, i)| {
                    format!(
                        "{} (plain->body == (kiln_body_fn){symbol}_fn_{i}) {{ status = {symbol}_fn_{i}{fast}(&ctx_call, &v{v}, plain->upvalues, &vreceiver, call_args, (size_t){argc}); }} else",
                        if k == 0 { "if" } else { " if" },
                        fast = if plan.fast.get(*i as usize).copied().unwrap_or(false) { "_f" } else { "" }
                    )
                })
                .collect();
            let init = if args.is_empty() {
                "KILN_VAL_UNDEFINED".to_owned()
            } else {
                args.iter()
                    .map(|a| format!("v{}", a.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let check = status_check(diagnostic_site, pc, label, unwind);
            let closed_world = !program.has_direct_eval() && !program.has_dynamic_import();

            // 1. Math.* intrinsic if closed_world
            let math_intrinsic = if closed_world {
                match (key_str, argc) {
                    ("sqrt", 1) => Some(format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}))) {{ \
                                 double d; memcpy(&d, &v{a0}, sizeof(d)); \
                                 v{v} = kiln_c99_val_from_double(sqrt(d)); \
                                 handled = 1; \
                             }} \
                         }}",
                        o = object.0,
                        a0 = args[0].0,
                        v = v
                    )),
                    ("floor", 1) => Some(format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}))) {{ \
                                 double d; memcpy(&d, &v{a0}, sizeof(d)); \
                                 v{v} = kiln_c99_val_from_double(floor(d)); \
                                 handled = 1; \
                             }} \
                         }}",
                        o = object.0,
                        a0 = args[0].0,
                        v = v
                    )),
                    ("ceil", 1) => Some(format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}))) {{ \
                                 double d; memcpy(&d, &v{a0}, sizeof(d)); \
                                 v{v} = kiln_c99_val_from_double(ceil(d)); \
                                 handled = 1; \
                             }} \
                         }}",
                        o = object.0,
                        a0 = args[0].0,
                        v = v
                    )),
                    ("round", 1) => Some(format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}))) {{ \
                                 double d; memcpy(&d, &v{a0}, sizeof(d)); \
                                 double r = isfinite(d) ? floor(d + 0.5) : d; \
                                 if (r == 0.0 && signbit(d)) r = -0.0; \
                                 v{v} = kiln_c99_val_from_double(r); \
                                 handled = 1; \
                             }} \
                         }}",
                        o = object.0,
                        a0 = args[0].0,
                        v = v
                    )),
                    ("trunc", 1) => Some(format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}))) {{ \
                                 double d; memcpy(&d, &v{a0}, sizeof(d)); \
                                 v{v} = kiln_c99_val_from_double(trunc(d)); \
                                 handled = 1; \
                             }} \
                         }}",
                        o = object.0,
                        a0 = args[0].0,
                        v = v
                    )),
                    ("abs", 1) => Some(format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}))) {{ \
                                 double d; memcpy(&d, &v{a0}, sizeof(d)); \
                                 v{v} = kiln_c99_val_from_double(fabs(d)); \
                                 handled = 1; \
                             }} \
                         }}",
                        o = object.0,
                        a0 = args[0].0,
                        v = v
                    )),
                    ("sign", 1) => Some(format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}))) {{ \
                                 double d; memcpy(&d, &v{a0}, sizeof(d)); \
                                 v{v} = kiln_c99_val_from_double(isnan(d) || d == 0.0 ? d : (d < 0.0 ? -1.0 : 1.0)); \
                                 handled = 1; \
                             }} \
                         }}",
                        o = object.0,
                        a0 = args[0].0,
                        v = v
                    )),
                    ("pow", 2) => Some(format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}) && KILN_VAL_IS_NUMBER(v{a1}))) {{ \
                                 double d0, d1; memcpy(&d0, &v{a0}, sizeof(d0)); memcpy(&d1, &v{a1}, sizeof(d1)); \
                                 v{v} = kiln_c99_val_from_double(pow(d0, d1)); \
                                 handled = 1; \
                             }} \
                         }}",
                        o = object.0,
                        a0 = args[0].0,
                        a1 = args[1].0,
                        v = v
                    )),
                    ("min", 2) => Some(format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}) && KILN_VAL_IS_NUMBER(v{a1}))) {{ \
                                 double d0, d1; memcpy(&d0, &v{a0}, sizeof(d0)); memcpy(&d1, &v{a1}, sizeof(d1)); \
                                 double res = (isnan(d0) || isnan(d1)) ? NAN : ((d0 < d1 || (d0 == 0.0 && d1 == 0.0 && signbit(d0))) ? d0 : d1); \
                                 v{v} = kiln_c99_val_from_double(res); \
                                 handled = 1; \
                             }} \
                         }}",
                        o = object.0,
                        a0 = args[0].0,
                        a1 = args[1].0,
                        v = v
                    )),
                    ("max", 2) => Some(format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}) && KILN_VAL_IS_NUMBER(v{a1}))) {{ \
                                 double d0, d1; memcpy(&d0, &v{a0}, sizeof(d0)); memcpy(&d1, &v{a1}, sizeof(d1)); \
                                 double res = (isnan(d0) || isnan(d1)) ? NAN : ((d0 > d1 || (d0 == 0.0 && d1 == 0.0 && !signbit(d0))) ? d0 : d1); \
                                 v{v} = kiln_c99_val_from_double(res); \
                                 handled = 1; \
                             }} \
                         }}",
                        o = object.0,
                        a0 = args[0].0,
                        a1 = args[1].0,
                        v = v
                    )),
                    _ => None,
                }
            } else {
                None
            };

            // 2. Array push intrinsic
            let push_intrinsic = if key_str == "push" && argc == 1 {
                Some(format!(
                    "if (!handled && kiln_c99_val_array_push_fast(v{o}, v{a0}, &v{v}) == KILN_C99_OK) {{ \
                         handled = 1; \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                ))
            } else {
                None
            };

            // 3. RegExp test and exec intrinsics
            let regex_intrinsic = if argc == 1 && (key_str == "test" || key_str == "exec") {
                let func = if key_str == "test" {
                    "kiln_c99_val_regexp_test_fast"
                } else {
                    "kiln_c99_val_regexp_exec_fast"
                };
                Some(format!(
                    "if (!handled && {func}(v{o}, v{a0}, &v{v}) == KILN_C99_OK) {{ \
                         handled = 1; \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                ))
            } else {
                None
            };

            let map_intrinsic = if closed_world && key_str == "map" && argc == 1 {
                Some(format!(
                    "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                             if (!plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                                 kiln_value built_val; \
                                 int status = kiln_c99_new_array_with_capacity(&built_val, arr->count, arr->is_numeric); \
                                 if (status != KILN_C99_OK) return status; \
                                 kiln_array *built_arr = kiln_c99_array_of(built_val); \
                                 kiln_val vrecv = plain->arrow ? plain->captured_this : v{o}; \
                                 if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                                 for (size_t idx = 0; idx < arr->count; ++idx) {{ \
                                     if (arr->elements[idx] == KILN_VAL_HOLE) {{ \
                                         built_arr->elements[built_arr->count++] = KILN_VAL_HOLE; \
                                         built_arr->is_numeric = 0; \
                                         continue; \
                                     }} \
                                     kiln_val vargs[3] = {{ arr->elements[idx], kiln_c99_val_from_double((double)idx), v{o} }}; \
                                     kiln_val prod = KILN_VAL_UNDEFINED; \
                                     int status; \
                                     if (!plain->publishes_frame) {{ kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)3); }} \
                                     else status = kiln_c99_invoke_closure(host, &prod, plain, vrecv, vargs, (size_t)3, depth, thrown, NULL, 0); \
                                     if (KILN_UNLIKELY(status != KILN_C99_OK)) {{ \
                                         {check} \
                                     }} \
                                     if (built_arr->is_numeric && !KILN_VAL_IS_NUMBER(prod)) {{ \
                                         built_arr->is_numeric = 0; \
                                     }} \
                                     built_arr->elements[built_arr->count++] = prod; \
                                 }} \
                                 v{v} = kiln_val_from_boundary(built_val); \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v,
                    check = check
                ))
            } else {
                None
            };

            let filter_intrinsic = if closed_world && key_str == "filter" && argc == 1 {
                Some(format!(
                    "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                             if (!plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                                 kiln_value built_val; \
                                 int status = kiln_c99_new_array_with_capacity(&built_val, arr->count, arr->is_numeric); \
                                 if (status != KILN_C99_OK) return status; \
                                 kiln_array *built_arr = kiln_c99_array_of(built_val); \
                                 kiln_val vrecv = plain->arrow ? plain->captured_this : v{o}; \
                                 if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                                 for (size_t idx = 0; idx < arr->count; ++idx) {{ \
                                     if (arr->elements[idx] == KILN_VAL_HOLE) continue; \
                                     kiln_val elem = arr->elements[idx]; \
                                     kiln_val vargs[3] = {{ elem, kiln_c99_val_from_double((double)idx), v{o} }}; \
                                     kiln_val prod = KILN_VAL_UNDEFINED; \
                                     int status; \
                                     if (!plain->publishes_frame) {{ kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)3); }} \
                                     else status = kiln_c99_invoke_closure(host, &prod, plain, vrecv, vargs, (size_t)3, depth, thrown, NULL, 0); \
                                     if (KILN_UNLIKELY(status != KILN_C99_OK)) {{ \
                                         {check} \
                                     }} \
                                     if (kiln_c99_val_truthy(prod)) {{ \
                                         built_arr->elements[built_arr->count++] = elem; \
                                     }} \
                                 }} \
                                 v{v} = kiln_val_from_boundary(built_val); \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v,
                    check = check
                ))
            } else {
                None
            };

            let reduce_intrinsic = if closed_world && key_str == "reduce" && argc == 2 {
                Some(format!(
                    "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                             if (!plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                                 kiln_val vacc = v{a1}; \
                                 kiln_val vrecv = plain->arrow ? plain->captured_this : v{o}; \
                                 if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                                 for (size_t idx = 0; idx < arr->count; ++idx) {{ \
                                     if (arr->elements[idx] == KILN_VAL_HOLE) continue; \
                                     kiln_val vargs[4] = {{ vacc, arr->elements[idx], kiln_c99_val_from_double((double)idx), v{o} }}; \
                                     kiln_val prod = KILN_VAL_UNDEFINED; \
                                     int status; \
                                     if (!plain->publishes_frame) {{ kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)4); }} \
                                     else status = kiln_c99_invoke_closure(host, &prod, plain, vrecv, vargs, (size_t)4, depth, thrown, NULL, 0); \
                                     if (KILN_UNLIKELY(status != KILN_C99_OK)) {{ \
                                         {check} \
                                     }} \
                                     vacc = prod; \
                                 }} \
                                 v{v} = vacc; \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    a1 = args[1].0,
                    v = v,
                    check = check
                ))
            } else {
                None
            };

            let slice_intrinsic = if closed_world && key_str == "slice" && argc == 1 {
                Some(format!(
                    "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_NUMBER(v{a0})) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             double d0; memcpy(&d0, &v{a0}, sizeof(d0)); \
                             if (d0 >= 0.0 && d0 == floor(d0)) {{ \
                                 size_t start = (size_t)d0; \
                                 size_t slice_len = start < arr->count ? arr->count - start : 0; \
                                 kiln_value built_val; \
                                 int status = kiln_c99_new_array_with_capacity(&built_val, slice_len, arr->is_numeric); \
                                 if (status != KILN_C99_OK) return status; \
                                 if (slice_len > 0) {{ \
                                     kiln_array *b_arr = kiln_c99_array_of(built_val); \
                                     memcpy(b_arr->elements, arr->elements + start, slice_len * sizeof(kiln_val)); \
                                     b_arr->count = slice_len; \
                                 }} \
                                 v{v} = kiln_val_from_boundary(built_val); \
                                 handled = 1; \
                             }} \
                         }} \
                     }} else if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_NUMBER(v{a0})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         if (s_recv != NULL && s_recv->length == s_recv->byte_len) {{ \
                             double d0; memcpy(&d0, &v{a0}, sizeof(d0)); \
                             if (d0 >= 0.0 && d0 == floor(d0)) {{ \
                                 size_t start = (size_t)d0; \
                                 size_t slen = start < s_recv->byte_len ? s_recv->byte_len - start : 0; \
                                 v{v} = kiln_c99_val_from_ascii(s_recv->bytes + start, slen); \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                ))
            } else {
                None
            };

            let join_intrinsic = if closed_world && key_str == "join" && argc == 1 {
                Some(format!(
                    "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_STRING(v{a0})) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             const kiln_string *sep = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                             size_t total_bytes = 0; \
                             int can_fast = (sep != NULL); \
                             for (size_t idx = 0; idx < arr->count && can_fast; ++idx) {{ \
                                 kiln_val elem = arr->elements[idx]; \
                                 if (KILN_VAL_IS_STRING(elem)) {{ \
                                     const kiln_string *es = (const kiln_string *)KILN_VAL_PTR(elem); \
                                     if (es) total_bytes += es->byte_len; \
                                 }} else {{ can_fast = 0; }} \
                             }} \
                             if (can_fast) {{ \
                                 if (arr->count > 1) total_bytes += (arr->count - 1) * sep->byte_len; \
                                 char *buf = kiln_c99_gc_alloc_text(total_bytes); \
                                 if (buf != NULL) {{ \
                                     char *dst = buf; \
                                     size_t total_len = 0; \
                                     for (size_t idx = 0; idx < arr->count; ++idx) {{ \
                                         if (idx > 0 && sep->byte_len > 0) {{ \
                                             memcpy(dst, sep->bytes, sep->byte_len); \
                                             dst += sep->byte_len; \
                                             total_len += sep->length; \
                                         }} \
                                         const kiln_string *es = (const kiln_string *)KILN_VAL_PTR(arr->elements[idx]); \
                                         if (es && es->byte_len > 0) {{ \
                                             memcpy(dst, es->bytes, es->byte_len); \
                                             dst += es->byte_len; \
                                             total_len += es->length; \
                                         }} \
                                     }} \
                                     kiln_string *s = ((kiln_string *)buf) - 1; \
                                     s->length = total_len; \
                                     s->byte_len = total_bytes; \
                                     s->bytes = buf; \
                                     v{v} = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, s); \
                                     handled = 1; \
                                 }} \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                ))
            } else {
                None
            };

            let split_intrinsic = if closed_world && key_str == "split" && argc == 1 {
                Some(format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_STRING(v{a0})) {{ \
                         const kiln_string *src = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         const kiln_string *sep = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                         if (src != NULL && sep != NULL && src->length == src->byte_len && sep->byte_len == 1) {{ \
                             char ch = sep->bytes[0]; \
                             size_t part_count = 1; \
                             for (size_t i = 0; i < src->byte_len; ++i) {{ \
                                 if (src->bytes[i] == ch) part_count++; \
                             }} \
                             kiln_value built_val; \
                             int status = kiln_c99_new_array_with_capacity(&built_val, part_count, 0); \
                             if (status == KILN_C99_OK) {{ \
                                 kiln_array *arr = kiln_c99_array_of(built_val); \
                                 const char *p = src->bytes; \
                                 const char *end = src->bytes + src->byte_len; \
                                 while (p <= end) {{ \
                                     const char *next = (const char *)memchr(p, ch, (size_t)(end - p)); \
                                     size_t piece_len = next ? (size_t)(next - p) : (size_t)(end - p); \
                                     kiln_val piece_val = kiln_c99_val_from_ascii(p, piece_len); \
                                     arr->elements[arr->count++] = piece_val; \
                                     if (next == NULL) break; \
                                     p = next + 1; \
                                 }} \
                                 v{v} = kiln_val_from_boundary(built_val); \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                ))
            } else {
                None
            };

            let index_of_intrinsic = if closed_world && key_str == "indexOf" && argc == 1 {
                Some(format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_STRING(v{a0})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         const kiln_string *s_ndl = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                         if (s_recv != NULL && s_ndl != NULL && s_recv->length == s_recv->byte_len && s_ndl->length == s_ndl->byte_len) {{ \
                             const char *found = kiln_c99_memmem(s_recv->bytes, s_recv->byte_len, s_ndl->bytes, s_ndl->byte_len); \
                             v{v} = kiln_c99_val_from_double(found ? (double)(found - s_recv->bytes) : -1.0); \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                ))
            } else {
                None
            };

            let substring_intrinsic = if closed_world && key_str == "substring" && argc == 2 {
                Some(format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_NUMBER(v{a0}) && KILN_VAL_IS_NUMBER(v{a1})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         if (s_recv != NULL && s_recv->length == s_recv->byte_len) {{ \
                             double d0, d1; memcpy(&d0, &v{a0}, sizeof(d0)); memcpy(&d1, &v{a1}, sizeof(d1)); \
                             if (d0 == floor(d0) && d1 == floor(d1) && d0 >= 0.0 && d1 >= d0 && (size_t)d1 <= s_recv->byte_len) {{ \
                                 size_t start = (size_t)d0; \
                                 size_t len = (size_t)d1 - start; \
                                 v{v} = kiln_c99_val_from_ascii(s_recv->bytes + start, len); \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    a1 = args[1].0,
                    v = v
                ))
            } else {
                None
            };

            let collection_intrinsic = match (key_str, argc) {
                ("set", 2) => Some(format!(
                    "if (!handled && ((v{o} & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX || (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {{ \
                             kiln_collection *col = (kiln_collection *)ptr; \
                             if (col->is_map) {{ \
                                 kiln_c99_collection_put_val_fast(col, v{a0}, v{a1}); \
                                 v{v} = v{o}; \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    a1 = args[1].0,
                    v = v
                )),
                ("get", 1) => Some(format!(
                    "if (!handled && ((v{o} & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX || (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {{ \
                             kiln_collection *col = (kiln_collection *)ptr; \
                             if (col->is_map) {{ \
                                 v{v} = kiln_c99_collection_get_val_fast(col, v{a0}); \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                ("has", 1) => Some(format!(
                    "if (!handled && ((v{o} & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX || (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {{ \
                             kiln_collection *col = (kiln_collection *)ptr; \
                             v{v} = kiln_c99_collection_has_val_fast(col, v{a0}); \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                ("add", 1) => Some(format!(
                    "if (!handled && ((v{o} & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX || (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {{ \
                             kiln_collection *col = (kiln_collection *)ptr; \
                             if (!col->is_map) {{ \
                                 kiln_c99_collection_put_val_fast(col, v{a0}, v{a0}); \
                                 v{v} = v{o}; \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                ("delete", 1) => Some(format!(
                    "if (!handled && ((v{o} & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX || (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {{ \
                             kiln_collection *col = (kiln_collection *)ptr; \
                             v{v} = kiln_c99_collection_delete_val_fast(col, v{a0}); \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                ("clear", 0) => Some(format!(
                    "if (!handled && ((v{o} & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX || (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {{ \
                             kiln_collection *col = (kiln_collection *)ptr; \
                             col->count = 0; \
                             col->live = 0; \
                             if (col->hash_table != NULL && col->hash_capacity > 0) {{ \
                                 memset(col->hash_table, 0, col->hash_capacity * sizeof(uint32_t)); \
                             }} \
                             v{v} = KILN_VAL_UNDEFINED; \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    v = v
                )),
                _ => None,
            };

            let json_intrinsic = match (key_str, argc) {
                ("stringify", 1) => Some(format!(
                    "if (!handled && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_JSON) {{ \
                         if (kiln_c99_val_json_stringify_fast(host, v{a0}, &v{v}, depth, thrown) == KILN_C99_OK) {{ \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                ("parse", 1) => Some(format!(
                    "if (!handled && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_JSON) {{ \
                         if (kiln_c99_val_json_parse_fast(host, v{a0}, &v{v}, depth, thrown) == KILN_C99_OK) {{ \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                _ => None,
            };

            let sort_intrinsic = if key_str == "sort" && argc <= 1 {
                let cmp_expr = if argc == 1 {
                    format!("v{}", args[0].0)
                } else {
                    "KILN_VAL_UNDEFINED".to_string()
                };
                Some(format!(
                    "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             if (kiln_c99_val_array_sort_fast(host, arr, {cmp}, depth, thrown) == KILN_C99_OK) {{ \
                                 v{v} = v{o}; \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    cmp = cmp_expr,
                    v = v
                ))
            } else {
                None
            };

            let array_extra_intrinsic = match (key_str, argc) {
                ("pop", 0) => Some(format!(
                    "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             if (!arr->frozen && !arr->not_extensible) {{ \
                                 v{v} = arr->count > 0 ? arr->elements[--arr->count] : KILN_VAL_UNDEFINED; \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    v = v
                )),
                ("shift", 0) => Some(format!(
                    "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             if (!arr->frozen && !arr->not_extensible) {{ \
                                 if (arr->count > 0) {{ \
                                     kiln_val first = arr->elements[0]; \
                                     arr->count--; \
                                     if (arr->count > 0) memmove(arr->elements, arr->elements + 1, arr->count * sizeof(kiln_val)); \
                                     v{v} = first; \
                                 }} else {{ \
                                     v{v} = KILN_VAL_UNDEFINED; \
                                 }} \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    v = v
                )),
                ("includes", 1) => Some(format!(
                    "if (!handled) {{ \
                         if ((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {{ \
                             void *ptr = KILN_VAL_PTR(v{o}); \
                             if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                                 kiln_array *arr = (kiln_array *)ptr; \
                                 int inc = 0; \
                                 for (size_t i = 0; i < arr->count; ++i) {{ \
                                     if (arr->elements[i] == KILN_VAL_HOLE) {{ \
                                         if (v{a0} == KILN_VAL_UNDEFINED) {{ inc = 1; break; }} \
                                         continue; \
                                     }} \
                                     if (kiln_c99_val_same_value_zero(arr->elements[i], v{a0})) {{ inc = 1; break; }} \
                                 }} \
                                 v{v} = inc ? KILN_VAL_TRUE : KILN_VAL_FALSE; \
                                 handled = 1; \
                             }} \
                         }} else if (KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_STRING(v{a0})) {{ \
                             const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                             const kiln_string *s_ndl = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                             if (s_recv != NULL && s_ndl != NULL) {{ \
                                 const char *found = kiln_c99_memmem(s_recv->bytes, s_recv->byte_len, s_ndl->bytes, s_ndl->byte_len); \
                                 v{v} = found ? KILN_VAL_TRUE : KILN_VAL_FALSE; \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                _ => None,
            };

            let string_extra_intrinsic = match (key_str, argc) {
                ("startsWith", 1) => Some(format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_STRING(v{a0})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         const kiln_string *s_ndl = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                         if (s_recv != NULL && s_ndl != NULL) {{ \
                             int sw = (s_recv->byte_len >= s_ndl->byte_len && memcmp(s_recv->bytes, s_ndl->bytes, s_ndl->byte_len) == 0); \
                             v{v} = sw ? KILN_VAL_TRUE : KILN_VAL_FALSE; \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                ("endsWith", 1) => Some(format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_STRING(v{a0})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         const kiln_string *s_ndl = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                         if (s_recv != NULL && s_ndl != NULL) {{ \
                             int ew = (s_recv->byte_len >= s_ndl->byte_len && memcmp(s_recv->bytes + (s_recv->byte_len - s_ndl->byte_len), s_ndl->bytes, s_ndl->byte_len) == 0); \
                             v{v} = ew ? KILN_VAL_TRUE : KILN_VAL_FALSE; \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                ("charCodeAt", 1) => Some(format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_NUMBER(v{a0})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         if (s_recv != NULL && s_recv->length == s_recv->byte_len) {{ \
                             double d0; memcpy(&d0, &v{a0}, sizeof(d0)); \
                             if (isnan(d0)) d0 = 0.0; \
                             d0 = trunc(d0); \
                             if (d0 >= 0.0 && (size_t)d0 < s_recv->length) {{ \
                                 v{v} = kiln_c99_val_from_double((double)(unsigned char)s_recv->bytes[(size_t)d0]); \
                                 handled = 1; \
                             }} else {{ \
                                 v{v} = KILN_QNAN_BASE; \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                ("charAt", 1) => Some(format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_NUMBER(v{a0})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         if (s_recv != NULL && s_recv->length == s_recv->byte_len) {{ \
                             double d0; memcpy(&d0, &v{a0}, sizeof(d0)); \
                             if (isnan(d0)) d0 = 0.0; \
                             d0 = trunc(d0); \
                             if (d0 >= 0.0 && (size_t)d0 < s_recv->length) {{ \
                                 v{v} = kiln_c99_val_single_char((unsigned char)s_recv->bytes[(size_t)d0]); \
                                 handled = 1; \
                             }} else {{ \
                                 v{v} = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string); \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                )),
                ("trim", 0) => Some(format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{o})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         if (s_recv != NULL && s_recv->length == s_recv->byte_len) {{ \
                             size_t start = 0; \
                             while (start < s_recv->byte_len && ((unsigned char)s_recv->bytes[start] <= ' ' || (unsigned char)s_recv->bytes[start] == 0xa0)) start++; \
                             size_t end = s_recv->byte_len; \
                             while (end > start && ((unsigned char)s_recv->bytes[end - 1] <= ' ' || (unsigned char)s_recv->bytes[end - 1] == 0xa0)) end--; \
                             v{v} = kiln_c99_val_from_ascii(s_recv->bytes + start, end - start); \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    v = v
                )),
                ("slice", 2) => Some(format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_NUMBER(v{a0}) && KILN_VAL_IS_NUMBER(v{a1})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         if (s_recv != NULL && s_recv->length == s_recv->byte_len) {{ \
                             double d0, d1; memcpy(&d0, &v{a0}, sizeof(d0)); memcpy(&d1, &v{a1}, sizeof(d1)); \
                             if (d0 >= 0.0 && d0 == floor(d0) && d1 >= d0 && d1 == floor(d1)) {{ \
                                 size_t start = (size_t)d0; \
                                 size_t end = (size_t)d1 <= s_recv->byte_len ? (size_t)d1 : s_recv->byte_len; \
                                 size_t slen = start < end ? end - start : 0; \
                                 v{v} = kiln_c99_val_from_ascii(s_recv->bytes + start, slen); \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    a1 = args[1].0,
                    v = v
                )),
                _ => None,
            };

            let iterator_intrinsic = if key_str == "next" && argc == 0 {
                Some(format!(
                    "if (!handled && kiln_c99_val_iterator_next_fast(host, v{o}, &v{v}, depth, thrown) == KILN_C99_OK) {{ \
                         handled = 1; \
                     }}",
                    o = object.0,
                    v = v
                ))
            } else {
                None
            };

            // An argument the plan proved a number needs no test: its twin is the operand.
            let math_intrinsic = math_intrinsic.map(|text| {
                args.iter().filter(|a| numbers.contains(a)).fold(text, |text, a| {
                    text.replace(
                        &format!(
                            "if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a}))) {{ double d; memcpy(&d, &v{a}, sizeof(d));",
                            a = a.0
                        ),
                        &format!("{{ double d = d{};", a.0),
                    )
                })
            });
            // A result proven a number is computed as its twin and boxed from it, and the
            // box, when nothing reads it, is not computed; the statement's tail then leaves
            // the twin alone where the intrinsic answered.
            let math_intrinsic = match math_intrinsic {
                Some(text) if typed => {
                    let assign = format!("v{v} = kiln_c99_val_from_double(");
                    let mut out = String::with_capacity(text.len());
                    let mut rest = text.as_str();
                    while let Some(at) = rest.find(&assign) {
                        let expr_start = at + assign.len();
                        let Some(end) = rest[expr_start..].find(");") else {
                            break;
                        };
                        let expr = &rest[expr_start..expr_start + end];
                        out.push_str(&rest[..at]);
                        out.push_str(&format!(
                            "d{v} = {expr}; v{v} = kiln_c99_val_from_double(d{v}); kiln_twin = 1;"
                        ));
                        rest = &rest[expr_start + end + 2..];
                    }
                    out.push_str(rest);
                    Some(out)
                }
                other => other,
            };
            let math_s = math_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
            let push_s = push_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
            let regex_s = regex_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
            let map_s = map_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
            let filter_s = filter_intrinsic
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            let reduce_s = reduce_intrinsic
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            let slice_s = slice_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
            let join_s = join_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
            let split_s = split_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
            let index_of_s = index_of_intrinsic
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            let substring_s = substring_intrinsic
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            let collection_s = collection_intrinsic
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            let json_s = json_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
            let sort_s = sort_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
            let array_extra_s = array_extra_intrinsic
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            let string_extra_s = string_extra_intrinsic
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            let iterator_s = iterator_intrinsic
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            let n = argc.max(1);

            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; \
                kiln_val call_args[{n}] = {{ {init} }}; \
                int handled = 0; int kiln_twin = 0; (void)kiln_twin; {typed_remembered}\
                {math_s}{push_s}{regex_s}{map_s}{filter_s}{reduce_s}{slice_s}{join_s}{split_s}{index_of_s}{substring_s}{collection_s}{json_s}{sort_s}{array_extra_s}{string_extra_s}{iterator_s} \
                if (!handled && site.pic_count > 0 && KILN_VAL_IS_OBJECT(v{o})) {{ \
                    const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(v{o}); \
                    kiln_val cached_fn = kiln_c99_site_cached_callee(&site, obj); \
                    if (cached_fn != 0 && KILN_VAL_IS_CLOSURE(cached_fn)) {{ \
                        kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(cached_fn); \
                        {typed_chain}if (!plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                            kiln_val vreceiver = plain->arrow ? plain->captured_this : v{o}; \
                            if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vreceiver); \
                            v{v} = KILN_VAL_UNDEFINED; \
                            int status; \
                            if (!plain->publishes_frame) {{ kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; {by_name} {{ status = plain->body(&ctx_call, &v{v}, plain->upvalues, &vreceiver, call_args, (size_t){argc}); }} }} \
                            else {{ status = kiln_c99_invoke_closure(host, &v{v}, plain, vreceiver, call_args, (size_t){argc}, depth, thrown, NULL, 0); }} \
                            if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) {{ \
                                if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(v{o}).tag; \
                                if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"CallMethod invocation refused\"; \
                                kiln_c99_note_refusal(\"{diagnostic_site}\", (size_t){pc}, \"{label}\", (size_t)kiln_c99_diagnostic_active_depth); \
                            }} else if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {{ \
                                if (kiln_c99_diagnostic_detail == NULL) {{ \
                                    kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(v{o}).tag; \
                                    kiln_c99_diagnostic_detail = \"CallMethod invocation threw\"; \
                                }} \
                                kiln_c99_note_refusal(\"{diagnostic_site}\", (size_t){pc}, \"{label}\", (size_t)kiln_c99_diagnostic_active_depth); \
                            }} \
                            {check} \
                            handled = 1; \
                        }} \
                    }} else if (cached_fn != 0 && KILN_VAL_IS_METHOD(cached_fn) && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                        kiln_value bself = kiln_val_to_boundary(v{o}); \
                        kiln_value bargs[{n}]; \
                        for (size_t bi = 0; bi < {argc}; ++bi) bargs[bi] = kiln_val_to_boundary(call_args[bi]); \
                        kiln_value bresult = kiln_c99_undefined(); \
                        int status = kiln_c99_invoke_method(host, &bresult, KILN_VAL_METHOD_ID(cached_fn), bself, {argc} > 0 ? bargs : NULL, (size_t){argc}, depth, thrown); \
                        {check} \
                        v{v} = kiln_val_from_boundary(bresult); \
                        handled = 1; \
                    }} \
                }} \
                if (!handled && kiln_c99_cached_method_matches(&site, v{o}) && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                    kiln_value bself = kiln_val_to_boundary(v{o}); \
                    kiln_value bargs[{n}]; \
                    for (size_t bi = 0; bi < {argc}; ++bi) bargs[bi] = kiln_val_to_boundary(call_args[bi]); \
                    kiln_value bresult = kiln_c99_undefined(); \
                    int status = kiln_c99_invoke_method(host, &bresult, site.cached_method, bself, {argc} > 0 ? bargs : NULL, (size_t){argc}, depth, thrown); \
                    {check} \
                    v{v} = kiln_val_from_boundary(bresult); \
                    handled = 1; \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_call_method(host, v{o}, call_args, (size_t){argc}, {symbol}_key_{name}, {key_len}, depth, thrown, &site, \"{diagnostic_site}\", (size_t){pc}, \"{label}\", &v{v}); \
                    {check} \
                }} {typed_sync} }}",
                o = object.0,
                typed_sync = if typed && spec.exits && spec.calls.contains(&vid) {
                    let invariant = lookup_speculation_invariant("method_call_number");
                    format!("/* Speculation exit [{}]: {} */ if (!kiln_twin) {{ if (KILN_UNLIKELY(!KILN_VAL_IS_NUMBER(v{v}))) goto kiln_generic_resume_{v}; memcpy(&d{v}, &v{v}, sizeof(d{v})); }}", invariant.name, invariant.language_rule)
                } else if typed {
                    format!("if (!kiln_twin) memcpy(&d{v}, &v{v}, sizeof(d{v}));")
                } else {
                    String::new()
                }
            )
            .unwrap();
        }
        GraphOp::New { callee, args, .. } => {
            let argc = args.len();
            let mut init = format!("v{}", callee.0);
            for arg in args {
                init.push_str(&format!(", v{}", arg.0));
            }
            let check = status_check(diagnostic_site, pc, label, unwind);
            let fast_new = if argc == 0 {
                format!(
                    "if (KILN_VAL_IS_BUILTIN(v{c})) {{ \
                         uint32_t bid = KILN_VAL_BUILTIN_ID(v{c}); \
                         if (bid == KILN_C99_BUILTIN_MAP) {{ \
                             kiln_collection *col = kiln_c99_collection_alloc_fast(1, 0); \
                             if (col != NULL) {{ v{v} = KILN_VAL_TAG_PTR(KILN_TAG_OBJECT_PREFIX, col); handled = 1; }} \
                         }} else if (bid == KILN_C99_BUILTIN_SET) {{ \
                             kiln_collection *col = kiln_c99_collection_alloc_fast(0, 0); \
                             if (col != NULL) {{ v{v} = KILN_VAL_TAG_PTR(KILN_TAG_OBJECT_PREFIX, col); handled = 1; }} \
                         }} \
                     }} ",
                    c = callee.0,
                    v = v
                )
            } else {
                String::new()
            };
            writeln!(
                output,
                "    {{ int handled = 0; {fast_new}if (!handled) {{ kiln_val call_stack[{n}] = {{ {init} }}; int status = kiln_c99_val_new(host, call_stack[0], {argc} > 0 ? &call_stack[1] : NULL, (size_t){argc}, depth, thrown, &v{v}); {check} }} }}",
                n = argc + 1
            )
            .unwrap();
        }
        GraphOp::NewObject { pairs, .. } => {
            if let Some(scalar_obj) = graph.scalar_objects.get(&vid) {
                let alloc = vid.0;
                let written_indices: HashSet<usize> = pairs
                    .iter()
                    .filter_map(|(k, _)| {
                        resolve_const_string(*k, defs, program)
                            .and_then(|s| scalar_obj.fields.get(s))
                            .copied()
                    })
                    .collect();
                for &field_idx in scalar_obj.fields.values() {
                    if !written_indices.contains(&field_idx) {
                        writeln!(output, "    v{alloc}_f_{field_idx} = KILN_VAL_UNDEFINED;")
                            .unwrap();
                        writeln!(output, "    d{alloc}_f_{field_idx} = 0.0;").unwrap();
                        writeln!(output, "    i{alloc}_f_{field_idx} = 0;").unwrap();
                    }
                }
                for (key, val) in pairs {
                    if let Some(prop_name) = resolve_const_string(*key, defs, program)
                        && let Some(&field_idx) = scalar_obj.fields.get(prop_name)
                    {
                        let val_vid = val.0;
                        writeln!(output, "    v{alloc}_f_{field_idx} = v{val_vid};").unwrap();
                        if numbers.contains(val) {
                            writeln!(output, "    d{alloc}_f_{field_idx} = d{val_vid};").unwrap();
                        } else {
                            writeln!(
                                output,
                                "    memcpy(&d{alloc}_f_{field_idx}, &v{alloc}_f_{field_idx}, sizeof(double));"
                            )
                            .unwrap();
                        }
                        if ints.contains(val) {
                            writeln!(output, "    i{alloc}_f_{field_idx} = i{val_vid};").unwrap();
                        } else {
                            writeln!(output, "    i{alloc}_f_{field_idx} = 0;").unwrap();
                        }
                    }
                }
                writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
                return;
            }
            // Sized once for its pairs, then each pair defined in source order.
            let count = pairs.len();
            let alloc = if count > 0 {
                format!("kiln_c99_new_object_with_capacity(&built, (size_t){count})")
            } else {
                "kiln_c99_new_object(&built)".to_owned()
            };
            let check = status_check(diagnostic_site, pc, label, unwind);
            let mut defines = String::new();
            for (key, value) in pairs {
                defines.push_str(&format!(
                    " status = kiln_c99_define_computed(built, kiln_val_to_boundary(v{}), kiln_val_to_boundary(v{})); {check}",
                    key.0, value.0
                ));
            }
            writeln!(
                output,
                "    {{ kiln_value built; int status = {alloc}; if (status != KILN_C99_OK) return status;{defines} v{v} = kiln_val_from_boundary(built); }}"
            )
            .unwrap();
        }
        GraphOp::MakeFunction { index, .. } => {
            let target = &program.functions[*index as usize];
            let captures = &target.upvalues;
            let defining_receiver = if target.arrow {
                "receiver"
            } else {
                "KILN_VAL_UNDEFINED"
            };
            let mut setup = String::new();
            let captured = if captures.is_empty() {
                "NULL"
            } else {
                setup.push_str(&format!(" kiln_val captured[{}];", captures.len()));
                for (position, upvalue) in captures.iter().enumerate() {
                    match upvalue {
                        Upvalue::ParentLocal(slot) => {
                            setup.push_str(&format!(" captured[{position}] = locals[{slot}];"))
                        }
                        Upvalue::ParentUpvalue(slot) => {
                            setup.push_str(&format!(" captured[{position}] = upvalues[{slot}];"))
                        }
                    }
                }
                "captured"
            };
            let construction = closure_construction(
                &program.functions,
                &program.source,
                frame_stacks,
                strict_functions,
                symbol,
                diagnostic_site,
                *index,
                defining_receiver,
                captured,
                captures.len(),
            );
            let publish = if plan.publishes_frame[*index as usize] {
                ""
            } else {
                " kiln_c99_closure_of(built)->publishes_frame = 0;"
            };
            writeln!(
                output,
                "    {{ kiln_value built;{setup} int status = {construction}; if (status != KILN_C99_OK) return status;{publish} v{v} = kiln_val_from_boundary(built); }}"
            )
            .unwrap();
        }
        GraphOp::NewArray { elements, .. } => {
            let count = elements.len();
            if count == 0 {
                writeln!(
                    output,
                    "    {{ kiln_value built; int status = kiln_c99_new_array_with_capacity(&built, 0, 1); if (status != KILN_C99_OK) return status; v{v} = kiln_val_from_boundary(built); }}"
                )
                .unwrap();
            } else {
                let mut assigns = String::new();
                let mut check_numeric = String::new();
                for (i, elem) in elements.iter().enumerate() {
                    assigns.push_str(&format!(" arr->elements[{i}] = v{};", elem.0));
                    check_numeric.push_str(&format!(" && KILN_VAL_IS_NUMBER(v{})", elem.0));
                }
                writeln!(
                    output,
                    "    {{ kiln_value built; int status = kiln_c99_new_array_with_capacity(&built, {count}, 1{check_numeric}); if (status != KILN_C99_OK) return status; kiln_array *arr = kiln_c99_array_of(built);{assigns} arr->count = {count}; v{v} = kiln_val_from_boundary(built); }}"
                )
                .unwrap();
            }
        }
        // Property and element access call the value-shaped helpers directly: the same
        // functions the stack-shaped entry points wrap, without the one-element arrays.
        GraphOp::GetProperty { object, name, .. } => {
            if let Some(&root) = graph.scalar_alias_map.get(object)
                && let Some(scalar_obj) = graph.scalar_objects.get(&root)
            {
                let alloc = root.0;
                if let Some(&field_idx) = scalar_obj.field_by_name_index.get(name) {
                    writeln!(output, "    v{v} = v{alloc}_f_{field_idx};").unwrap();
                    if typed {
                        writeln!(output, "    d{v} = d{alloc}_f_{field_idx};").unwrap();
                    }
                    if typed_int {
                        writeln!(output, "    i{v} = i{alloc}_f_{field_idx};").unwrap();
                    }
                } else {
                    writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
                    if typed {
                        writeln!(output, "    d{v} = 0.0;").unwrap();
                    }
                    if typed_int {
                        writeln!(output, "    i{v} = 0;").unwrap();
                    }
                }
                return;
            }
            let key_str = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.as_str(),
                _ => "",
            };
            let key_len = key_str.len();
            let check = status_check(diagnostic_site, pc, label, unwind);
            let length_check = if key_str == "length" && spec.arrays.contains(object) {
                format!(
                    "{{ v{v} = kiln_c99_val_from_double((double)((const kiln_array *)KILN_VAL_PTR(v{o}))->count); handled = 1; }} ",
                    o = object.0
                )
            } else if key_str == "length" {
                format!(
                    "if (KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (KILN_LIKELY(site.has_cached_method == 1 || kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY)) {{ \
                             site.has_cached_method = 1; \
                             v{v} = kiln_c99_val_from_double((double)((const kiln_array *)ptr)->count); \
                             handled = 1; \
                         }} \
                     }} else if (KILN_VAL_IS_STRING(v{o})) {{ \
                         const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         v{v} = kiln_c99_val_from_double((double)str->length); \
                         handled = 1; \
                     }} ",
                    o = object.0,
                    v = v
                )
            } else {
                String::new()
            };
            let size_check = if key_str == "size" {
                format!(
                    "if ((v{o} & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX || (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {{ \
                             const kiln_collection *col = (const kiln_collection *)ptr; \
                             if (!col->is_weak) {{ \
                                 v{v} = kiln_c99_val_from_double((double)col->live); \
                                 handled = 1; \
                             }} \
                         }} \
                     }} ",
                    o = object.0,
                    v = v
                )
            } else {
                String::new()
            };
            // A read of `this`: the function's receiver-shape cache answers first, one indexed
            // load and no compare, because the shape was matched once at entry. The boxed
            // clone, whose slow path is the one that learns, fills the row afterwards.
            // A read of `this`: the function's receiver-shape cache answers with one indexed
            // load when the receiver's shape has a row, which was filled whole from the shape.
            let (receiver_tier, receiver_learn) = match (this_site, self_index) {
                (Some(i), Some(_)) => (
                    format!(
                        "if (recv_row != NULL) {{ v{v} = recv_obj->slots[recv_row[{i}]]; handled = 1; }} "
                    ),
                    String::new(),
                ),
                _ => (String::new(), String::new()),
            };
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; \
                int handled = 0; \
                {receiver_tier}{length_check}{size_check}\
                if (!handled && KILN_VAL_IS_OBJECT(v{o})) {{ \
                    const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(v{o}); \
                    const kiln_shape *shape = obj->shape; \
                    uint32_t slot = UINT32_MAX; \
                    if (KILN_LIKELY(shape != NULL)) {{ \
                        if (shape == site.entries[0].shape) slot = site.entries[0].slot; \
                        else if (shape == site.entries[1].shape) slot = site.entries[1].slot; \
                        else if (shape == site.entries[2].shape) slot = site.entries[2].slot; \
                        else if (shape == site.entries[3].shape) slot = site.entries[3].slot; \
                    }} \
                    if (slot != UINT32_MAX && KILN_LIKELY(slot < obj->slot_capacity)) {{ \
                        v{v} = obj->slots[slot]; \
                        handled = 1; \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_read_property(host, v{o}, {symbol}_key_{name}, {key_len}, depth, thrown, &site, &v{v}); \
                    if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_property_refusal(kiln_val_to_boundary(v{o})); \
                    {check} \
                }}{receiver_learn} }}\
                {typed_sync}",
                o = object.0,
                typed_sync = if typed { format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));") } else { String::new() }
            )
            .unwrap();
        }
        GraphOp::SetProperty {
            object,
            name,
            value,
            ..
        }
        | GraphOp::SetPropertyStrict {
            object,
            name,
            value,
            ..
        } => {
            if let Some(&root) = graph.scalar_alias_map.get(object)
                && let Some(scalar_obj) = graph.scalar_objects.get(&root)
            {
                let alloc = root.0;
                let val = value.0;
                if let Some(&field_idx) = scalar_obj.field_by_name_index.get(name) {
                    writeln!(output, "    v{alloc}_f_{field_idx} = v{val};").unwrap();
                    if numbers.contains(value) {
                        writeln!(output, "    d{alloc}_f_{field_idx} = d{val};").unwrap();
                    } else {
                        writeln!(
                            output,
                            "    memcpy(&d{alloc}_f_{field_idx}, &v{alloc}_f_{field_idx}, sizeof(double));"
                        )
                        .unwrap();
                    }
                    if ints.contains(value) {
                        writeln!(output, "    i{alloc}_f_{field_idx} = i{val};").unwrap();
                    } else {
                        writeln!(output, "    i{alloc}_f_{field_idx} = 0;").unwrap();
                    }
                }
                writeln!(output, "    v{v} = v{val};").unwrap();
                if typed {
                    writeln!(output, "    d{v} = d{val};").unwrap();
                }
                if typed_int {
                    writeln!(output, "    i{v} = i{val};").unwrap();
                }
                return;
            }
            let key_str = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.as_str(),
                _ => "",
            };
            let key_len = key_str.len();
            let strict = matches!(op, GraphOp::SetPropertyStrict { strict: true, .. }) as u8;
            let check = status_check(diagnostic_site, pc, label, unwind);
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; \
                int handled = 0; \
                if (KILN_VAL_IS_OBJECT(v{o})) {{ \
                    kiln_object *obj = (kiln_object *)KILN_VAL_PTR(v{o}); \
                    const kiln_shape *shape = obj->shape; \
                    uint32_t slot = UINT32_MAX; \
                    if (KILN_LIKELY(shape != NULL)) {{ \
                        if (shape == site.entries[0].shape && site.entries[0].transition_to == NULL) slot = site.entries[0].slot; \
                        else if (shape == site.entries[1].shape && site.entries[1].transition_to == NULL) slot = site.entries[1].slot; \
                        else if (shape == site.entries[2].shape && site.entries[2].transition_to == NULL) slot = site.entries[2].slot; \
                        else if (shape == site.entries[3].shape && site.entries[3].transition_to == NULL) slot = site.entries[3].slot; \
                    }} \
                    if (slot != UINT32_MAX && KILN_LIKELY(slot < obj->slot_capacity && !obj->frozen)) {{ \
                        obj->slots[slot] = v{val}; \
                        v{v} = v{val}; \
                        handled = 1; \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_write_property(host, v{o}, {symbol}_key_{name}, {key_len}, v{val}, {strict}, depth, thrown, &site); \
                    {check} \
                    v{v} = v{val}; \
                }} }}\
                {typed_sync}",
                o = object.0,
                val = value.0,
                typed_sync = if typed { format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));") } else { String::new() }
            )
            .unwrap();
        }
        GraphOp::GetComputed { object, key, .. } => {
            if let Some(&root) = graph.scalar_alias_map.get(object)
                && let Some(scalar_obj) = graph.scalar_objects.get(&root)
                && let Some(key_str) = resolve_const_string(*key, defs, program)
            {
                let alloc = root.0;
                if let Some(&field_idx) = scalar_obj.fields.get(key_str) {
                    writeln!(output, "    v{v} = v{alloc}_f_{field_idx};").unwrap();
                    if typed {
                        writeln!(output, "    d{v} = d{alloc}_f_{field_idx};").unwrap();
                    }
                    if typed_int {
                        writeln!(output, "    i{v} = i{alloc}_f_{field_idx};").unwrap();
                    }
                } else {
                    writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
                    if typed {
                        writeln!(output, "    d{v} = 0.0;").unwrap();
                    }
                    if typed_int {
                        writeln!(output, "    i{v} = 0;").unwrap();
                    }
                }
                return;
            }
            let check = status_check(diagnostic_site, pc, label, unwind);
            // The object an array by the entry guard: no tag or kind to test, and an index
            // proven an integer is the element's index as it is.
            let array_tier = if spec.arrays.contains(object) && ints.contains(key) {
                format!(
                    "{{ const kiln_array *arr = (const kiln_array *)KILN_VAL_PTR(v{o}); if (KILN_LIKELY(i{k} >= 0 && (uint64_t)i{k} < arr->count && arr->elements[i{k}] != KILN_VAL_HOLE)) {{ v{v} = arr->elements[i{k}]; handled = 1; }} }} ",
                    o = object.0,
                    k = key.0
                )
            } else if spec.arrays.contains(object) {
                format!(
                    "if (KILN_VAL_IS_NUMBER(v{k})) {{ const kiln_array *arr = (const kiln_array *)KILN_VAL_PTR(v{o}); double idx; memcpy(&idx, &v{k}, sizeof(idx)); size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && arr->elements[slot] != KILN_VAL_HOLE)) {{ v{v} = arr->elements[slot]; handled = 1; }} }} ",
                    o = object.0,
                    k = key.0
                )
            } else {
                String::new()
            };
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; \
                int handled = 0; {array_tier}\
                if (!handled && KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_NUMBER(v{k}))) {{ \
                    void *ptr = KILN_VAL_PTR(v{o}); \
                    if (site.has_cached_method == 1 || kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                        site.has_cached_method = 1; \
                        double idx; \
                        memcpy(&idx, &v{k}, sizeof(idx)); \
                        size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; \
                        const kiln_array *arr = (const kiln_array *)ptr; \
                        if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && arr->elements[slot] != KILN_VAL_HOLE)) {{ \
                            v{v} = arr->elements[slot]; \
                            handled = 1; \
                        }} \
                    }} \
                }} else if (KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_NUMBER(v{k})) {{ \
                    double idx; \
                    memcpy(&idx, &v{k}, sizeof(idx)); \
                    size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; \
                    const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                    if (KILN_LIKELY(str != NULL && idx >= 0.0 && idx == (double)slot && slot < str->length && str->length == str->byte_len)) {{ \
                        v{v} = kiln_c99_val_single_char((unsigned char)str->bytes[slot]); \
                        handled = 1; \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_read_element(host, v{o}, v{k}, depth, thrown, &v{v}); \
                    if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_computed_refusal(kiln_val_to_boundary(v{o}), kiln_val_to_boundary(v{k})); \
                    {check} \
                }} }}\
                {typed_sync}",
                o = object.0,
                k = key.0,
                typed_sync = if typed { format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));") } else { String::new() }
            )
            .unwrap();
        }
        GraphOp::SetComputed {
            object, key, value, ..
        }
        | GraphOp::SetComputedStrict {
            object, key, value, ..
        } => {
            if let Some(&root) = graph.scalar_alias_map.get(object)
                && let Some(scalar_obj) = graph.scalar_objects.get(&root)
                && let Some(key_str) = resolve_const_string(*key, defs, program)
            {
                let alloc = root.0;
                let val = value.0;
                if let Some(&field_idx) = scalar_obj.fields.get(key_str) {
                    writeln!(output, "    v{alloc}_f_{field_idx} = v{val};").unwrap();
                    if numbers.contains(value) {
                        writeln!(output, "    d{alloc}_f_{field_idx} = d{val};").unwrap();
                    } else {
                        writeln!(
                            output,
                            "    memcpy(&d{alloc}_f_{field_idx}, &v{alloc}_f_{field_idx}, sizeof(double));"
                        )
                        .unwrap();
                    }
                    if ints.contains(value) {
                        writeln!(output, "    i{alloc}_f_{field_idx} = i{val};").unwrap();
                    } else {
                        writeln!(output, "    i{alloc}_f_{field_idx} = 0;").unwrap();
                    }
                }
                writeln!(output, "    v{v} = v{val};").unwrap();
                if typed {
                    writeln!(output, "    d{v} = d{val};").unwrap();
                }
                if typed_int {
                    writeln!(output, "    i{v} = i{val};").unwrap();
                }
                return;
            }
            let strict = matches!(op, GraphOp::SetComputedStrict { strict: true, .. }) as u8;
            let check = status_check(diagnostic_site, pc, label, unwind);
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; \
                int handled = 0; \
                if (KILN_LIKELY((v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_NUMBER(v{k}))) {{ \
                    void *ptr = KILN_VAL_PTR(v{o}); \
                    if (site.has_cached_method == 1 || kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                        site.has_cached_method = 1; \
                        double idx; \
                        memcpy(&idx, &v{k}, sizeof(idx)); \
                        size_t slot = (idx >= 0.0 && idx <= 4294967294.0) ? (size_t)idx : (size_t)-1; \
                        kiln_array *arr = (kiln_array *)ptr; \
                        if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && !arr->frozen)) {{ \
                            if (arr->is_numeric && !KILN_VAL_IS_NUMBER(v{val})) arr->is_numeric = 0; \
                            arr->elements[slot] = v{val}; \
                            v{v} = v{val}; \
                            handled = 1; \
                        }} \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_write_element(host, v{o}, v{k}, v{val}, {strict}, depth, thrown); \
                    {check} \
                    v{v} = v{val}; \
                }} }}\
                {typed_sync}",
                o = object.0,
                k = key.0,
                val = value.0,
                typed_sync = if typed { format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));") } else { String::new() }
            )
            .unwrap();
        }
        GraphOp::Phi { .. } => {
            // A phi is assigned on the edge into its block, by the predecessor's jump or
            // branch, or by the landing that comes in from the dispatch switch.
        }
    }
}
