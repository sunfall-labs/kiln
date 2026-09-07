mod arithmetic;
mod call_method;
mod properties;

use super::clones::typed_fail;
use super::method_candidates;
use super::{EntryKind, Speculation};
use crate::direct::{CallPlan, direct_callable};
use crate::graph::{
    FunctionGraph, GraphOp, InductionVariable, ValueId, as_safe_integer, resolve_const_string,
};
use crate::prelude::lookup_speculation_invariant;
use crate::tail::{TailContext, emit_tail_instruction};
use crate::{
    BuiltinLoad, builtin_load, closure_construction, compact_diagnostic_site, diagnostic_source,
};
use kiln_ir::Instruction;
use kiln_ir::Upvalue;
use kiln_ir::{BinaryOp, Constant, FunctionProto, Program, UnaryOp};
use std::collections::HashSet;
use std::fmt::Write;

/// The references an operation's emission shares with every other operation in the same body:
/// everything `emit_graph_op` receives except the operation itself and the value it defines.
/// An arm large enough to live in its own file takes this instead of the same twenty parameters
/// spelled out again. Not every arm moved out so far reads every field -- `array_slots`,
/// `proto`, `frame_stacks`, `strict_functions` and `kind` are here for the arms still inline in
/// `emit_graph_op` and for the ones a later split will move next.
#[allow(dead_code)]
pub(super) struct OpContext<'a> {
    pub graph: &'a FunctionGraph,
    pub program: &'a Program,
    pub symbol: &'a str,
    pub diagnostic_site: &'a str,
    pub scalar_proto: bool,
    pub numbers: &'a HashSet<ValueId>,
    pub ints: &'a HashSet<ValueId>,
    pub defs: &'a std::collections::HashMap<ValueId, &'a GraphOp>,
    pub ivs: &'a std::collections::HashMap<ValueId, InductionVariable>,
    pub array_slots: &'a HashSet<u32>,
    pub proto: &'a FunctionProto,
    pub frame_stacks: &'a [usize],
    pub strict_functions: &'a [bool],
    pub origins: &'a std::collections::HashMap<ValueId, crate::graph::Origin>,
    pub unwind: &'a str,
    pub plan: &'a CallPlan,
    pub kind: EntryKind,
    pub self_index: Option<u32>,
    pub this_site: Option<usize>,
    /// An earlier read of the same `this` slot in this block, with nothing between that could
    /// have changed it: on the row path this read is a copy of it.
    pub this_copy: Option<ValueId>,
    pub spec: &'a Speculation,
    /// Emitting the parallel clone of a typed entry; see `emit_graph::parallel_clone`.
    pub parallel: bool,
}

/// One status check after a runtime call. A throw unwinds; a refusal is attributed to this site
/// and reported through `kiln_refusal`, which carries the diagnostic in the result; anything
/// else returns as it is.
pub(super) fn status_check(
    diagnostic_site: &str,
    pc: u32,
    operation: &str,
    unwind: &str,
) -> String {
    if compact_diagnostic_site(diagnostic_site) {
        // Body assembly detects this marker to materialize the shared refusal label.
        return format!("KU_STATUS({pc}, \"{operation}\", {unwind}); /* goto kiln_refusal; */");
    }
    let diagnostic_source = diagnostic_source(diagnostic_site);
    format!(
        "if (status == KILN_C99_THROWN) {unwind} if (status == KILN_C99_CONTROL_ERROR) {{ kiln_c99_note_refusal({diagnostic_source}, (size_t){pc}, \"{operation}\", (size_t)kiln_c99_diagnostic_active_depth); goto kiln_refusal; }} if (status != KILN_C99_OK) return status;"
    )
}

/// The numeric fast path of a binary operator on two unboxed doubles `a` and `b`, as the C
/// expression producing a `kiln_val`, or `None` for an operator with no numeric form.
/// The arithmetic of a binary operation on two doubles `a` and `b`, as a double.
pub(super) fn binary_raw_body(op: BinaryOp) -> Option<&'static str> {
    Some(match op {
        BinaryOp::Add => "a + b",
        BinaryOp::Subtract => "a - b",
        BinaryOp::Multiply => "a * b",
        BinaryOp::Divide => "a / b",
        BinaryOp::Exponentiate => "pow(a, b)",
        _ => return None,
    })
}

pub(super) fn binary_fast_body(op: BinaryOp) -> Option<&'static str> {
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

