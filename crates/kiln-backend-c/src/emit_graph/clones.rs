use super::EntryKind;
use super::emit_function_from_graph_with_profile;
use crate::direct::CallPlan;
use crate::graph::{FunctionGraph, GraphOp, Proven, Terminator, ValueId};
use kiln_ir::{BinaryOp, Constant, FunctionProto, Program};
use std::collections::HashSet;

/// Whether an operation may appear in a restartable body: it has a pure fast path, and its
/// slow path -- the one the fast clone replaces with a restart -- is where every effect lives.
fn restartable(
    op: &GraphOp,
    plan: &CallPlan,
    vid: ValueId,
    graph: &FunctionGraph,
    program: &Program,
) -> bool {
    match op {
        GraphOp::Const { .. }
        | GraphOp::LoadLocal(_)
        | GraphOp::LoadThis
        | GraphOp::LoadBuiltin(_)
        | GraphOp::Phi { .. }
        | GraphOp::StoreLocal { .. }
        | GraphOp::CheckTdz { .. }
        | GraphOp::GetProperty { .. }
        | GraphOp::GetComputed { .. }
        | GraphOp::Unary { .. }
        | GraphOp::IsReturnSignal(_) => true,
        GraphOp::LoadUpvalue(_) | GraphOp::LoadCell(_) => plan.unchecked.contains(&vid),
        GraphOp::Binary { op, .. } => !matches!(op, BinaryOp::In | BinaryOp::InstanceOf),
        // A method call is restartable only as an intrinsic on the `Math` built-in: the call
        // that would otherwise run is a user function, which must not run twice.
        GraphOp::CallMethod { object, name, .. } => {
            let on_math = graph
                .blocks
                .iter()
                .flat_map(|b| b.operations.iter())
                .any(|(v, op)| {
                    v == object && matches!(op, GraphOp::LoadBuiltin(kiln_ir::Builtin::Math))
                });
            let key = match program.constants.get(*name as usize) {
                Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
                _ => String::new(),
            };
            on_math
                && matches!(
                    key.as_str(),
                    "sqrt" | "abs" | "floor" | "ceil" | "round" | "min" | "max" | "trunc" | "sign"
                )
        }
        _ => false,
    }
}

/// One operation's text with its slow path replaced by `bail`. The fast path of a property or
/// element read is its cache hit and the slow path the last `if (!handled)`; of an intrinsic
/// method call, the intrinsic and everything from the first `if (!handled)` on; of an
/// arithmetic operation, the guarded double form and its `else`.
pub(super) fn bail_out(op: &GraphOp, text: &str, bail: &str) -> String {
    fn block_end(text: &str, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (i, c) in text[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(open + i);
                    }
                }
                _ => {}
            }
        }
        None
    }
    match op {
        GraphOp::CallMethod { .. } => match text.find("if (!handled") {
            Some(at) => {
                // Everything after the intrinsic, up to the statement's closing brace.
                let tail_close = text.rfind('}').unwrap_or(text.len());
                format!(
                    "{}if (!handled) {{ {bail} }} {}",
                    &text[..at],
                    &text[tail_close..]
                )
            }
            None => text.to_owned(),
        },
        GraphOp::GetProperty { .. } | GraphOp::GetComputed { .. } => {
            match text.rfind("if (!handled) {") {
                Some(at) => {
                    let open = at + "if (!handled) ".len();
                    match block_end(text, open) {
                        Some(close) => format!(
                            "{}if (!handled) {{ {bail} }}{}",
                            &text[..at],
                            &text[close + 1..]
                        ),
                        None => text.to_owned(),
                    }
                }
                None => text.to_owned(),
            }
        }
        GraphOp::Unary { .. } | GraphOp::Binary { .. } => match text.find("} else {") {
            Some(at) => {
                let open = at + "} else ".len();
                match block_end(text, open) {
                    Some(close) => {
                        format!("{}}} else {{ {bail} }}{}", &text[..at], &text[close + 1..])
                    }
                    None => text.to_owned(),
                }
            }
            None => text.to_owned(),
        },
        _ => text.to_owned(),
    }
}

/// The placeholder every slow path of a restartable body is replaced by while the text is
/// checked; the real restart is spliced in last, so the check sees the body and nothing else.
pub(super) const RESTART: &str = "KILN_RESTART;";

