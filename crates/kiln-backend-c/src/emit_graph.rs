//! C99 code generation directly from the SSA Value Graph.
//!
//! As specified in Phase 1 Step 1.3 of `fastest-native-plan.md` and ADR 0056:
//! - Eliminates the memory-bound `stack[]` array and `sp` stack pointer.
//! - Emits SSA intermediate values directly as C local variables (`kiln_val v0`, `v1`, ...).
//! - Retains `locals[]` only when needed for closure cells or generator suspension frames.
//! - Materializes call arguments into local stack arrays at call sites only, avoiding whole-body address-taking.

use crate::direct::{CallPlan, TypedNeeds, direct_callable};
use crate::graph::{
    BasicBlock, BlockId, FunctionGraph, FunctionTypeProfile, GraphOp, InductionVariable, Proven,
    Terminator, ValueId, as_safe_integer, candidate_number_parameters_with_profile,
    find_induction_variables, proven_ints_with_assumptions, proven_numbers_with_assumptions,
    resolve_const_string,
};
use crate::prelude::lookup_speculation_invariant;
use crate::tail::{TailContext, emit_tail_instruction};
use crate::{BuiltinLoad, builtin_load, closure_construction, proto_publishes_frame};
use kiln_ir::Upvalue;
use kiln_ir::{BinaryOp, Constant, ExportBinding, FunctionProto, Instruction, Program, UnaryOp};
use std::collections::HashSet;
use std::fmt::Write;

/// The first operation or terminator in `graph` that has no lowering below, as a short name,
/// for the census. `None` when everything lowers.
pub fn first_unlowerable(graph: &FunctionGraph) -> Option<String> {
    for block in &graph.blocks {
        for (_, op) in &block.operations {
            if !lowers(op) {
                let name = format!("{op:?}");
                return Some(name.split([' ', '(', '{']).next().unwrap_or("?").to_owned());
            }
        }
    }
    None
}

fn lowers(op: &GraphOp) -> bool {
    matches!(
        op,
        GraphOp::Const { .. }
            | GraphOp::LoadLocal(_)
            | GraphOp::StoreLocal { .. }
            | GraphOp::LoadCell(_)
            | GraphOp::StoreCell { .. }
            | GraphOp::LoadUpvalue(_)
            | GraphOp::StoreUpvalue { .. }
            | GraphOp::LoadBuiltin(_)
            | GraphOp::CaughtValue
            | GraphOp::Bridge { .. }
            | GraphOp::MakeCell(_)
            | GraphOp::LoadGlobal(_)
            | GraphOp::StoreGlobal { .. }
            | GraphOp::LoadThis
            | GraphOp::LoadNewTarget
            | GraphOp::MarkUninitialized(_)
            | GraphOp::CheckTdz { .. }
            | GraphOp::Unary { .. }
            | GraphOp::Binary { .. }
            | GraphOp::Call { .. }
            | GraphOp::CallMethod { .. }
            | GraphOp::NewArray { .. }
            | GraphOp::NewObject { .. }
            | GraphOp::New { .. }
            | GraphOp::MakeFunction { .. }
            | GraphOp::GetProperty { .. }
            | GraphOp::SetProperty { .. }
            | GraphOp::SetPropertyStrict { .. }
            | GraphOp::GetComputed { .. }
            | GraphOp::SetComputed { .. }
            | GraphOp::SetComputedStrict { .. }
            | GraphOp::Phi { .. }
    )
}

/// Which clone of a function body this is. The boxed entry is what a closure carries and what
/// every generic call reaches. The typed entry (`_t`) is reached only by direct calls that
/// proved every argument a number and passed exactly the parameter count: inside it the
/// parameters are numbers by contract, and nothing checks what the caller already proved.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Boxed,
    /// The boxed signature, with every slow path replaced by a restart through the boxed
    /// entry. Only a body that can be restarted gets one: nothing in it has an effect before
    /// the point a slow path could be taken, so running it again from the top is the same
    /// as running the slow path. With no runtime call left in it, the clone needs no saved
    /// registers and the C compiler inlines it where a cached callee is called by name.
    Fast,
    /// The parameters arrive as `double`s. When the plan proved the return a number, the result
    /// leaves as one too, through `double *result`; otherwise through `kiln_val *result`.
    Typed {
        returns_number: bool,
        /// The typed signature over a restartable body: every slow path is a restart through
        /// the boxed entry, exactly as in the fast clone, so what the fast path computes is
        /// known to be a number and stays in a register. The restart boxes the parameters,
        /// runs the boxed body once, and hands its answer back through the typed result.
        restartable: bool,
    },
}

/// What a versioned body's fast clone assumes beyond its parameters, and how it leaves when
/// an assumption fails. An invariant read in `arrays` is an array, checked once at entry. A
/// method call in `calls` answers a number: checked where it answers, and a call that answers
/// anything else leaves for the generic clone at the label after that same call, with every
/// value before it already computed. The generic clone, `exits` false, only carries the labels.
#[derive(Default, Clone, Debug)]
pub struct Speculation {
    pub arrays: HashSet<ValueId>,
    pub calls: HashSet<ValueId>,
    pub exits: bool,
}

impl EntryKind {
    /// Whether every slow path of this body is a restart through the boxed entry.
    pub fn restarts(self) -> bool {
        matches!(
            self,
            EntryKind::Fast
                | EntryKind::Typed {
                    restartable: true,
                    ..
                }
        )
    }
}

/// Which body a graph is emitted as: a function, called through a closure, or the unit's entry
/// point, which the embedding calls once with the module's imports and takes the exports from.
pub enum Site<'a> {
    Function {
        /// This function's index, so a call to itself can reuse its own upvalue table.
        index: u32,
    },
    Entry {
        /// The module's exports, read from `locals[]` at the return; `None` for a scalar unit.
        exports: Option<&'a [ExportBinding]>,
        /// A CommonJS unit: its one export is `module.exports`, read from the `module` slot.
        commonjs: bool,
    },
}

/// The protos this unit defines as a method of `key` -- `class C { key() {} }` and
/// `{ key() {} }` -- read off the bytecode as `Const(key) MakeFunction(i) DefineMethod`. A hint
/// only: a method-call site whose cached callee's body is one of these calls it by name, which
/// lets the C compiler inline it; a callee that is none of them takes the indirect call. Nothing
/// is assumed about which closure a site sees.
pub(crate) fn method_candidates(program: &Program, key: &str) -> Vec<u32> {
    let mut found = Vec::new();
    let mut scan = |code: &[Instruction]| {
        for window in code.windows(3) {
            if let [
                Instruction::Const(k),
                Instruction::MakeFunction(i),
                Instruction::DefineMethod,
            ] = window
                && matches!(program.constants.get(*k as usize), Some(Constant::String(s)) if s == key)
                && direct_callable(&program.functions[*i as usize])
                && !found.contains(i)
            {
                found.push(*i);
            }
        }
    };
    scan(&program.code);
    for proto in &program.functions {
        scan(&proto.code);
    }
    found
}

/// The typed entry's declaration: the result in a register, `double` where the plan proved
/// it and `kiln_val` otherwise, with `KILN_VAL_TYPED_FAIL` for a failure; then the depth, what
/// the body needs and nothing it does not, and the parameters as the doubles they are.
pub fn typed_signature(
    proto: &FunctionProto,
    returns_number: bool,
    needs: TypedNeeds,
) -> (&'static str, String) {
    let result = if returns_number { "double" } else { "kiln_val" };
    let mut params = String::from("int depth");
    if needs.upvalues {
        params.push_str(", kiln_val *upvalues");
    }
    if needs.receiver {
        params.push_str(", kiln_val receiver");
    }
    if needs.runtime {
        params.push_str(", kiln_host *host, kiln_value *thrown");
    }
    for k in 0..proto.parameter_count {
        params.push_str(&format!(", double p{k}"));
    }
    (result, params)
}