pub(super) fn binary_code(op: BinaryOp) -> u16 {
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
pub(super) fn unary_fast_body(op: UnaryOp) -> Option<&'static str> {
    Some(match op {
        UnaryOp::Plus | UnaryOp::ToNumeric => "value",
        UnaryOp::Negate => "kiln_c99_val_from_double(-a)",
        UnaryOp::BitwiseNot => "kiln_c99_val_unary_fast(5, value)",
        UnaryOp::Increment => "kiln_c99_val_from_double(a + 1.0)",
        UnaryOp::Decrement => "kiln_c99_val_from_double(a - 1.0)",
        UnaryOp::Not | UnaryOp::TypeOf | UnaryOp::Void => return None,
    })
}

pub(super) fn unary_code(op: UnaryOp) -> u16 {
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
    this_copy: Option<ValueId>,
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
    let ctx = OpContext {
        graph,
        program,
        symbol,
        diagnostic_site,
        scalar_proto,
        numbers,
        ints,
        defs,
        ivs,
        array_slots,
        proto,
        frame_stacks,
        strict_functions,
        origins,
        unwind,
        parallel: super::parallel_clone(),
        plan,
        kind,
        self_index,
        this_site,
        this_copy,
        spec,
    };
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
            if crate::compact_diagnostic_site(diagnostic_site) {
                writeln!(
                    output,
                    "    KU_LOAD_CELL(v{v}, locals[{slot}], {thrown}, {pc}, \"{label}\", {unwind}); /* goto kiln_refusal; */"
                )
                .unwrap();
            } else {
                let check = status_check(diagnostic_site, pc, label, unwind);
                writeln!(
                    output,
                    "    {{ const kiln_val *cell = (const kiln_val *)KILN_VAL_PTR(locals[{slot}]); int status = kiln_c99_val_check_tdz(*cell, {thrown}); {check} v{v} = *cell; }}"
                )
                .unwrap();
            }
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
            if crate::compact_diagnostic_site(diagnostic_site) {
                writeln!(
                    output,
                    "    KU_LOAD_CELL(v{v}, upvalues[{index}], thrown, {pc}, \"{label}\", {unwind}); /* goto kiln_refusal; */"
                )
                .unwrap();
            } else {
                let check = status_check(diagnostic_site, pc, label, unwind);
                writeln!(
                    output,
                    "    {{ const kiln_val *cell = (const kiln_val *)KILN_VAL_PTR(upvalues[{index}]); int status = kiln_c99_val_check_tdz(*cell, thrown); {check} v{v} = *cell; }}"
                )
                .unwrap();
            }
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
            pushes: 1,
        } if matches!(
            instruction,
            Instruction::SpreadObject | Instruction::DefineNamedProperty
        ) && graph
            .scalar_alias_map
            .get(&operands[0])
            .is_some_and(|root| graph.spread_sinks.contains_key(root)) =>
        {
            // A sunk literal's spread or define: the fast clone's graph alone carries it as a
            // scalar object (`FunctionGraph::spread_sinks`).
            emit_sunk_bridge(output, vid, instruction, operands, &ctx);
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
                    compact: plan.compact,
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
            arithmetic::emit_unary_int(output, vid, op, value, &ctx)
        }
        GraphOp::Unary { op, value } if typed => {
            arithmetic::emit_unary_typed(output, vid, op, value, &ctx)
        }
        GraphOp::Unary { op, value } => arithmetic::emit_unary_boxed(output, vid, op, value, &ctx),
        GraphOp::Binary { op, left, right }
            if ints.contains(left)
                && ints.contains(right)
                && !matches!(op, BinaryOp::In | BinaryOp::InstanceOf) =>
        {
            arithmetic::emit_binary_ints(output, vid, op, left, right, &ctx)
        }
        GraphOp::Binary { op, left, right }
            if numbers.contains(left)
                && numbers.contains(right)
                && !matches!(op, BinaryOp::In | BinaryOp::InstanceOf) =>
        {
            arithmetic::emit_binary_numbers(output, vid, op, left, right, &ctx)
        }
        // `in` and `instanceof` are not arithmetic: they go inline against their own prelude
        // entry points and never to the binary helper, which answers something else for their
        // codes.
        GraphOp::Binary {
            op: BinaryOp::In,
            left,
            right,
        } => arithmetic::emit_binary_in(output, vid, left, right, &ctx),
        GraphOp::Binary {
            op: BinaryOp::InstanceOf,
            left,
            right,
        } => arithmetic::emit_binary_instance_of(output, vid, left, right, &ctx),
        GraphOp::Binary { op, left, right } => {
            arithmetic::emit_binary_boxed(output, vid, op, left, right, &ctx)
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
            if plan.compact {
                let check = status_check(diagnostic_site, pc, label, unwind);
                let sync_number = typed
                    .then(|| format!(" memcpy(&d{v}, &v{v}, sizeof(d{v}));"))
                    .unwrap_or_default();
                writeln!(
                    output,
                    "    {{ kiln_val call_stack[{n}] = {{ {init} }}; const kiln_val *call_args = {argc} > 0 ? &call_stack[1] : NULL; int status = kiln_c99_val_call(host, call_stack[0], call_args, (size_t){argc}, depth, thrown, {diagnostic_source}, (size_t){pc}, \"{label}\", &v{v}); {check}{sync_number} }}",
                    n = argc + 1,
                    diagnostic_source = diagnostic_source(diagnostic_site),
                )
                .unwrap();
                return;
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
        } => call_method::emit_call_method(output, vid, object, name, args, &ctx),
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
            if plan.compact && count > 1 {
                let values = pairs
                    .iter()
                    .flat_map(|(key, value)| [key, value])
                    .map(|value| format!("v{}", value.0))
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(output, "    {{ kiln_val pairs[{pair_count}] = {{ {values} }}; kiln_value built; int status = {alloc}; if (status != KILN_C99_OK) return status; for (size_t pair = 0; pair < (size_t){count}; ++pair) {{ status = kiln_c99_define_computed(built, kiln_val_to_boundary(pairs[pair * 2]), kiln_val_to_boundary(pairs[pair * 2 + 1])); {check} }} v{v} = kiln_val_from_boundary(built); }}", pair_count = count * 2).unwrap();
            } else {
                let mut defines = String::new();
                for (key, value) in pairs {
                    defines.push_str(&format!(
                        " status = kiln_c99_define_computed(built, kiln_val_to_boundary(v{}), kiln_val_to_boundary(v{})); {check}",
                        key.0, value.0
                    ));
                }
                writeln!(output, "    {{ kiln_value built; int status = {alloc}; if (status != KILN_C99_OK) return status;{defines} v{v} = kiln_val_from_boundary(built); }}").unwrap();
            }
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
                        Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. } => {
                            setup.push_str(&format!(" captured[{position}] = locals[{slot}];"))
                        }
                        Upvalue::ParentUpvalue(slot)
                        | Upvalue::NamedParentUpvalue { index: slot, .. } => {
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
                let mut check_numeric = String::new();
                for elem in elements {
                    check_numeric.push_str(&format!(" && KILN_VAL_IS_NUMBER(v{})", elem.0));
                }
                if plan.compact && count > 1 {
                    let values = elements
                        .iter()
                        .map(|elem| format!("v{}", elem.0))
                        .collect::<Vec<_>>()
                        .join(", ");
                    writeln!(output, "    {{ kiln_val elements[{count}] = {{ {values} }}; kiln_value built; int status = kiln_c99_new_array_with_capacity(&built, {count}, 1{check_numeric}); if (status != KILN_C99_OK) return status; kiln_array *arr = kiln_c99_array_of(built); memcpy(arr->elements, elements, sizeof(elements)); arr->count = {count}; v{v} = kiln_val_from_boundary(built); }}").unwrap();
                } else {
                    let mut assigns = String::new();
                    for (i, elem) in elements.iter().enumerate() {
                        assigns.push_str(&format!(" arr->elements[{i}] = v{};", elem.0));
                    }
                    writeln!(output, "    {{ kiln_value built; int status = kiln_c99_new_array_with_capacity(&built, {count}, 1{check_numeric}); if (status != KILN_C99_OK) return status; kiln_array *arr = kiln_c99_array_of(built);{assigns} arr->count = {count}; v{v} = kiln_val_from_boundary(built); }}").unwrap();
                }
            }
        }
        // Property and element access call the value-shaped helpers directly: the same
        // functions the stack-shaped entry points wrap, without the one-element arrays.
        GraphOp::GetProperty { object, name, .. } => {
            properties::emit_get_property(output, vid, object, name, &ctx)
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
        } => properties::emit_set_property(output, vid, op, object, name, value, &ctx),
        GraphOp::GetComputed { object, key, .. } => {
            properties::emit_get_computed(output, vid, object, key, &ctx)
        }
        GraphOp::SetComputed {
            object, key, value, ..
        }
        | GraphOp::SetComputedStrict {
            object, key, value, ..
        } => properties::emit_set_computed(output, vid, op, object, key, value, &ctx),
        GraphOp::Phi { .. } => {
            // A phi is assigned on the edge into its block, by the predecessor's jump or
            // branch, or by the landing that comes in from the dispatch switch.
        }
    }
}