/// A restartable clone of a function -- the fast clone, or the typed entry over a restartable
/// body -- or `None` when the body cannot be restarted: an operation with an effect before a
/// slow path, a handler, a suspension, or a slow path the surgery could not replace. The text
/// is what will be compiled, so the last word is a scan of it.
///
/// Inside such a body a guarded operation's result is known on the fast path: an arithmetic
/// operation whose operands failed the number test has restarted, so past it the result is a
/// number, and so is what a `Math` intrinsic answered. The clone is planned with those proofs
/// folded in, and its intermediates stay unboxed.
#[allow(clippy::too_many_arguments)]
pub fn emit_restartable(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    symbol: &str,
    index: usize,
    scalar_proto: bool,
    diagnostic_site: &str,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    plan: &CallPlan,
    kind: EntryKind,
    type_profile_record: bool,
) -> Option<String> {
    debug_assert!(kind.restarts());
    if proto.generator || proto.asynchronous || plan.publishes_frame[index] {
        return None;
    }
    for block in &graph.blocks {
        if block.exception_handler.is_some() {
            return None;
        }
        if !matches!(
            block.terminator,
            Terminator::Return(_)
                | Terminator::Branch { .. }
                | Terminator::BranchNotNullish { .. }
                | Terminator::Jump(_)
        ) {
            return None;
        }
        for (vid, op) in &block.operations {
            if !restartable(op, plan, *vid, graph, program) {
                return None;
            }
        }
    }
    let mut restart_plan = plan.clone();
    restart_plan.numbers = guarded_numbers(graph, program, plan);
    let mut text = String::new();
    emit_function_from_graph_with_profile(
        &mut text,
        graph,
        proto,
        program,
        symbol,
        index,
        scalar_proto,
        diagnostic_site,
        frame_stacks,
        strict_functions,
        &restart_plan,
        kind,
        None,
        type_profile_record,
    );
    const EFFECTS: &[&str] = &[
        "goto kiln_",
        "return status",
        "kiln_c99_typed_status = ",
        "kiln_c99_val_call",
        "->body(",
        "kiln_c99_invoke",
        "kiln_c99_val_read_property(",
        "kiln_c99_val_read_element(",
        "kiln_c99_val_binary(",
        "kiln_c99_val_unary(",
        "thrown",
        "locals[",
    ];
    // The body, past the signature: a typed entry's parameter list names `thrown` when the
    // entry needs it, and the decision may not depend on what the entry needs.
    let body = &text[text.find(") {\n").map_or(0, |at| at + 4)..];
    if EFFECTS.iter().any(|needle| body.contains(needle)) {
        return None;
    }
    let bail = match kind {
        EntryKind::Fast => fast_bail(symbol, index),
        EntryKind::Typed { returns_number, .. } => typed_bail(symbol, index, proto, returns_number),
        EntryKind::Boxed => unreachable!("a boxed body is not a restart"),
    };
    Some(text.replace(RESTART, &bail))
}

/// What a restartable body proves on its fast path, beyond the plan: every guarded arithmetic
/// result and every `Math` intrinsic's answer is a number, because the path that saw anything
/// else has restarted.
fn guarded_numbers(graph: &FunctionGraph, program: &Program, plan: &CallPlan) -> HashSet<ValueId> {
    let mut assumptions: std::collections::HashMap<ValueId, Proven> = plan
        .numbers
        .iter()
        .map(|&v| (v, Proven::Number))
        .chain(plan.ints.iter().map(|&v| (v, Proven::Int)))
        .collect();
    // A property read that feeds an operator only numbers accept is one: a shape says which
    // slots an object has and never what is in them, so without this every `p.x * p.x` runs
    // boxed and nothing downstream of it is proven either. `+` is deliberately not in the list.
    // It is the operator a string also takes, and a body that concatenates a property would
    // then restart on every call and never use this entry again -- where the versioned body's
    // guess costs a jump to the clone beside it and can afford `+`.
    let numeric_only = |value: &ValueId| {
        graph
            .blocks
            .iter()
            .flat_map(|b| b.operations.iter())
            .any(|(_, op)| match op {
                GraphOp::Binary { op, left, right } => {
                    (left == value || right == value)
                        && matches!(
                            op,
                            BinaryOp::Subtract
                                | BinaryOp::Multiply
                                | BinaryOp::Divide
                                | BinaryOp::Exponentiate
                                | BinaryOp::Remainder
                        )
                }
                _ => false,
            })
    };
    for (vid, op) in graph.blocks.iter().flat_map(|b| b.operations.iter()) {
        let guarded = match op {
            GraphOp::Binary { op, .. } => matches!(
                op,
                BinaryOp::Add
                    | BinaryOp::Subtract
                    | BinaryOp::Multiply
                    | BinaryOp::Divide
                    | BinaryOp::Exponentiate
                    | BinaryOp::Remainder
            ),
            GraphOp::CallMethod { .. } => restartable(op, plan, *vid, graph, program),
            GraphOp::GetProperty { .. } | GraphOp::GetComputed { .. } => numeric_only(vid),
            _ => false,
        };
        if guarded {
            assumptions.entry(*vid).or_insert(Proven::Number);
        }
    }
    crate::graph::proven_types_with(graph, &assumptions)
        .into_iter()
        .filter_map(|(v, kind)| kind.is_number().then_some(v))
        .collect()
}

