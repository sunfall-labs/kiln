use super::clones::{RESTART, bail_out, route_typed_failures, typed_fail};
use super::ops::emit_graph_op;
use super::sites::{receiver_prologue, share_sites, this_columns};
use super::{EntryKind, Site, Speculation, this_site_count, typed_signature};
use crate::direct::{CallPlan, direct_callable};
use crate::graph::{
    BasicBlock, BlockId, FunctionGraph, FunctionTypeProfile, GraphOp, InductionVariable, Proven,
    Terminator, ValueId, array_slots, candidate_number_parameters_with_profile,
    find_induction_variables, proven_ints_with_assumptions, proven_numbers_with_assumptions,
};
use crate::prelude::lookup_speculation_invariant;
use kiln_ir::{BinaryOp, Constant, ExportBinding, FunctionProto, Program};
use std::collections::HashSet;
use std::fmt::Write;

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
            // The parallel clone takes its parameters and nothing else -- that is its contract,
            // and `lib.rs` rejects the clone if its body reaches for more.
            let needs = if super::parallel_clone() {
                crate::direct::TypedNeeds::default()
            } else {
                plan.typed_needs[index]
            };
            let (result, params) = typed_signature(proto, returns_number, needs);
            // A restartable typed entry is the fast clone's body with a typed signature: no
            // runtime call survives in it, so it inlines where a method call enters it by
            // name, and the receiver the caller matched is the receiver it reads.
            let inline = if restartable {
                "static inline __attribute__((always_inline))"
            } else {
                "static inline"
            };
            let suffix = if super::parallel_clone() { "_p" } else { "_t" };
            writeln!(
                output,
                "{inline} {result} {symbol}_fn_{index}{suffix}({params}) {{"
            )
            .unwrap();
        }
    }

    // 3. Context & arguments setup. `ctx` is never null: every caller, the runtime's and the
    // emitted direct path alike, passes its own.
    let receiver_cache = if this_site_count(graph) > 0 {
        receiver_prologue(symbol, index, &this_columns(graph).1, kind)
    } else {
        String::new()
    };
    // `kiln_c99_inheriting_new_target` is set around a constructor invocation and restored
    // after it, so within one activation it is invariant; every call site read it through the
    // GOT, twice per call in a loop. Read once here.
    if matches!(kind, EntryKind::Typed { .. }) {
        output.push_str("    (void)depth;\n");
        output.push_str("    const int kiln_inheriting = kiln_c99_inheriting_new_target; (void)kiln_inheriting;\n");
        output.push_str(&receiver_cache);
    } else if kind == EntryKind::Fast {
        // No runtime call survives in a fast clone; what it reads is the receiver and the
        // arguments, and the context only to hand on when it restarts.
        output.push_str("    (void)upvalues;\n");
        output.push_str(
            "    kiln_val receiver = receiver_ptr ? *receiver_ptr : KILN_VAL_UNDEFINED;\n",
        );
        output.push_str("    const int kiln_inheriting = kiln_c99_inheriting_new_target; (void)kiln_inheriting;\n");
        output.push_str(&receiver_cache);
    } else if scalar_proto {
        output.push_str("    (void)ctx; (void)receiver_ptr; (void)upvalues;\n");
        output.push_str("    const int kiln_inheriting = kiln_c99_inheriting_new_target; (void)kiln_inheriting;\n");
    } else {
        output.push_str("    kiln_host *host = ctx->host;\n");
        output.push_str("    kiln_value *thrown = ctx->thrown;\n");
        output.push_str("    kiln_generator *frame = ctx->frame;\n");
        output.push_str("    int depth = ctx->depth;\n");
        output.push_str("    const int kiln_inheriting = kiln_c99_inheriting_new_target; (void)kiln_inheriting;\n");
        output.push_str(
            "    kiln_val receiver = receiver_ptr ? *receiver_ptr : KILN_VAL_UNDEFINED;\n",
        );
        output.push_str("    (void)receiver_ptr;\n");
        output.push_str(&receiver_cache);
        // Which of the two `arguments` objects a mention in this body, or in an arrow made
        // here, would build. Only such a body reads the frame global, so only such a body
        // writes it; a plain body leaves its caller's value alone.
        if plan.publishes_frame[index] {
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
                "static const kiln_shape *{symbol}_fn_{index}_recv_shape[4];\nstatic const kiln_shape *{symbol}_fn_{index}_recv_refused[4];\nstatic uint32_t {symbol}_fn_{index}_recv_slot[4][{this_sites}] = {{ {{{row}}}, {{{row}}}, {{{row}}}, {{{row}}} }};"
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
    output.push_str("    const int kiln_inheriting = kiln_c99_inheriting_new_target; (void)kiln_inheriting;\n");
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
    booleans: &HashSet<ValueId>,
) {
    if !booleans.is_empty() {
        let mut ids: Vec<u32> = booleans.iter().map(|v| v.0).collect();
        ids.sort_unstable();
        output.push_str("    int ");
        for (i, vid) in ids.iter().enumerate() {
            if i > 0 {
                output.push_str(", ");
            }
            write!(output, "b{vid} = 0").unwrap();
        }
        output.push_str(";\n");
    }
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
    booleans: &HashSet<ValueId>,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    ivs: &std::collections::HashMap<ValueId, InductionVariable>,
    block_prefix: &str,
    is_fast_clone: bool,
    type_profile_record: bool,
    this_sites: &std::collections::HashMap<ValueId, usize>,
    spec: &Speculation,
) {
    // A proven number nothing reads boxed is never boxed: its box statement is dropped from
    // the operation's text, and its twin is all that lives. Only in a body with one clone --
    // a versioned body's clones share every value, and the generic one reads them boxed. (A
    // fast clone that boxed only at its exits was tried on 2026-09-06: it measured nothing,
    // and a bundled `marked` answered wrongly under it.)
    let boxed = if block_prefix == "kiln_block_" {
        Some(boxed_uses(graph, numbers, booleans, array_slots, kind))
    } else {
        None
    };
    // A second read of the same `this` slot in a block, with nothing between the two that could
    // run code or write a property, reads what the first read. Only the row path copies -- an
    // accessor puts its object in dictionary mode, and a dictionary object has no row -- so the
    // generic path below it still calls a getter twice, as the language does.
    let this_copies: std::collections::HashMap<ValueId, ValueId> = {
        let mut copies = std::collections::HashMap::new();
        for block in &graph.blocks {
            let mut last: std::collections::HashMap<usize, ValueId> = std::collections::HashMap::new();
            for (vid, op) in &block.operations {
                let pure = match op {
                    GraphOp::Const { .. }
                    | GraphOp::LoadLocal(_)
                    | GraphOp::StoreLocal { .. }
                    | GraphOp::LoadCell(_)
                    | GraphOp::StoreCell { .. }
                    | GraphOp::LoadUpvalue(_)
                    | GraphOp::StoreUpvalue { .. }
                    | GraphOp::LoadBuiltin(_)
                    | GraphOp::LoadThis
                    | GraphOp::LoadGlobal(_)
                    | GraphOp::MarkUninitialized(_)
                    | GraphOp::CheckTdz { .. }
                    | GraphOp::GetProperty { .. }
                    | GraphOp::Phi { .. }
                    | GraphOp::MakeCell(_) => true,
                    GraphOp::Unary { value, .. } => numbers.contains(value),
                    GraphOp::Binary { left, right, .. } => numbers.contains(left) && numbers.contains(right),
                    _ => false,
                };
                if let (GraphOp::GetProperty { .. }, Some(col)) = (op, this_sites.get(vid)) {
                    match last.get(col) {
                        Some(prev) => {
                            copies.insert(*vid, *prev);
                        }
                        None => {
                            last.insert(*col, *vid);
                        }
                    }
                }
                if !pure {
                    last.clear();
                }
            }
        }
        copies
    };
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
                && direct_callable(
                    &program.functions[target as usize],
                    plan.publishes_frame[target as usize],
                )
                && matches!(kind, EntryKind::Typed { .. })
            {
                used.extend(args.iter().copied());
                let self_call = self_index == Some(target);
                let typed_site = plan.typed_sites.contains(vid);
                // An arrow's typed entry, when it needs a receiver, reads it out of the
                // callee's own `captured_this` (see the `Call` arm of `emit_graph_op`), so the
                // callee value must stay alive for that too -- not only for its upvalue table.
                // A self-call needs neither: the receiver a self-referencing arrow would read
                // this way is this frame's own, already in scope as `receiver`.
                let reads_callee = !self_call
                    && (!typed_site
                        || plan.typed_needs[target as usize].upvalues
                        || (program.functions[target as usize].arrow
                            && plan.typed_needs[target as usize].receiver));
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
            let target: &mut String = &mut op_text;
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
                this_copies.get(&vid).copied(),
                spec,
            );
            if kind.restarts() {
                op_text = bail_out(op, &op_text, RESTART);
            }
            if let Some(boxed) = &boxed
                && numbers.contains(&vid)
                && !boxed.contains(&vid)
            {
                op_text = op_text.replace(
                    &format!(" v{} = kiln_c99_val_from_double(d{});", vid.0, vid.0),
                    " ",
                );
            }
            output.push_str(&op_text);
            // A boolean's twin follows from its box: one compare, and a branch or a phi then
            // reads the twin.
            if booleans.contains(&vid) && !matches!(op, GraphOp::Phi { .. }) {
                writeln!(output, "    b{} = (v{} == KILN_VAL_TRUE);", vid.0, vid.0).unwrap();
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
            booleans,
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

/// The values proven booleans under `assumptions`: comparisons, `!`, `in`, `instanceof`, the
/// literals, and phis that agree.
fn booleans_with(
    graph: &FunctionGraph,
    assumptions: &std::collections::HashMap<ValueId, Proven>,
) -> HashSet<ValueId> {
    crate::graph::proven_types_with(graph, assumptions)
        .into_iter()
        .filter_map(|(v, kind)| (kind == Proven::Boolean).then_some(v))
        .collect()
}

/// The values some operation reads boxed. Arithmetic and comparison on two proven numbers
/// read the twins; a branch on such a comparison or on a proven boolean reads no box; a typed
/// entry returning a number returns the twin. Everything else reads the box, and a phi that is
/// read boxed reads its inputs boxed.
fn boxed_uses(
    graph: &FunctionGraph,
    numbers: &HashSet<ValueId>,
    booleans: &HashSet<ValueId>,
    array_slots: &HashSet<u32>,
    kind: EntryKind,
) -> HashSet<ValueId> {
    let twin_only = |op: &BinaryOp| {
        matches!(
            op,
            BinaryOp::Add
                | BinaryOp::Subtract
                | BinaryOp::Multiply
                | BinaryOp::Divide
                | BinaryOp::Exponentiate
                | BinaryOp::Remainder
                | BinaryOp::StrictEqual
                | BinaryOp::StrictNotEqual
                | BinaryOp::LooseEqual
                | BinaryOp::LooseNotEqual
                | BinaryOp::LessThan
                | BinaryOp::LessThanOrEqual
                | BinaryOp::GreaterThan
                | BinaryOp::GreaterThanOrEqual
        )
    };
    let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    let mut used: HashSet<ValueId> = HashSet::new();
    // A phi, a store to a slot that lives in SSA, and a dead-zone check of a proven number
    // are copies: they read their input boxed only if something reads them boxed.
    let mut phis: Vec<(ValueId, Vec<ValueId>)> = Vec::new();
    for block in &graph.blocks {
        for (vid, op) in &block.operations {
            match op {
                GraphOp::Phi { inputs } => {
                    phis.push((*vid, inputs.iter().map(|(_, v)| *v).collect()));
                }
                GraphOp::StoreLocal { slot, value } if !array_slots.contains(slot) => {
                    phis.push((*vid, vec![*value]));
                }
                GraphOp::CheckTdz { value, .. } if numbers.contains(value) => {
                    phis.push((*vid, vec![*value]));
                }
                GraphOp::Binary { op, left, right }
                    if twin_only(op) && numbers.contains(left) && numbers.contains(right) => {}
                _ => used.extend(op.operands()),
            }
        }
        match &block.terminator {
            Terminator::Branch { condition, .. } => {
                let compares_twins = matches!(
                    defs.get(condition),
                    Some(GraphOp::Binary { op, left, right })
                        if twin_only(op) && numbers.contains(left) && numbers.contains(right)
                );
                if !compares_twins && !booleans.contains(condition) {
                    used.insert(*condition);
                }
            }
            Terminator::Return(Some(v)) => {
                let typed_number = matches!(
                    kind,
                    EntryKind::Typed {
                        returns_number: true,
                        ..
                    }
                );
                if !(typed_number && numbers.contains(v)) {
                    used.insert(*v);
                }
            }
            Terminator::Throw(v) => {
                used.insert(*v);
            }
            Terminator::Suspend { value, spilled, .. } => {
                used.insert(*value);
                used.extend(spilled.iter().copied());
            }
            _ => {}
        }
    }
    loop {
        let before = used.len();
        for (phi, inputs) in &phis {
            if used.contains(phi) {
                used.extend(inputs.iter().copied());
            }
        }
        if used.len() == before {
            return used;
        }
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
    let this_sites = this_columns(graph).0;
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
    let baseline_booleans = booleans_with(
        graph,
        &baseline_numbers
            .iter()
            .map(|&v| (v, Proven::Number))
            .chain(baseline_ints.iter().map(|&v| (v, Proven::Int)))
            .collect(),
    );
    let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    // A read of `length` from a guarded array bounds a loop below 2^32.
    let length_bounds: HashSet<ValueId> = ops
        .iter()
        .filter_map(|(vid, op)| match op {
            GraphOp::GetProperty { object, name, .. }
                if spec_arrays.contains(object)
                    && matches!(program.constants.get(*name as usize), Some(Constant::String(s)) if s == "length") =>
            {
                Some(*vid)
            }
            _ => None,
        })
        .collect();
    let ivs = find_induction_variables(graph, &length_bounds);

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
        // An array's `length` is an integer below 2^32; any other read of a guarded array is
        // a number, as before.
        for (vid, op) in &ops {
            if let GraphOp::GetProperty { object, name, .. } = op
                && spec_arrays.contains(object)
            {
                let is_length = matches!(program.constants.get(*name as usize), Some(Constant::String(s)) if s == "length");
                candidate_assumptions.insert(*vid, if is_length { Proven::Int } else { Proven::Number });
            }
        }
        let speculative_numbers = proven_numbers_with_assumptions(graph, &candidate_assumptions);
        let mut all_numbers = baseline_numbers.clone();
        all_numbers.extend(&speculative_numbers);

        let speculative_ints = proven_ints_with_assumptions(graph, &candidate_assumptions);
        let mut all_ints = baseline_ints.clone();
        all_ints.extend(&speculative_ints);

        let all_booleans = booleans_with(graph, &candidate_assumptions);
        emit_ssa_declarations(output, graph, &all_numbers, &all_ints, &all_booleans);

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
            &all_booleans,
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
            &baseline_booleans,
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
        emit_ssa_declarations(
            output,
            graph,
            baseline_numbers,
            baseline_ints,
            &baseline_booleans,
        );
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
            &baseline_booleans,
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

#[allow(clippy::too_many_arguments)]
fn emit_terminator(
    output: &mut String,
    graph: &FunctionGraph,
    current_block: &BasicBlock,
    terminator: &Terminator,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
    booleans: &HashSet<ValueId>,
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
            emit_phi_assignments(
                output,
                graph,
                current_block.id,
                *target,
                numbers,
                ints,
                booleans,
            );
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
                // A proven boolean branches on its twin.
                _ if booleans.contains(condition) => Some(format!("b{}", condition.0)),
                _ => None,
            }
            .unwrap_or_else(|| format!("kiln_c99_val_truthy(v{})", condition.0));
            writeln!(output, "    if ({test}) {{").unwrap();
            emit_phi_assignments(
                output,
                graph,
                current_block.id,
                *true_block,
                numbers,
                ints,
                booleans,
            );
            writeln!(output, "        goto {block_prefix}{};", true_block.0).unwrap();
            output.push_str("    } else {\n");
            emit_phi_assignments(
                output,
                graph,
                current_block.id,
                *false_block,
                numbers,
                ints,
                booleans,
            );
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
    booleans: &HashSet<ValueId>,
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
        if booleans.contains(&ValueId(phi_v)) {
            write!(output, " int phi_tb{i} = b{from_v};").unwrap();
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
        if booleans.contains(&ValueId(phi_v)) {
            write!(output, " b{phi_v} = phi_tb{i};").unwrap();
        }
    }
    output.push_str(" }\n");
}