/// The text that rebuilds a sunk literal on the heap, when the fast clone leaves for the generic
/// clone after sinking it: an object with every named field from its scalar twins, stored into
/// the allocation's value and every alias of it. The generic clone then reads a real object.
///
/// Only the named fields exist, in name order: the literal never escapes, so no other key,
/// no order, and no presence is observable -- the same argument that let it be scalar.
pub(super) fn materialize_sunk(
    graph: &FunctionGraph,
    symbol: &str,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    array_slots: &HashSet<u32>,
    root: ValueId,
) -> String {
    let scalar_obj = &graph.scalar_objects[&root];
    let alloc = root.0;
    let mut constant_of_field: std::collections::HashMap<usize, u32> =
        std::collections::HashMap::new();
    for (&constant, &field_idx) in &scalar_obj.field_by_name_index {
        let slot = constant_of_field.entry(field_idx).or_insert(constant);
        *slot = (*slot).min(constant);
    }
    let mut text = format!(
        "{{ kiln_value kiln_mat; int kiln_mat_status = kiln_c99_new_object_with_capacity(&kiln_mat, (size_t){}); if (kiln_mat_status != KILN_C99_OK) return kiln_mat_status;",
        scalar_obj.fields.len().max(1)
    );
    for &field_idx in scalar_obj.fields.values() {
        let Some(constant) = constant_of_field.get(&field_idx) else {
            continue;
        };
        text.push_str(&format!(
            " kiln_mat_status = kiln_c99_define_computed(kiln_mat, kiln_val_to_boundary(KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &{symbol}_string_{constant})), kiln_val_to_boundary(v{alloc}_f_{field_idx})); if (kiln_mat_status != KILN_C99_OK) return kiln_mat_status;"
        ));
    }
    text.push_str(&format!(" v{alloc} = kiln_val_from_boundary(kiln_mat);"));
    let mut aliases: Vec<ValueId> = graph
        .spread_alias_map
        .iter()
        .filter(|(_, r)| **r == root)
        .map(|(a, _)| *a)
        .collect();
    aliases.sort();
    for alias in aliases {
        text.push_str(&format!(" v{} = v{alloc};", alias.0));
        if let Some(GraphOp::StoreLocal { slot, .. }) = defs.get(&alias)
            && array_slots.contains(slot)
        {
            text.push_str(&format!(" locals[{slot}] = v{alloc};"));
        }
    }
    text.push_str(" }");
    text
}