/// A runtime call that writes its answer through `&vN` takes the address of the SSA value, and
/// a C compiler then keeps that value in memory for the whole body -- every read of it, on
/// every path, pays a load and a store, for a slow path that rarely runs. The answer goes
/// through one scratch slot per body instead and is copied into the value, which stays in a
/// register. `memcpy(&dN, &vN, ..)` is the double twin's move and is left alone.
fn hoist_out_params(body: &str) -> (String, bool) {
    let mut out = String::with_capacity(body.len());
    let mut used = false;
    for line in body.split_inclusive('\n') {
        let mut rest = line;
        let mut rewritten = String::with_capacity(line.len());
        while let Some(at) = rest.find("&v") {
            let digits_end = rest[at + 2..]
                .find(|c: char| !c.is_ascii_digit())
                .map_or(rest.len(), |i| at + 2 + i);
            let digits = &rest[at + 2..digits_end];
            // Inside an open `memcpy(` the address is a source or the twin's destination.
            let before = format!("{rewritten}{}", &rest[..at]);
            let in_memcpy = before
                .rfind("memcpy(")
                .is_some_and(|m| !before[m..].contains(')'));
            if digits.is_empty() || in_memcpy || !rest[digits_end..].starts_with([')', ',']) {
                rewritten.push_str(&rest[..digits_end]);
                rest = &rest[digits_end..];
                continue;
            }
            // The call's own end: its balanced close followed by `;`. A call inside a
            // condition or a nested argument has no statement of its own to append to.
            let mut depth = 1usize;
            let mut close = None;
            for (i, c) in rest[digits_end..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            if rest[digits_end + i + 1..].starts_with(';') {
                                close = Some(digits_end + i + 2);
                            }
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(close) = close else {
                rewritten.push_str(&rest[..digits_end]);
                rest = &rest[digits_end..];
                continue;
            };
            rewritten.push_str(&rest[..at]);
            rewritten.push_str("&kiln_out");
            rewritten.push_str(&rest[digits_end..close]);
            rewritten.push_str(&format!(" v{digits} = kiln_out;"));
            used = true;
            rest = &rest[close..];
        }
        rewritten.push_str(rest);
        out.push_str(&rewritten);
    }
    (out, used)
}

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
        | GraphOp::Unary { .. } => true,
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
                Some(Constant::String(s)) => s.as_str(),
                _ => "",
            };
            on_math
                && matches!(
                    key,
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
fn bail_out(op: &GraphOp, text: &str, bail: &str) -> String {
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
const RESTART: &str = "KILN_RESTART;";

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
    if proto.generator || proto.asynchronous || proto_publishes_frame(proto) {
        return None;
    }
    for block in &graph.blocks {
        if block.exception_handler.is_some() {
            return None;
        }
        if !matches!(
            block.terminator,
            Terminator::Return(_) | Terminator::Branch { .. } | Terminator::Jump(_)
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

/// A function's inline-cache sites, shared by every clone of it. Each operation declares its
/// site inline as `static kiln_c99_site site`; a clone that declared its own would start
/// empty and, having no slow path to learn with, stay empty. So the sites are hoisted to file
/// scope, named per function and numbered in emission order -- the same order in every clone
/// -- and only the boxed clone, emitted first, declares them. Returns the body with the sites
/// renamed and how many there were.
fn share_sites(body: &str, prefix: &str) -> (String, usize) {
    const DECLARATION: &str =
        "static kiln_c99_site site = {KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}; ";
    let mut out = String::with_capacity(body.len());
    let mut count = 0usize;
    for line in body.split_inclusive('\n') {
        if let Some(at) = line.find(DECLARATION) {
            let name = format!("{prefix}_{count}");
            count += 1;
            let stripped = format!("{}{}", &line[..at], &line[at + DECLARATION.len()..]);
            // `site` as a token of its own: `site.`, `&site`, `(site` -- never inside an
            // identifier such as `kiln_c99_site` or `diagnostic_site`.
            let mut renamed = String::with_capacity(stripped.len() + 64);
            let bytes = stripped.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if stripped[i..].starts_with("site")
                    && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'))
                    && !bytes
                        .get(i + 4)
                        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    renamed.push_str(&name);
                    i += 4;
                } else {
                    let ch = stripped[i..].chars().next().unwrap();
                    renamed.push(ch);
                    i += ch.len_utf8();
                }
            }
            out.push_str(&renamed);
        } else {
            out.push_str(line);
        }
    }
    (out, count)
}

/// The reads of `this` in a body, each with its column in the function's receiver-shape
/// cache: one column per property name, in first-read order -- the same order in every
/// clone. Two reads of one name share a column because for one receiver of one shape they
/// are one slot, and the C compiler then reads it once.
fn this_columns(graph: &FunctionGraph) -> std::collections::HashMap<ValueId, usize> {
    let mut names: Vec<u32> = Vec::new();
    graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(vid, op)| match op {
            GraphOp::GetProperty { object, name, .. }
                if graph
                    .blocks
                    .iter()
                    .flat_map(|b| b.operations.iter())
                    .any(|(v, def)| v == object && matches!(def, GraphOp::LoadThis)) =>
            {
                let column = names.iter().position(|n| n == name).unwrap_or_else(|| {
                    names.push(*name);
                    names.len() - 1
                });
                Some((*vid, column))
            }
            _ => None,
        })
        .collect()
}

/// How many columns the receiver-shape cache of a body has.
fn this_site_count(graph: &FunctionGraph) -> usize {
    this_columns(graph)
        .values()
        .map(|c| c + 1)
        .max()
        .unwrap_or(0)
}

/// The receiver-shape cache's match at entry: the row whose shape is the receiver's, or -1.
/// The boxed clone, whose slow path learns, also claims a free row for a shape it has not
/// seen. Emitted right after `receiver` is defined and before any dispatch that could jump
/// past it: a resumed generator lands at its landing with this already run.
fn receiver_prologue(symbol: &str, index: usize, kind: EntryKind) -> String {
    let claim = if kind == EntryKind::Boxed {
        format!(
            " else {{ int j; for (j = 0; j < 4; ++j) if ({symbol}_fn_{index}_recv_shape[j] == NULL) {{ {symbol}_fn_{index}_recv_shape[j] = recv_shape; recv_k = j; break; }} }}"
        )
    } else {
        String::new()
    };
    format!(
        "    int recv_k = -1; const kiln_object *recv_obj = NULL; if (KILN_VAL_IS_OBJECT(receiver)) {{ recv_obj = (const kiln_object *)KILN_VAL_PTR(receiver); const kiln_shape *recv_shape = recv_obj->shape; if (recv_shape != NULL) {{ if (recv_shape == {symbol}_fn_{index}_recv_shape[0]) recv_k = 0; else if (recv_shape == {symbol}_fn_{index}_recv_shape[1]) recv_k = 1; else if (recv_shape == {symbol}_fn_{index}_recv_shape[2]) recv_k = 2; else if (recv_shape == {symbol}_fn_{index}_recv_shape[3]) recv_k = 3;{claim} }} }} const uint32_t *recv_row = recv_k >= 0 ? {symbol}_fn_{index}_recv_slot[recv_k] : NULL; (void)recv_row;\n"
    )
}

/// The fast clone's way out: the boxed entry, from the top, with everything as it arrived.
fn fast_bail(symbol: &str, index: usize) -> String {
    format!("return {symbol}_fn_{index}(ctx, result, upvalues, receiver_ptr, args, argc);")
}

/// What a typed entry returns on failure, for the kind of result it has.
fn typed_fail(kind: EntryKind) -> &'static str {
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
fn route_typed_failures(body: &str, fail: &str) -> String {
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

pub(crate) use crate::graph::array_slots;

/// Whether every operation in `graph` has a lowering below. This is the one place that decides
/// what the emitter takes; `graph_for` asks it and fails the unit's emission otherwise.
pub fn can_lower(graph: &FunctionGraph) -> bool {
    first_unlowerable(graph).is_none()
}

/// Emits the C99 function body for a function proto using its SSA Value Graph.
#[allow(clippy::too_many_arguments)]
pub fn emit_function_from_graph(
    output: &mut String,
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
) {
    emit_function_from_graph_with_profile(
        output,
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
        kind,
        None,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn emit_function_from_graph_with_profile(
    output: &mut String,
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
    func_profile: Option<&FunctionTypeProfile>,
    type_profile_record: bool,
) {
    let function_start = output.len();
    // 1. Function comment header
    if let Some(name) = proto.name.as_deref().filter(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$')
    }) {
        match kind {
            EntryKind::Boxed => writeln!(output, "/* function {index}: {name} */").unwrap(),
            EntryKind::Typed { .. } => {
                writeln!(output, "/* function {index}: {name}, typed entry */").unwrap()
            }
            EntryKind::Fast => {
                writeln!(output, "/* function {index}: {name}, fast clone */").unwrap()
            }
        }
    }

    // 2. Signature. The typed entry takes what a direct call has in registers: no context
    // struct to build and read back, no argument array, no count. Every caller of it proved
    // the arguments are numbers and passed exactly the parameter count.
    match kind {
        EntryKind::Boxed => writeln!(
            output,
            "static inline int {symbol}_fn_{index}(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues, const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {{"
        )
        .unwrap(),
        EntryKind::Fast => writeln!(
            output,
            "static inline __attribute__((always_inline)) int {symbol}_fn_{index}_f(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues, const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {{"
        )
        .unwrap(),
        EntryKind::Typed {
            returns_number,
            restartable,
        } => {
            let (result, params) =
                typed_signature(proto, returns_number, plan.typed_needs[index]);
            // A restartable typed entry is the fast clone's body with a typed signature: no
            // runtime call survives in it, so it inlines where a method call enters it by
            // name, and the receiver the caller matched is the receiver it reads.
            let inline = if restartable {
                "static inline __attribute__((always_inline))"
            } else {
                "static inline"
            };
            writeln!(
                output,
                "{inline} {result} {symbol}_fn_{index}_t({params}) {{"
            )
            .unwrap();
        }
    }

    // 3. Context & arguments setup. `ctx` is never null: every caller, the runtime's and the
    // emitted direct path alike, passes its own.
    let receiver_cache = if this_site_count(graph) > 0 {
        receiver_prologue(symbol, index, kind)
    } else {
        String::new()
    };
    if matches!(kind, EntryKind::Typed { .. }) {
        output.push_str("    (void)depth;\n");
        output.push_str(&receiver_cache);
    } else if kind == EntryKind::Fast {
        // No runtime call survives in a fast clone; what it reads is the receiver and the
        // arguments, and the context only to hand on when it restarts.
        output.push_str("    (void)upvalues;\n");
        output.push_str(
            "    kiln_val receiver = receiver_ptr ? *receiver_ptr : KILN_VAL_UNDEFINED;\n",
        );
        output.push_str(&receiver_cache);
    } else if scalar_proto {
        output.push_str("    (void)ctx; (void)receiver_ptr; (void)upvalues;\n");
    } else {
        output.push_str("    kiln_host *host = ctx->host;\n");
        output.push_str("    kiln_value *thrown = ctx->thrown;\n");
        output.push_str("    kiln_generator *frame = ctx->frame;\n");
        output.push_str("    int depth = ctx->depth;\n");
        output.push_str(
            "    kiln_val receiver = receiver_ptr ? *receiver_ptr : KILN_VAL_UNDEFINED;\n",
        );
        output.push_str("    (void)receiver_ptr;\n");
        output.push_str(&receiver_cache);
        // Which of the two `arguments` objects a mention in this body, or in an arrow made
        // here, would build. Only such a body reads the frame global, so only such a body
        // writes it; a plain body leaves its caller's value alone.
        if proto_publishes_frame(proto) {
            writeln!(
                output,
                "    kiln_c99_frame_mapped_arguments = {};",
                i32::from(proto.mapped_arguments)
            )
            .unwrap();
        }
    }
    if kind == EntryKind::Boxed {
        output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
        if type_profile_record && proto.parameter_count > 0 {
            writeln!(
                output,
                "    for (size_t _pi = 0; _pi < argc && _pi < {}; ++_pi) {{ {symbol}_fn_{index}_param_tags[_pi] |= kiln_profile_tag_of(args[_pi]); }}",
                proto.parameter_count
            )
            .unwrap();
        }
    } else if matches!(kind, EntryKind::Typed { .. })
        && type_profile_record
        && proto.parameter_count > 0
    {
        for p in 0..proto.parameter_count {
            writeln!(output, "    {symbol}_fn_{index}_param_tags[{p}] |= 1;").unwrap();
        }
    }

    // `locals[]` exists only for slots that are read through the array: a cell (the slot holds
    // the box, and closures read the box), or a slot some block loads because no SSA value
    // reaches it. Every other slot lives in SSA values and phis, and the array is not declared
    // at all when no slot needs it. The entry block reads a parameter from `args` directly.
    let suspends = proto.generator || proto.asynchronous;
    let array_slots = if suspends {
        (0..proto.local_count).collect::<HashSet<u32>>()
    } else {
        array_slots(graph, program)
    };
    let locals = usize::try_from(proto.local_count).unwrap().max(1);
    if suspends {
        // The frame outlives this call: its locals are the generator's, bound by the runtime
        // before the first entry, and its stack holds what a suspend spilled.
        output.push_str("    kiln_val *locals = frame->locals;\n    (void)args; (void)argc;\n");
        if !graph
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::Suspend { .. }))
            && graph.seam_block.is_none()
        {
            output.push_str("    (void)frame;\n");
        } else {
            output.push_str("    if (frame->started) {\n        switch (frame->resume) {\n");
            for block in &graph.blocks {
                if let Terminator::Suspend { pc, .. } = block.terminator {
                    writeln!(output, "        case {pc}: goto kiln_resume_{pc};").unwrap();
                }
            }
            if let Some(seam) = graph.seam_block {
                writeln!(
                    output,
                    "        case {}: goto kiln_block_{};",
                    proto.code.len(),
                    seam.0
                )
                .unwrap();
            }
            output.push_str("        default: return KILN_C99_CONTROL_ERROR;\n        }\n    }\n");
        }
    } else if !array_slots.is_empty() {
        writeln!(output, "    kiln_val locals[{locals}];").unwrap();
        writeln!(
            output,
            "    for (size_t slot = 0; slot < (size_t){locals}; ++slot) locals[slot] = KILN_VAL_UNDEFINED;"
        )
        .unwrap();
        if proto.parameter_count > 0 && matches!(kind, EntryKind::Typed { .. }) {
            for k in 0..proto.parameter_count {
                writeln!(output, "    locals[{k}] = kiln_c99_val_from_double(p{k});").unwrap();
            }
        } else if proto.parameter_count > 0 {
            writeln!(
                output,
                "    for (size_t slot = 0; slot < (size_t){} && slot < argc; ++slot) locals[slot] = args[slot];",
                proto.parameter_count
            )
            .unwrap();
        }
    }
    if proto.parameter_count == 0 && !suspends && kind == EntryKind::Boxed {
        output.push_str("    (void)args; (void)argc;\n");
    }

    let (shared_sites, this_sites) = emit_blocks(
        output,
        graph,
        proto,
        program,
        symbol,
        scalar_proto,
        diagnostic_site,
        frame_stacks,
        strict_functions,
        &array_slots,
        &Site::Function {
            index: index as u32,
        },
        plan,
        kind,
        func_profile,
        type_profile_record,
    );
    output.push_str("}\n");
    // The boxed clone, emitted first, declares the sites every clone of this function uses,
    // and the receiver-shape cache: four rows of shapes, a slot per read of `this` in each.
    if kind == EntryKind::Boxed && (shared_sites > 0 || this_sites > 0) {
        let mut declarations = String::new();
        for k in 0..shared_sites {
            writeln!(
                declarations,
                "static kiln_c99_site {symbol}_fn_{index}_site_{k} = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}};"
            )
            .unwrap();
        }
        if this_sites > 0 {
            let row = vec!["UINT32_MAX"; this_sites].join(", ");
            writeln!(
                declarations,
                "static const kiln_shape *{symbol}_fn_{index}_recv_shape[4];\nstatic uint32_t {symbol}_fn_{index}_recv_slot[4][{this_sites}] = {{ {{{row}}}, {{{row}}}, {{{row}}}, {{{row}}} }};"
            )
            .unwrap();
        }
        output.insert_str(function_start, &declarations);
    }
}

/// Emits the unit's entry point body from its graph. The caller has opened the function,
/// declared `locals[]` (the unit's globals, or an automatic array), bound the imports into it,
/// and declared `depth` and `thrown` when the unit has callables; this emits the blocks, the
/// return that hands the exports over, and the tail an uncaught throw or a refusal leaves by.
#[allow(clippy::too_many_arguments)]
pub fn emit_entry_from_graph(
    output: &mut String,
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    symbol: &str,
    scalar_proto: bool,
    diagnostic_site: &str,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    array_slots: &HashSet<u32>,
    exports: Option<&[ExportBinding]>,
    commonjs: bool,
    plan: &CallPlan,
) {
    emit_entry_from_graph_with_profile(
        output,
        graph,
        proto,
        program,
        symbol,
        scalar_proto,
        diagnostic_site,
        frame_stacks,
        strict_functions,
        array_slots,
        exports,
        commonjs,
        plan,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn emit_entry_from_graph_with_profile(
    output: &mut String,
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    symbol: &str,
    scalar_proto: bool,
    diagnostic_site: &str,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    array_slots: &HashSet<u32>,
    exports: Option<&[ExportBinding]>,
    commonjs: bool,
    plan: &CallPlan,
    type_profile_record: bool,
) {
    emit_blocks(
        output,
        graph,
        proto,
        program,
        symbol,
        scalar_proto,
        diagnostic_site,
        frame_stacks,
        strict_functions,
        array_slots,
        &Site::Entry { exports, commonjs },
        plan,
        EntryKind::Boxed,
        None,
        type_profile_record,
    );
}

fn emit_ssa_declarations(
    output: &mut String,
    graph: &FunctionGraph,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
) {
    if !ints.is_empty() {
        let mut ids: Vec<u32> = ints.iter().map(|v| v.0).collect();
        ids.sort_unstable();
        output.push_str("    int64_t ");
        for (i, vid) in ids.iter().enumerate() {
            if i > 0 {
                output.push_str(", ");
            }
            write!(output, "i{vid} = 0").unwrap();
        }
        output.push_str(";\n");
    }
    if !numbers.is_empty() {
        let mut ids: Vec<u32> = numbers.iter().map(|v| v.0).collect();
        ids.sort_unstable();
        output.push_str("    double ");
        for (i, vid) in ids.iter().enumerate() {
            if i > 0 {
                output.push_str(", ");
            }
            write!(output, "d{vid} = 0.0").unwrap();
        }
        output.push_str(";\n");
    }
    if graph.next_value_id > 0 {
        output.push_str("    kiln_val ");
        for vid in 0..graph.next_value_id {
            if vid > 0 {
                output.push_str(", ");
            }
            write!(output, "v{vid} = KILN_VAL_UNDEFINED").unwrap();
        }
        output.push_str(";\n");
    }
    if !graph.scalar_objects.is_empty() {
        let mut sorted_objs: Vec<_> = graph.scalar_objects.values().collect();
        sorted_objs.sort_by_key(|s| s.alloc_id.0);
        for s in sorted_objs {
            let alloc = s.alloc_id.0;
            for &idx in s.fields.values() {
                writeln!(
                    output,
                    "    kiln_val v{alloc}_f_{idx} = KILN_VAL_UNDEFINED;"
                )
                .unwrap();
                writeln!(output, "    double d{alloc}_f_{idx} = 0.0;").unwrap();
                writeln!(output, "    int64_t i{alloc}_f_{idx} = 0;").unwrap();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_block_sequence(
    output: &mut String,
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    symbol: &str,
    scalar_proto: bool,
    diagnostic_site: &str,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    array_slots: &HashSet<u32>,
    site: &Site<'_>,
    plan: &CallPlan,
    kind: EntryKind,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    ivs: &std::collections::HashMap<ValueId, InductionVariable>,
    block_prefix: &str,
    is_fast_clone: bool,
    type_profile_record: bool,
    this_sites: &std::collections::HashMap<ValueId, usize>,
    spec: &Speculation,
) {
    let self_index = match site {
        Site::Function { index } => Some(*index),
        Site::Entry { .. } => None,
    };
    // A pure read nothing consumes is not emitted: it would only name something -- an upvalue
    // table, a slot -- that the body then appears to need. The callee read a direct self-call
    // has replaced is the common case.
    let mut used: HashSet<ValueId> = HashSet::new();
    for block in &graph.blocks {
        for (vid, op) in &block.operations {
            // A direct call reads its callee value only for the upvalue table it carries: not
            // on a self-call, which has its own, and not into a typed entry that takes none.
            // A boxed body's direct call keeps it for the generic path past the depth limit.
            if let GraphOp::Call { callee, args, .. } = op
                && let Some(&target) = plan.known.get(callee)
                && direct_callable(&program.functions[target as usize])
                && matches!(kind, EntryKind::Typed { .. })
            {
                used.extend(args.iter().copied());
                let self_call = self_index == Some(target);
                let typed_site = plan.typed_sites.contains(vid);
                let reads_callee =
                    !self_call && (!typed_site || plan.typed_needs[target as usize].upvalues);
                if reads_callee {
                    used.insert(*callee);
                }
                continue;
            }
            used.extend(op.operands());
        }
        match &block.terminator {
            Terminator::Return(Some(v)) | Terminator::Throw(v) => {
                used.insert(*v);
            }
            Terminator::Branch { condition, .. } => {
                used.insert(*condition);
            }
            Terminator::Suspend { value, spilled, .. } => {
                used.insert(*value);
                used.extend(spilled.iter().copied());
            }
            _ => {}
        }
    }
    let dead_read = |vid: ValueId, op: &GraphOp| {
        !used.contains(&vid)
            && match op {
                GraphOp::LoadGlobal(_) | GraphOp::Const { .. } => true,
                GraphOp::LoadCell(_) | GraphOp::LoadUpvalue(_) => plan.unchecked.contains(&vid),
                _ => false,
            }
    };
    for block in &graph.blocks {
        // A restartable body never suspends, so a block nothing jumps to is the code the
        // compiler leaves after a body's last statement: emitted, it would be a refusal in a
        // typed entry, and a typed entry with a refusal in it is not restartable.
        if kind.restarts() && block.id != graph.entry_block && block.predecessors.is_empty() {
            continue;
        }
        for from in &graph.blocks {
            if let Terminator::Suspend {
                resume,
                pc,
                spilled,
                ..
            } = &from.terminator
                && *resume == block.id
            {
                let landing_unwind = match block.exception_handler {
                    Some(handler) => format!("goto {block_prefix}{};", handler.0),
                    None => "goto kiln_unwind;".to_owned(),
                };
                let phis: Vec<u32> = block
                    .operations
                    .iter()
                    .filter(|(_, op)| matches!(op, GraphOp::Phi { .. }))
                    .map(|(v, _)| v.0)
                    .collect();
                writeln!(output, "kiln_resume_{pc}:;").unwrap();
                writeln!(
                    output,
                    "    if (frame->raising) {{ *thrown = frame->sent; {landing_unwind} }}"
                )
                .unwrap();
                for (i, phi) in phis.iter().enumerate() {
                    if i < spilled.len() {
                        writeln!(output, "    v{phi} = frame->stack[{i}];").unwrap();
                    } else {
                        writeln!(output, "    v{phi} = kiln_val_from_boundary(frame->sent);")
                            .unwrap();
                    }
                }
            }
        }
        writeln!(output, "{block_prefix}{}:;", block.id.0).unwrap();
        if graph.seam_block == Some(block.id) {
            output
                .push_str("    if (frame->binding) { frame->sp = 0; return KILN_C99_YIELDED; }\n");
        }
        let unwind = match block.exception_handler {
            Some(handler) => format!("goto {block_prefix}{};", handler.0),
            None => "goto kiln_unwind;".to_owned(),
        };

        if is_fast_clone && block.predecessors.len() >= 2 {
            let loop_inv = lookup_speculation_invariant("loop_speculation_exit");
            writeln!(
                output,
                "    /* Speculation exit check [{}]: {} */",
                loop_inv.name, loop_inv.language_rule
            )
            .unwrap();
            writeln!(
                output,
                "    static uint32_t kiln_loop_guard_{} = 0;",
                block.id.0
            )
            .unwrap();
            writeln!(
                output,
                "    if (KILN_UNLIKELY(kiln_c99_guard_should_exit_in(kiln_spec_mode, &kiln_loop_guard_{}))) goto kiln_generic_block_{};",
                block.id.0, block.id.0
            )
            .unwrap();
        }

        for &(vid, ref op) in &block.operations {
            if dead_read(vid, op) {
                continue;
            }
            let mut op_text = String::new();
            let target: &mut String = if kind.restarts() {
                &mut op_text
            } else {
                output
            };
            emit_graph_op(
                target,
                vid,
                op,
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
                &graph.origins,
                &unwind,
                plan,
                kind,
                self_index,
                this_sites.get(&vid).copied(),
                spec,
            );
            if kind.restarts() {
                output.push_str(&bail_out(op, &op_text, RESTART));
            }
            if !spec.exits && spec.calls.contains(&vid) {
                writeln!(output, "kiln_generic_resume_{}:;", vid.0).unwrap();
            }
        }

        emit_terminator(
            output,
            graph,
            block,
            &block.terminator,
            numbers,
            ints,
            defs,
            &unwind,
            site,
            scalar_proto,
            kind,
            block_prefix,
            symbol,
            type_profile_record,
        );
    }
}

/// The SSA value declarations, every block with its terminator, and the tail: what a function
/// body and the entry point share once their prologues are written.
#[allow(clippy::too_many_arguments)]
fn emit_blocks(
    output: &mut String,
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    symbol: &str,
    scalar_proto: bool,
    diagnostic_site: &str,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    array_slots: &HashSet<u32>,
    site: &Site<'_>,
    plan: &CallPlan,
    kind: EntryKind,
    func_profile: Option<&FunctionTypeProfile>,
    type_profile_record: bool,
) -> (usize, usize) {
    let body_start = output.len();
    // The reads of `this`, each with its column in the function's receiver-shape cache: one
    // row per shape seen, one column per property name.
    let this_sites = this_columns(graph);
    let candidate_params = candidate_number_parameters_with_profile(graph, proto, func_profile);
    let speculate_enabled = std::env::var("KILN_SPECULATE").as_deref() != Ok("0");
    let suspends = graph
        .blocks
        .iter()
        .any(|b| matches!(b.terminator, Terminator::Suspend { .. }))
        || graph.seam_block.is_some();
    // Beyond the parameters: invariant upvalue reads indexed or measured as arrays, and method
    // calls whose answers feed arithmetic. Neither is proven; both are guessed and checked.
    let ops: Vec<(ValueId, &GraphOp)> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    let spec_arrays: HashSet<ValueId> = ops
        .iter()
        .filter_map(|(vid, op)| match op {
            GraphOp::LoadUpvalue(_) if plan.invariant.contains(vid) => Some(*vid),
            _ => None,
        })
        .filter(|vid| {
            ops.iter().any(|(_, op)| match op {
                GraphOp::GetComputed { object, .. } => object == vid,
                GraphOp::GetProperty { object, name, .. } => {
                    object == vid
                        && matches!(program.constants.get(*name as usize), Some(Constant::String(s)) if s == "length")
                }
                _ => false,
            })
        })
        .collect();
    let spec_calls: HashSet<ValueId> = ops
        .iter()
        .filter_map(|(vid, op)| match op {
            GraphOp::CallMethod { .. } if !plan.numbers.contains(vid) => Some(*vid),
            _ => None,
        })
        .filter(|vid| {
            ops.iter().any(|(_, op)| match op {
                GraphOp::Binary { op, left, right } => {
                    (left == vid || right == vid)
                        && matches!(
                            op,
                            BinaryOp::Add
                                | BinaryOp::Subtract
                                | BinaryOp::Multiply
                                | BinaryOp::Divide
                                | BinaryOp::Remainder
                                | BinaryOp::Exponentiate
                        )
                }
                _ => false,
            })
        })
        .collect();
    let versioning = speculate_enabled
        && !suspends
        && kind == EntryKind::Boxed
        && (!candidate_params.is_empty() || !spec_arrays.is_empty() || !spec_calls.is_empty());

    let baseline_numbers = &plan.numbers;
    let baseline_ints = &plan.ints;
    let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    let ivs = find_induction_variables(graph);

    if versioning {
        // The speculative proof starts from the plan's: what the plan proved by a typed call
        // site or an intrinsic is not derivable from the graph alone, and a union of two
        // proofs is not a proof -- an operation whose operands each came from one would be
        // typed by neither and have no twin.
        let mut candidate_assumptions: std::collections::HashMap<ValueId, Proven> =
            baseline_numbers
                .iter()
                .map(|&v| (v, Proven::Number))
                .chain(baseline_ints.iter().map(|&v| (v, Proven::Int)))
                .collect();
        for &slot in &candidate_params {
            if let Some(&vid) = graph.initial_locals.get(slot) {
                candidate_assumptions.insert(vid, Proven::Number);
            }
        }
        for vid in &spec_calls {
            candidate_assumptions.insert(*vid, Proven::Number);
        }
        // An array's `length` is a number.
        for (vid, op) in &ops {
            if let GraphOp::GetProperty { object, .. } = op
                && spec_arrays.contains(object)
            {
                candidate_assumptions.insert(*vid, Proven::Number);
            }
        }
        let speculative_numbers = proven_numbers_with_assumptions(graph, &candidate_assumptions);
        let mut all_numbers = baseline_numbers.clone();
        all_numbers.extend(&speculative_numbers);

        let speculative_ints = proven_ints_with_assumptions(graph, &candidate_assumptions);
        let mut all_ints = baseline_ints.clone();
        all_ints.extend(&speculative_ints);

        emit_ssa_declarations(output, graph, &all_numbers, &all_ints);

        let index = match site {
            Site::Function { index } => *index,
            _ => 0,
        };
        let invariant = lookup_speculation_invariant("parameter_number");
        writeln!(
            output,
            "    /* Speculation entry guard [{}]: {} */",
            invariant.name, invariant.language_rule
        )
        .unwrap();
        writeln!(output, "    static uint32_t kiln_guard_entry_{index} = 0;").unwrap();
        output.push_str("    const int kiln_spec_mode = kiln_c99_speculate_exit_active();\n");
        let mut conds = vec![format!(
            "!kiln_c99_guard_should_exit_in(kiln_spec_mode, &kiln_guard_entry_{index})"
        )];
        for &slot in &candidate_params {
            conds.push(format!(
                "(argc > {slot} && KILN_VAL_IS_DOUBLE(args[{slot}]))"
            ));
        }
        let mut spec_sorted: Vec<ValueId> = spec_arrays.iter().copied().collect();
        spec_sorted.sort();
        for vid in &spec_sorted {
            let Some(GraphOp::LoadUpvalue(index)) = defs.get(vid) else {
                continue;
            };
            let invariant = lookup_speculation_invariant("upvalue_array");
            writeln!(
                output,
                "    /* Speculation entry guard [{}]: {} */",
                invariant.name, invariant.language_rule
            )
            .unwrap();
            writeln!(
                output,
                "    kiln_val kiln_spec_{} = *(const kiln_val *)KILN_VAL_PTR(upvalues[{index}]);",
                vid.0
            )
            .unwrap();
            conds.push(format!(
                "((kiln_spec_{v} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && kiln_c99_gc_kind_of_ptr(KILN_VAL_PTR(kiln_spec_{v})) == KILN_GC_ARRAY)",
                v = vid.0
            ));
        }
        let entry_id = graph.entry_block.0;
        writeln!(
            output,
            "    if (KILN_LIKELY({})) {{\n        goto kiln_fast_block_{entry_id};\n    }} else {{\n        goto kiln_generic_block_{entry_id};\n    }}",
            conds.join(" && ")
        )
        .unwrap();

        writeln!(output, "    /* Clone A: guarded fast path */").unwrap();
        emit_block_sequence(
            output,
            graph,
            proto,
            program,
            symbol,
            scalar_proto,
            diagnostic_site,
            frame_stacks,
            strict_functions,
            array_slots,
            site,
            plan,
            kind,
            &all_numbers,
            &all_ints,
            &defs,
            &ivs,
            "kiln_fast_block_",
            true,
            type_profile_record,
            &this_sites,
            &Speculation {
                arrays: spec_arrays.clone(),
                calls: spec_calls.clone(),
                exits: true,
            },
        );

        writeln!(output, "    /* Clone B: generic fallback path */").unwrap();
        emit_block_sequence(
            output,
            graph,
            proto,
            program,
            symbol,
            scalar_proto,
            diagnostic_site,
            frame_stacks,
            strict_functions,
            array_slots,
            site,
            plan,
            kind,
            baseline_numbers,
            baseline_ints,
            &defs,
            &ivs,
            "kiln_generic_block_",
            false,
            type_profile_record,
            &this_sites,
            &Speculation {
                arrays: HashSet::new(),
                calls: spec_calls.clone(),
                exits: false,
            },
        );
    } else {
        emit_ssa_declarations(output, graph, baseline_numbers, baseline_ints);
        emit_block_sequence(
            output,
            graph,
            proto,
            program,
            symbol,
            scalar_proto,
            diagnostic_site,
            frame_stacks,
            strict_functions,
            array_slots,
            site,
            plan,
            kind,
            baseline_numbers,
            baseline_ints,
            &defs,
            &ivs,
            "kiln_block_",
            false,
            type_profile_record,
            &this_sites,
            &Speculation::default(),
        );
    }

    // 5b. Answers through `&vN` go through one scratch slot, so the values stay in registers.
    {
        let (hoisted, used) = hoist_out_params(&output[body_start..]);
        if used {
            let declaration = "    kiln_val kiln_out = KILN_VAL_UNDEFINED;\n";
            output.truncate(body_start);
            output.push_str(declaration);
            output.push_str(&hoisted);
        }
    }

    // 5c. A function's sites are shared by its clones.
    let mut shared_sites = 0usize;
    if let Site::Function { index } = site {
        let (renamed, count) =
            share_sites(&output[body_start..], &format!("{symbol}_fn_{index}_site"));
        output.truncate(body_start);
        output.push_str(&renamed);
        shared_sites = count;
    }

    // 6. Unwind and refusal handlers
    match site {
        Site::Function { .. } if kind == EntryKind::Fast => {
            // A fast clone has no slow path left to unwind from; a body that still jumps to
            // one is not restartable, and `emit_fast_clone` says so.
        }
        Site::Function { .. } if matches!(kind, EntryKind::Typed { .. }) => {
            // A typed entry leaves its failure pending and returns the failure value. The
            // labels exist only where the body jumps to them: a body with nothing that throws
            // has no unwind, and no `thrown` to read.
            let fail = typed_fail(kind);
            let body = &output[body_start..];
            let unwinds = body.contains("goto kiln_unwind;");
            let refuses = body.contains("goto kiln_refusal;");
            if unwinds {
                output.push_str("kiln_unwind:;\n");
                writeln!(
                    output,
                    "    kiln_c99_typed_status = KILN_C99_THROWN; kiln_c99_typed_thrown = *thrown; return {fail};"
                )
                .unwrap();
            }
            if refuses {
                output.push_str("kiln_refusal:;\n");
                writeln!(
                    output,
                    "    kiln_c99_typed_status = KILN_C99_CONTROL_ERROR; return {fail};"
                )
                .unwrap();
            }
            let routed = route_typed_failures(&output[body_start..], fail);
            output.truncate(body_start);
            output.push_str(&routed);
        }
        Site::Function { .. } => {
            output.push_str("kiln_unwind:;\n");
            output.push_str("    return KILN_C99_THROWN;\n");
            if !scalar_proto {
                output.push_str("kiln_refusal:;\n");
                output.push_str(
                    "    *result = kiln_val_from_boundary(kiln_c99_control_diagnostic());\n",
                );
                output.push_str("    return KILN_C99_CONTROL_ERROR;\n");
            }
        }
        Site::Entry { .. } => {
            // Nothing above the entry point catches, so a throw that reaches here is uncaught:
            // it leaves as text in `*result`, the site it was refused at appended when there is
            // one. A label nothing jumps to is a warning, so each exists only when used.
            let unwinds = output[body_start..].contains("goto kiln_unwind;");
            let refuses = output[body_start..].contains("goto kiln_refusal;");
            if unwinds {
                output.push_str("kiln_unwind:;\n");
                if scalar_proto {
                    output.push_str("    return KILN_C99_THROWN;\n");
                } else {
                    output.push_str("    { kiln_value message = kiln_c99_text(*thrown); kiln_value site = kiln_c99_control_diagnostic(); if (message.tag == KILN_VALUE_STRING && site.tag == KILN_VALUE_STRING && site.len != 0) { kiln_value separator = kiln_c99_copy_text(\" at \" , 4); *result = kiln_c99_concat(kiln_c99_concat(message, separator), site); } else { *result = message; } }\n");
                    output.push_str("    return KILN_C99_THROWN;\n");
                }
            }
            if refuses {
                output.push_str("kiln_refusal:;\n");
                output.push_str("    *result = kiln_c99_control_diagnostic();\n");
                output.push_str("    return KILN_C99_CONTROL_ERROR;\n");
            }
        }
    }
    (shared_sites, this_site_count(graph))
}

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
fn emit_graph_op(
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
            if graph.scalar_alias_map.contains_key(value) {
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
                            "    {{ int64_t res; if (KILN_LIKELY(!__builtin_add_overflow(i{x}, 1LL, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res; d{v} = (double)res; }} else {{ d{v} = (double)i{x} + 1.0; i{v} = (int64_t)d{v}; }} v{v} = kiln_c99_val_from_double(d{v}); }}"
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
                            "    {{ int64_t res; if (KILN_LIKELY(!__builtin_sub_overflow(i{x}, 1LL, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res; d{v} = (double)res; }} else {{ d{v} = (double)i{x} - 1.0; i{v} = (int64_t)d{v}; }} v{v} = kiln_c99_val_from_double(d{v}); }}"
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
                            "    {{ int64_t res; if (KILN_LIKELY(!__builtin_add_overflow(i{l}, i{r}, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res; d{v} = (double)res; }} else {{ d{v} = (double)i{l} + (double)i{r}; i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0; }} v{v} = kiln_c99_val_from_double(d{v}); }}"
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
                            "    {{ int64_t res; if (KILN_LIKELY(!__builtin_sub_overflow(i{l}, i{r}, &res) && res >= -9007199254740991LL && res <= 9007199254740991LL)) {{ i{v} = res; d{v} = (double)res; }} else {{ d{v} = (double)i{l} - (double)i{r}; i{v} = isfinite(d{v}) ? (int64_t)d{v} : 0; }} v{v} = kiln_c99_val_from_double(d{v}); }}"
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
            if plan
                .known
                .get(callee)
                .is_some_and(|&target| direct_callable(&program.functions[target as usize])) =>
        {
            // The callee is the closure of a proto this unit compiled, so the body is called by
            // name: no callable test, no body-pointer comparison, and the receiver the language
            // gives a plain function, decided here. A site that proved every argument a number
            // enters the typed clone. What the direct path does not cover -- a `super()` in
            // flight, the depth limit -- takes the generic call, which answers the same way.
            let target = plan.known[callee];
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
            let receiver = if sloppy {
                "kiln_val recv = KILN_VAL_UNDEFINED; (void)kiln_c99_global_object_val(&recv);"
            } else {
                "kiln_val recv = KILN_VAL_UNDEFINED;"
            };
            let check = status_check(diagnostic_site, pc, label, unwind);
            // A function calling itself through its own binding is calling the very closure
            // that is running: its upvalue table is this one, and the callee value need not be
            // read at all.
            let own_upvalues = matches!(self_index, Some(index) if index == target);
            let upvalues = if own_upvalues {
                "upvalues".to_owned()
            } else {
                format!("((kiln_closure *)KILN_VAL_PTR(v{}))->upvalues", callee.0)
            };
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
                if needs.runtime {
                    call_args.push_str(", host, thrown");
                }
                for a in args {
                    call_args.push_str(&format!(", d{}", a.0));
                }
                let recv = if needs.receiver { receiver } else { "" };
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
            let candidates = method_candidates(program, key_str);
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
                    if needs.runtime {
                        call_args.push_str(", host, thrown");
                    }
                    for a in args {
                        call_args.push_str(&format!(", d{}", a.0));
                    }
                    let failed = format!(
                        "{{ int status = kiln_c99_typed_status; if (status == KILN_C99_THROWN) *thrown = kiln_c99_typed_thrown; {diag} {typed_check} }}"
                    );
                    if plan.typed_returns[i as usize] {
                        let twin = if typed { format!(" d{v} = kiln_typed_d;") } else { String::new() };
                        format!(
                            "if (plain->body == (kiln_body_fn){symbol}_fn_{i} && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ double kiln_typed_d = {symbol}_fn_{i}_t({call_args}); if (KILN_UNLIKELY(kiln_c99_typed_failed(kiln_typed_d))) {failed} else {{ v{v} = kiln_c99_val_from_double(kiln_typed_d);{twin} }} handled = 1; }} else "
                        )
                    } else {
                        format!(
                            "if (plain->body == (kiln_body_fn){symbol}_fn_{i} && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ kiln_val kiln_typed_r = {symbol}_fn_{i}_t({call_args}); if (KILN_UNLIKELY(kiln_typed_r == KILN_VAL_TYPED_FAIL)) {failed} else {{ v{v} = kiln_typed_r; }} handled = 1; }} else "
                        )
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
                int handled = 0; int kiln_twin = 0; (void)kiln_twin; \
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
            let publish = if proto_publishes_frame(target) {
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
            let (receiver_tier, receiver_learn) = match (this_site, self_index) {
                (Some(i), Some(index)) => (
                    format!(
                        "if (recv_row != NULL) {{ uint32_t rs = recv_row[{i}]; if (rs != UINT32_MAX && KILN_LIKELY(rs < recv_obj->slot_capacity)) {{ v{v} = recv_obj->slots[rs]; handled = 1; }} }} "
                    ),
                    if kind == EntryKind::Boxed {
                        format!(
                            " if (recv_k >= 0 && {symbol}_fn_{index}_recv_slot[recv_k][{i}] == UINT32_MAX && recv_obj->shape == {symbol}_fn_{index}_recv_shape[recv_k]) {symbol}_fn_{index}_recv_slot[recv_k][{i}] = kiln_c99_site_slot_for(&site, recv_obj->shape);"
                        )
                    } else {
                        String::new()
                    },
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

#[allow(clippy::too_many_arguments)]
fn emit_terminator(
    output: &mut String,
    graph: &FunctionGraph,
    current_block: &BasicBlock,
    terminator: &Terminator,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    unwind: &str,
    site: &Site<'_>,
    scalar_proto: bool,
    kind: EntryKind,
    block_prefix: &str,
    symbol: &str,
    type_profile_record: bool,
) {
    match terminator {
        Terminator::Return(value) => match site {
            Site::Function { index }
                if matches!(
                    kind,
                    EntryKind::Typed {
                        returns_number: true,
                        ..
                    }
                ) =>
            {
                if type_profile_record {
                    writeln!(output, "    {symbol}_fn_{index}_return_tag |= 1;").unwrap();
                }
                match value {
                    Some(val) if numbers.contains(val) => {
                        writeln!(output, "    return d{};", val.0).unwrap();
                    }
                    // The plan proved every reachable return a number; a return of anything
                    // else is in a block nothing reaches. It answers as a bug would, loudly.
                    _ => output.push_str("    goto kiln_refusal;\n"),
                }
            }
            Site::Function { .. } if matches!(kind, EntryKind::Typed { .. }) => match value {
                Some(val) => writeln!(output, "    return v{};", val.0).unwrap(),
                None => output.push_str("    return KILN_VAL_UNDEFINED;\n"),
            },
            Site::Function { index } => match value {
                Some(val) => {
                    if type_profile_record {
                        writeln!(
                            output,
                            "    {symbol}_fn_{index}_return_tag |= kiln_profile_tag_of(v{});",
                            val.0
                        )
                        .unwrap();
                    }
                    writeln!(output, "    *result = v{}; return KILN_C99_OK;", val.0).unwrap();
                }
                None => {
                    if type_profile_record {
                        writeln!(
                            output,
                            "    {symbol}_fn_{index}_return_tag |= kiln_profile_tag_of(KILN_VAL_UNDEFINED);"
                        )
                        .unwrap();
                    }
                    output.push_str("    *result = KILN_VAL_UNDEFINED; return KILN_C99_OK;\n");
                }
            },
            Site::Entry { exports, commonjs } => {
                // The entry's result crosses the embedding boundary, and a reference must not:
                // a callable leaving here becomes its text.
                match value {
                    Some(val) => {
                        writeln!(output, "    *result = kiln_val_to_boundary(v{});", val.0)
                            .unwrap();
                    }
                    None => output.push_str("    *result = kiln_c99_undefined();\n"),
                }
                if !scalar_proto {
                    output.push_str(
                        "    if (kiln_c99_crosses_boundary(*result)) *result = kiln_c99_text(*result);\n",
                    );
                }
                if *commonjs {
                    // `depth` and `thrown` exist only when the unit has callables. With none
                    // there is no getter that could have been installed on `module`, so this
                    // read cannot re-enter or throw and carries neither.
                    let (depth, thrown) = if scalar_proto {
                        ("0", "NULL")
                    } else {
                        ("depth", "thrown")
                    };
                    writeln!(
                        output,
                        "    {{ kiln_val mod_val = locals[2]; kiln_val module_val = (mod_val & KILN_TAG_MASK) == KILN_TAG_CELL_PREFIX ? *(kiln_val *)KILN_VAL_PTR(mod_val) : mod_val; int export_status = kiln_c99_get(host, kiln_val_to_boundary(module_val), \"exports\", 7, &exports[0], {depth}, {thrown}); if (export_status != KILN_C99_OK) return export_status; }}"
                    )
                    .unwrap();
                }
                // The slot is handed over exactly as it stands, box included: a mutable export
                // *is* its cell, which is what makes an import a live view of the binding.
                for (index, binding) in exports.iter().flat_map(|e| e.iter()).enumerate() {
                    writeln!(
                        output,
                        "    exports[{index}] = kiln_val_to_boundary(locals[{}]);",
                        binding.local_slot
                    )
                    .unwrap();
                }
                if type_profile_record {
                    writeln!(output, "    {symbol}_dump_type_profile();").unwrap();
                }
                output.push_str("    return KILN_C99_OK;\n");
            }
        },
        Terminator::Throw(val) => {
            writeln!(
                output,
                "    *thrown = kiln_val_to_boundary(v{}); {unwind}",
                val.0
            )
            .unwrap();
        }
        Terminator::Jump(target) => {
            emit_phi_assignments(output, graph, current_block.id, *target, numbers, ints);
            writeln!(output, "    goto {block_prefix}{};", target.0).unwrap();
        }
        Terminator::Branch {
            condition,
            true_block,
            false_block,
        } => {
            // A comparison of two proven ints branches on ints directly;
            // a comparison of two proven numbers branches on doubles directly;
            // anything else asks the value for its truthiness.
            let test = match defs.get(condition) {
                Some(GraphOp::Binary { op, left, right })
                    if ints.contains(left) && ints.contains(right) =>
                {
                    let sym = match op {
                        BinaryOp::LessThan => Some("<"),
                        BinaryOp::LessThanOrEqual => Some("<="),
                        BinaryOp::GreaterThan => Some(">"),
                        BinaryOp::GreaterThanOrEqual => Some(">="),
                        BinaryOp::StrictEqual | BinaryOp::LooseEqual => Some("=="),
                        BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => Some("!="),
                        _ => None,
                    };
                    sym.map(|sym| format!("i{} {sym} i{}", left.0, right.0))
                }
                Some(GraphOp::Binary { op, left, right })
                    if numbers.contains(left) && numbers.contains(right) =>
                {
                    let sym = match op {
                        BinaryOp::LessThan => Some("<"),
                        BinaryOp::LessThanOrEqual => Some("<="),
                        BinaryOp::GreaterThan => Some(">"),
                        BinaryOp::GreaterThanOrEqual => Some(">="),
                        BinaryOp::StrictEqual | BinaryOp::LooseEqual => Some("=="),
                        BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => Some("!="),
                        _ => None,
                    };
                    sym.map(|sym| format!("d{} {sym} d{}", left.0, right.0))
                }
                _ => None,
            }
            .unwrap_or_else(|| format!("kiln_c99_val_truthy(v{})", condition.0));
            writeln!(output, "    if ({test}) {{").unwrap();
            emit_phi_assignments(output, graph, current_block.id, *true_block, numbers, ints);
            writeln!(output, "        goto {block_prefix}{};", true_block.0).unwrap();
            output.push_str("    } else {\n");
            emit_phi_assignments(output, graph, current_block.id, *false_block, numbers, ints);
            writeln!(output, "        goto {block_prefix}{};", false_block.0).unwrap();
            output.push_str("    }\n");
        }
        Terminator::Suspend {
            value,
            spilled,
            pc,
            awaiting,
            ..
        } => {
            let mut spill = String::new();
            for (i, v) in spilled.iter().enumerate() {
                spill.push_str(&format!(" frame->stack[{i}] = v{};", v.0));
            }
            // Whether a handler covers this suspension. The handlers themselves are static
            // here, but the runtime's `return()` and `throw()` read this to decide whether the
            // body has a `finally` or `catch` to run, and so whether to re-enter it at all.
            let covered = i32::from(current_block.exception_handler.is_some());
            writeln!(
                output,
                "   {spill} frame->sp = {}; frame->resume = {pc}; frame->awaiting = {}; frame->handler_top = {covered}; *result = v{}; return KILN_C99_YIELDED;",
                spilled.len(),
                i32::from(*awaiting),
                value.0
            )
            .unwrap();
        }
        Terminator::Unreachable => {
            output.push_str("    goto kiln_refusal;\n");
        }
    }
}

fn emit_phi_assignments(
    output: &mut String,
    graph: &FunctionGraph,
    from: BlockId,
    target: BlockId,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
) {
    // A parallel move: every input is read before any phi is written, so two phis that swap
    // values (`[a, b] = [b, a]` across a back edge) do not clobber each other.
    let Some(target_block) = graph.block(target) else {
        return;
    };
    let mut moves: Vec<(u32, u32)> = Vec::new();
    for &(phi_vid, ref op) in &target_block.operations {
        if let GraphOp::Phi { inputs } = op {
            for &(pred, in_vid) in inputs {
                if pred == from && in_vid != phi_vid {
                    moves.push((phi_vid.0, in_vid.0));
                }
            }
        }
    }
    if moves.is_empty() {
        return;
    }
    output.push_str("        {");
    for (i, &(phi_v, from_v)) in moves.iter().enumerate() {
        write!(output, " kiln_val phi_t{i} = v{from_v};").unwrap();
        if numbers.contains(&ValueId(phi_v)) {
            write!(output, " double phi_td{i} = d{from_v};").unwrap();
        }
        if ints.contains(&ValueId(phi_v)) {
            write!(output, " int64_t phi_ti{i} = i{from_v};").unwrap();
        }
    }
    for (i, &(phi_v, _)) in moves.iter().enumerate() {
        write!(output, " v{phi_v} = phi_t{i};").unwrap();
        if numbers.contains(&ValueId(phi_v)) {
            write!(output, " d{phi_v} = phi_td{i};").unwrap();
        }
        if ints.contains(&ValueId(phi_v)) {
            write!(output, " i{phi_v} = phi_ti{i};").unwrap();
        }
    }
    output.push_str(" }\n");
}