/// The fast clone of a function: the boxed signature over a restartable body.
#[allow(clippy::too_many_arguments)]
pub fn emit_fast_clone(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    symbol: &str,
    index: usize,
    scalar_proto: bool,
    diagnostic_site: &str,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    plan: &CallPlan,
) -> Option<String> {
    emit_restartable(
        graph,
        proto,
        program,
        symbol,
        index,
        scalar_proto,
        diagnostic_site,
        frame_stacks,
        strict_functions,
        plan,
        EntryKind::Fast,
        false,
    )
}

/// A restartable typed entry's way out: the boxed body, once, with the parameters boxed
/// again, and its answer handed back the way the typed entry answers. A status other than
/// success is left pending exactly as the routed typed body leaves it.
fn typed_bail(symbol: &str, index: usize, proto: &FunctionProto, returns_number: bool) -> String {
    let argc = proto.parameter_count;
    let boxed: Vec<String> = (0..argc)
        .map(|k| format!("kiln_c99_val_from_double(p{k})"))
        .collect();
    let init = if boxed.is_empty() {
        "KILN_VAL_UNDEFINED".to_owned()
    } else {
        boxed.join(", ")
    };
    // The plan proved every return a number, so the boxed answer is one: its bits are the
    // double.
    let answer = if returns_number {
        "double kiln_restart_d; memcpy(&kiln_restart_d, &kiln_restart_out, sizeof(kiln_restart_d)); return kiln_restart_d;"
    } else {
        "return kiln_restart_out;"
    };
    let fail = if returns_number {
        "kiln_c99_typed_fail_double()"
    } else {
        "KILN_VAL_TYPED_FAIL"
    };
    format!(
        "{{ kiln_call_context kiln_restart_ctx = {{ host, thrown, NULL, depth }}; kiln_val kiln_restart_args[{n}] = {{ {init} }}; kiln_val kiln_restart_out = KILN_VAL_UNDEFINED; int kiln_restart_status = {symbol}_fn_{index}(&kiln_restart_ctx, &kiln_restart_out, upvalues, &receiver, kiln_restart_args, (size_t){argc}); if (KILN_UNLIKELY(kiln_restart_status != KILN_C99_OK)) {{ kiln_c99_typed_status = kiln_restart_status; if (kiln_restart_status == KILN_C99_THROWN) kiln_c99_typed_thrown = *thrown; return {fail}; }} {answer} }}",
        n = argc.max(1)
    )
}

/// The fast clone's way out: the boxed entry, from the top, with everything as it arrived.
fn fast_bail(symbol: &str, index: usize) -> String {
    format!("return {symbol}_fn_{index}(ctx, result, upvalues, receiver_ptr, args, argc);")
}

/// What a typed entry returns on failure, for the kind of result it has.
pub(super) fn typed_fail(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Typed {
            returns_number: true,
            ..
        } => "kiln_c99_typed_fail_double()",
        _ => "KILN_VAL_TYPED_FAIL",
    }
}

/// A typed entry's `return status;` and `return KILN_C99_*;` leave the status pending and
/// return the failure value instead: the entry's result is in a register, so a status cannot
/// travel in it. Applied to the emitted body text, which is exactly what will be compiled.
pub(super) fn route_typed_failures(body: &str, fail: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find("return ") {
        let (before, after) = rest.split_at(at);
        out.push_str(before);
        match after[7..].find(';').map(|i| i + 7) {
            Some(end) => {
                let token = &after[7..end];
                if token == "status" || (token.starts_with("KILN_C99_") && token != "KILN_C99_OK") {
                    out.push_str(&format!(
                        "{{ kiln_c99_typed_status = {token}; return {fail}; }}"
                    ));
                } else {
                    out.push_str(&after[..=end]);
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}