/// Every sunk literal of the fast clone's graph rebuilt, for an exit into the generic clone
/// from anywhere after the entry guard. A literal whose allocation the exit precedes is rebuilt
/// too, from its twins' current contents, and the generic clone's allocation then replaces it;
/// the field twins are the literal's state at every point, so nothing is lost either way.
pub(super) fn materialize_all_sunk(
    graph: &FunctionGraph,
    symbol: &str,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    array_slots: &HashSet<u32>,
) -> String {
    let mut roots: Vec<ValueId> = graph
        .spread_sinks
        .keys()
        .copied()
        .filter(|root| graph.scalar_objects.contains_key(root))
        .collect();
    roots.sort();
    roots
        .into_iter()
        .map(|root| materialize_sunk(graph, symbol, defs, array_slots, root))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One field of a sunk literal takes a value: the box, and the twins the value has.
fn emit_sunk_field_store(
    output: &mut String,
    alloc: u32,
    field_idx: usize,
    value: ValueId,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
) {
    let val = value.0;
    writeln!(output, "    v{alloc}_f_{field_idx} = v{val};").unwrap();
    if numbers.contains(&value) {
        writeln!(output, "    d{alloc}_f_{field_idx} = d{val};").unwrap();
    } else {
        writeln!(
            output,
            "    memcpy(&d{alloc}_f_{field_idx}, &v{alloc}_f_{field_idx}, sizeof(double));"
        )
        .unwrap();
    }
    if ints.contains(&value) {
        writeln!(output, "    i{alloc}_f_{field_idx} = i{val};").unwrap();
    } else {
        writeln!(output, "    i{alloc}_f_{field_idx} = 0;").unwrap();
    }
}

/// A spread or a constant-key define onto a sunk literal, in the fast clone.
///
/// The define is a field store. The spread copies the source's slot for each named field, under
/// the guard `kiln_c99_spread_object_fast` (spread.c) copies under: a shaped object without
/// symbol keys, accessors, elements or a dictionary. The slot of each field in the source's
/// shape is learnt once per shape at the site. A source that fails the guard is left to the
/// generic clone, which spreads it itself into the literal rebuilt by `materialize_sunk`.
fn emit_sunk_bridge(
    output: &mut String,
    vid: ValueId,
    instruction: &Instruction,
    operands: &[ValueId],
    ctx: &OpContext,
) {
    let v = vid.0;
    let graph = ctx.graph;
    let root = graph.scalar_alias_map[&operands[0]];
    let scalar_obj = &graph.scalar_objects[&root];
    let alloc = root.0;
    match instruction {
        Instruction::DefineNamedProperty => {
            let key = resolve_const_string(operands[1], ctx.defs, ctx.program)
                .expect("a sunk define's key is a constant string: the escape analysis checked");
            if let Some(&field_idx) = scalar_obj.fields.get(key) {
                emit_sunk_field_store(output, alloc, field_idx, operands[2], ctx.numbers, ctx.ints);
            }
            writeln!(output, "    v{v} = KILN_VAL_UNDEFINED;").unwrap();
        }
        Instruction::SpreadObject => {
            let source = operands[1].0;
            let invariant = lookup_speculation_invariant("spread_source_shaped");
            let count = scalar_obj.fields.len().max(1);
            let materialize = materialize_all_sunk(graph, ctx.symbol, ctx.defs, ctx.array_slots);
            writeln!(
                output,
                "    /* Speculation exit [{}]: {} */",
                invariant.name, invariant.language_rule
            )
            .unwrap();
            writeln!(
                output,
                "    {{ static const kiln_shape *kiln_sp_shape_{v} = NULL; static uint32_t kiln_sp_slot_{v}[{count}]; \
                 kiln_object *kiln_sp_src = KILN_VAL_IS_OBJECT(v{source}) ? (kiln_object *)KILN_VAL_PTR(v{source}) : NULL; \
                 if (KILN_UNLIKELY(kiln_sp_src == NULL || kiln_sp_src->shape == NULL || kiln_sp_src->properties != NULL || kiln_sp_src->elements != NULL || kiln_sp_src->accessor_count != 0 || kiln_sp_src->shape->has_symbol_key || kiln_c99_gc_kind_of_ptr(kiln_sp_src) != KILN_GC_OBJECT)) {{ {materialize} goto kiln_generic_resume_{v}; }}"
            )
            .unwrap();
            let mut relearn = String::new();
            for (name, &field_idx) in &scalar_obj.fields {
                relearn.push_str(&format!(
                    " kiln_sp_slot_{v}[{field_idx}] = kiln_c99_shape_slot_named(kiln_sp_src->shape, {}, {});",
                    crate::string::c_string(name),
                    name.len()
                ));
            }
            writeln!(
                output,
                "    if (KILN_UNLIKELY(kiln_sp_src->shape != kiln_sp_shape_{v})) {{ kiln_sp_shape_{v} = kiln_sp_src->shape;{relearn} }}"
            )
            .unwrap();
            for &field_idx in scalar_obj.fields.values() {
                writeln!(
                    output,
                    "    if (kiln_sp_slot_{v}[{field_idx}] != UINT32_MAX) {{ v{alloc}_f_{field_idx} = kiln_sp_src->slots[kiln_sp_slot_{v}[{field_idx}]]; memcpy(&d{alloc}_f_{field_idx}, &v{alloc}_f_{field_idx}, sizeof(double)); i{alloc}_f_{field_idx} = 0; }}"
                )
                .unwrap();
            }
            writeln!(output, "    v{v} = KILN_VAL_UNDEFINED; }}").unwrap();
        }
        _ => unreachable!("only a spread or a named define targets a sunk literal"),
    }
}
