//! C99 code generation directly from the SSA Value Graph.
//!
//! As specified in Phase 1 Step 1.3 of `fastest-native-plan.md` and ADR 0056:
//! - Eliminates the memory-bound `stack[]` array and `sp` stack pointer.
//! - Emits SSA intermediate values directly as C local variables (`kiln_val v0`, `v1`, ...).
//! - Retains `locals[]` only when needed for closure cells or generator suspension frames.
//! - Materializes call arguments into local stack arrays at call sites only, avoiding whole-body address-taking.

use crate::direct::{CallPlan, direct_callable};
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
use kiln_ir::{BinaryOp, Constant, ExportBinding, FunctionProto, Program, UnaryOp};
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
    /// The parameters arrive as `double`s. When the plan proved the return a number, the result
    /// leaves as one too, through `double *result`; otherwise through `kiln_val *result`.
    Typed {
        returns_number: bool,
    },
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

/// The typed entry's parameter list: what a direct call has in registers, the parameters as
/// the doubles they are, and the result as one where the plan proved it.
pub fn typed_signature(proto: &FunctionProto, returns_number: bool) -> String {
    let result = if returns_number {
        "double *result"
    } else {
        "kiln_val *result"
    };
    let params: String = (0..proto.parameter_count)
        .map(|k| format!(", double p{k}"))
        .collect();
    format!(
        "kiln_host *host, kiln_value *thrown, int depth, kiln_val *upvalues, kiln_val receiver, {result}{params}"
    )
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
        EntryKind::Typed { returns_number } => {
            writeln!(
                output,
                "static inline int {symbol}_fn_{index}_t({}) {{",
                typed_signature(proto, returns_number)
            )
            .unwrap();
        }
    }

    // 3. Context & arguments setup. `ctx` is never null: every caller, the runtime's and the
    // emitted direct path alike, passes its own.
    if matches!(kind, EntryKind::Typed { .. }) {
        output.push_str(
            "    (void)upvalues; (void)receiver; (void)host; (void)thrown; (void)depth;\n",
        );
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
) {
    let self_index = match site {
        Site::Function { index } => Some(*index),
        Site::Entry { .. } => None,
    };
    for block in &graph.blocks {
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
                "    if (KILN_UNLIKELY(kiln_c99_guard_should_exit(&kiln_loop_guard_{}))) goto kiln_generic_block_{};",
                block.id.0, block.id.0
            )
            .unwrap();
        }

        for &(vid, ref op) in &block.operations {
            emit_graph_op(
                output,
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
            );
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
) {
    let body_start = output.len();
    let candidate_params = candidate_number_parameters_with_profile(graph, proto, func_profile);
    let speculate_enabled = std::env::var("KILN_SPECULATE").as_deref() != Ok("0");
    let suspends = graph
        .blocks
        .iter()
        .any(|b| matches!(b.terminator, Terminator::Suspend { .. }))
        || graph.seam_block.is_some();
    let versioning =
        speculate_enabled && !candidate_params.is_empty() && !suspends && kind == EntryKind::Boxed;

    let baseline_numbers = &plan.numbers;
    let baseline_ints = &plan.ints;
    let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    let ivs = find_induction_variables(graph);

    if versioning {
        let candidate_assumptions: std::collections::HashMap<ValueId, Proven> = candidate_params
            .iter()
            .filter_map(|&slot| {
                graph
                    .initial_locals
                    .get(slot)
                    .map(|&vid| (vid, Proven::Number))
            })
            .collect();
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
        let mut conds = vec![format!(
            "!kiln_c99_guard_should_exit(&kiln_guard_entry_{index})"
        )];
        for &slot in &candidate_params {
            conds.push(format!(
                "(argc > {slot} && KILN_VAL_IS_DOUBLE(args[{slot}]))"
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
        );
    }

    // 6. Unwind and refusal handlers
    match site {
        Site::Function { .. } => {
            output.push_str("kiln_unwind:;\n");
            output.push_str("    return KILN_C99_THROWN;\n");
            if !scalar_proto {
                // The diagnostic travels in the realm's control record, which the entry point
                // reads; a boxed body also carries it in its result, a double result cannot.
                output.push_str("kiln_refusal:;\n");
                if !matches!(
                    kind,
                    EntryKind::Typed {
                        returns_number: true
                    }
                ) {
                    output.push_str(
                        "    *result = kiln_val_from_boundary(kiln_c99_control_diagnostic());\n",
                    );
                }
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
            match fast {
                Some(fast) => writeln!(
                    output,
                    "    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{l}) && KILN_VAL_IS_NUMBER(v{r}))) {{ double a; double b; memcpy(&a, &v{l}, sizeof(a)); memcpy(&b, &v{r}, sizeof(b)); v{v} = {fast}; }} else {{ {slow} }}"
                )
                .unwrap(),
                None => writeln!(output, "    {{ {slow} }}").unwrap(),
            }
            if typed_int {
                writeln!(output, "    if (KILN_VAL_IS_NUMBER(v{v})) {{ memcpy(&d{v}, &v{v}, sizeof(d{v})); i{v} = (int64_t)d{v}; }}").unwrap();
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
            let receiver = if strict_functions[target as usize] {
                ""
            } else {
                " (void)kiln_c99_global_object_val(&recv);"
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
            let (direct, twin) = if plan.typed_sites.contains(&vid) {
                let doubles: String = args.iter().map(|a| format!(", d{}", a.0)).collect();
                if plan.typed_returns[target as usize] {
                    // The result comes back as the double it is; the box is made here and
                    // deleted by the C compiler wherever nothing reads it untyped.
                    (
                        format!(
                            "int status = {symbol}_fn_{target}_t(host, thrown, depth + 1, {upvalues}, recv, &d{v}{doubles}); v{v} = kiln_c99_val_from_double(d{v});"
                        ),
                        String::new(),
                    )
                } else {
                    (
                        format!(
                            "int status = {symbol}_fn_{target}_t(host, thrown, depth + 1, {upvalues}, recv, &v{v}{doubles});"
                        ),
                        String::new(),
                    )
                }
            } else {
                let twin = if typed {
                    format!(" memcpy(&d{v}, &v{v}, sizeof(d{v}));")
                } else {
                    String::new()
                };
                (
                    format!(
                        "kiln_val call_args[{n}] = {{ {init} }}; kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; int status = {symbol}_fn_{target}(&ctx_call, &v{v}, {upvalues}, &recv, {argc} > 0 ? call_args : NULL, (size_t){argc});",
                        n = argc.max(1)
                    ),
                    twin,
                )
            };
            writeln!(
                output,
                "    if (KILN_LIKELY(!kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH)) {{ kiln_val recv = KILN_VAL_UNDEFINED;{receiver} {direct} if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status;{twin} }} else {{ kiln_val call_args[{n}] = {{ {init} }}; int status = kiln_c99_val_call(host, v{c}, {argc} > 0 ? call_args : NULL, (size_t){argc}, depth, thrown, \"{diagnostic_site}\", (size_t){pc}, \"{label}\", &v{v}); {check}{twin} }}",
                n = argc.max(1),
                c = callee.0
            )
            .unwrap();
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
                    let keyword = if static_branches.is_empty() {
                        "if"
                    } else {
                        " else if"
                    };
                    static_branches.push_str(&format!(
                        "{keyword} (plain->body == (kiln_body_fn){fn_name}) {{ status = {fn_name}(&ctx_call, &call_result, plain->upvalues, &recv, call_args, (size_t){argc}); }}"
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
                                     kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; \
                                     kiln_val prod = KILN_VAL_UNDEFINED; \
                                     status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)3); \
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
                                     kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; \
                                     kiln_val prod = KILN_VAL_UNDEFINED; \
                                     status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)3); \
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
                                     kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; \
                                     kiln_val prod = KILN_VAL_UNDEFINED; \
                                     int status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)4); \
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
            let n = argc.max(1);

            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; \
                kiln_val call_args[{n}] = {{ {init} }}; \
                int handled = 0; \
                {math_s}{push_s}{regex_s}{map_s}{filter_s}{reduce_s}{slice_s}{join_s}{split_s}{index_of_s}{substring_s} \
                if (!handled && site.pic_count > 0 && KILN_VAL_IS_OBJECT(v{o})) {{ \
                    const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(v{o}); \
                    kiln_val cached_fn = 0; \
                    if (site.pic_count >= 1 && obj->shape == site.entries[0].shape) cached_fn = site.entries[0].method; \
                    else if (site.pic_count >= 2 && obj->shape == site.entries[1].shape) cached_fn = site.entries[1].method; \
                    else if (site.pic_count >= 3 && obj->shape == site.entries[2].shape) cached_fn = site.entries[2].method; \
                    else if (site.pic_count >= 4 && obj->shape == site.entries[3].shape) cached_fn = site.entries[3].method; \
                    if (cached_fn != 0 && KILN_VAL_IS_CLOSURE(cached_fn)) {{ \
                        kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(cached_fn); \
                        if (!plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                            kiln_val vreceiver = plain->arrow ? plain->captured_this : v{o}; \
                            if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vreceiver); \
                            kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; \
                            v{v} = KILN_VAL_UNDEFINED; \
                            int status = plain->body(&ctx_call, &v{v}, plain->upvalues, &vreceiver, call_args, (size_t){argc}); \
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
                }} }}\
                {typed_sync}",
                o = object.0,
                typed_sync = if typed { format!("\n    memcpy(&d{v}, &v{v}, sizeof(d{v}));") } else { String::new() }
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
            writeln!(
                output,
                "    {{ kiln_val call_stack[{n}] = {{ {init} }}; int status = kiln_c99_val_new(host, call_stack[0], {argc} > 0 ? &call_stack[1] : NULL, (size_t){argc}, depth, thrown, &v{v}); {check} }}",
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
            let length_check = if key_str == "length" {
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
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; \
                int handled = 0; \
                {length_check}\
                if (!handled && site.pic_count > 0 && KILN_VAL_IS_OBJECT(v{o})) {{ \
                    const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(v{o}); \
                    uint32_t slot = UINT32_MAX; \
                    if (site.pic_count >= 1 && obj->shape == site.entries[0].shape) slot = site.entries[0].slot; \
                    else if (site.pic_count >= 2 && obj->shape == site.entries[1].shape) slot = site.entries[1].slot; \
                    else if (site.pic_count >= 3 && obj->shape == site.entries[2].shape) slot = site.entries[2].slot; \
                    else if (site.pic_count >= 4 && obj->shape == site.entries[3].shape) slot = site.entries[3].slot; \
                    if (slot != UINT32_MAX && KILN_LIKELY(slot < obj->slot_capacity)) {{ \
                        v{v} = obj->slots[slot]; \
                        handled = 1; \
                    }} \
                }} \
                if (!handled) {{ \
                    int status = kiln_c99_val_read_property(host, v{o}, {symbol}_key_{name}, {key_len}, depth, thrown, &site, &v{v}); \
                    if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) kiln_c99_note_get_property_refusal(kiln_val_to_boundary(v{o})); \
                    {check} \
                }} }}\
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
                if (site.pic_count > 0 && KILN_VAL_IS_OBJECT(v{o})) {{ \
                    kiln_object *obj = (kiln_object *)KILN_VAL_PTR(v{o}); \
                    uint32_t slot = UINT32_MAX; \
                    if (site.pic_count >= 1 && obj->shape == site.entries[0].shape && site.entries[0].transition_to == NULL) slot = site.entries[0].slot; \
                    else if (site.pic_count >= 2 && obj->shape == site.entries[1].shape && site.entries[1].transition_to == NULL) slot = site.entries[1].slot; \
                    else if (site.pic_count >= 3 && obj->shape == site.entries[2].shape && site.entries[2].transition_to == NULL) slot = site.entries[2].slot; \
                    else if (site.pic_count >= 4 && obj->shape == site.entries[3].shape && site.entries[3].transition_to == NULL) slot = site.entries[3].slot; \
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
                        size_t slot = (size_t)idx; \
                        const kiln_array *arr = (const kiln_array *)ptr; \
                        if (KILN_LIKELY(idx >= 0.0 && idx == (double)slot && slot < arr->count && arr->elements[slot] != KILN_VAL_HOLE)) {{ \
                            v{v} = arr->elements[slot]; \
                            handled = 1; \
                        }} \
                    }} \
                }} else if (KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_NUMBER(v{k})) {{ \
                    double idx; \
                    memcpy(&idx, &v{k}, sizeof(idx)); \
                    size_t slot = (size_t)idx; \
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
                        size_t slot = (size_t)idx; \
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
                        returns_number: true
                    }
                ) =>
            {
                if type_profile_record {
                    writeln!(output, "    {symbol}_fn_{index}_return_tag |= 1;").unwrap();
                }
                match value {
                    Some(val) if numbers.contains(val) => {
                        writeln!(output, "    *result = d{}; return KILN_C99_OK;", val.0).unwrap();
                    }
                    // The plan proved every reachable return a number; a return of anything
                    // else is in a block nothing reaches. It answers as a bug would, loudly.
                    _ => output.push_str("    goto kiln_refusal;\n"),
                }
            }
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
