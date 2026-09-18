use super::clones::{RESTART, bail_out, route_typed_failures, typed_fail};
use super::ops::emit_graph_op;
use super::sites::{self, receiver_prologue, share_sites, this_columns};
use super::{EntryKind, Site, Speculation, this_site_count, typed_signature};
use crate::direct::{CallPlan, direct_callable};
use crate::graph::{
    BasicBlock, BlockId, FunctionGraph, FunctionTypeProfile, GraphOp, InductionVariable, Proven,
    Terminator, ValueId, array_slots, candidate_array_parameters_with_profile,
    candidate_number_parameters_with_profile, find_induction_variables,
    proven_ints_with_assumptions, proven_numbers_with_assumptions,
};
use crate::prelude::lookup_speculation_invariant;
use kiln_ir::{BinaryOp, Constant, ExportBinding, FunctionProto, Instruction, Program, UnaryOp};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

// Clang spends more on thousands of distinct local declarations than on one constant-indexed
// frame. Below this point the brackets cost more source and object bytes than they save; above it
// the indexed spelling keeps large generated bodies smaller and modestly faster to compile.
const COMPACT_INDEXED_VALUE_THRESHOLD: u32 = 512;

/// Compact bodies use generic runtime calls rather than the optimized call paths. Determine
/// their context needs from the rendered body, including tail bridges and suspension setup,
/// instead of duplicating those emitters' requirements in an opcode classification.
///
/// Only these side-effect-free prologue declarations are candidates. Identifier matching is
/// deliberately conservative: names in comments or strings also retain a declaration. The
/// The compact operation macros that implicitly capture locals are accounted for below.
/// Noncompact clones are unchanged.
fn retain_used_context(output: &mut String, context: std::ops::Range<usize>) {
    const LOCALS: [(&str, &str); 6] = [
        ("host", "    kiln_host *host = ctx->host;\n"),
        ("thrown", "    kiln_value *thrown = ctx->thrown;\n"),
        ("frame", "    kiln_generator *frame = ctx->frame;\n"),
        ("depth", "    int depth = ctx->depth;\n"),
        (
            "kiln_inheriting",
            "    const int kiln_inheriting = kiln_c99_inheriting_new_target; (void)kiln_inheriting;\n",
        ),
        (
            "receiver",
            "    kiln_val receiver = receiver_ptr ? *receiver_ptr : KILN_VAL_UNDEFINED;\n",
        ),
    ];
    let mut used = [false; LOCALS.len()];
    let setup = &output[context.clone()];
    let body = &output[context.end..];
    if body.contains("KU_BINARY(")
        || body.contains("KU_READ_PROPERTY(")
        || body.contains("KU_WRITE_PROPERTY(")
        || body.contains("KU_CALL(")
        || body.contains("KU_CALL_METHOD(")
    {
        used[0] = true;
        used[1] = true;
        used[3] = true;
    }
    // Do not count a declaration's own name (or ctx->field) as a use. All other setup,
    // including receiver caches and arguments/frame publication, participates in the scan.
    let remaining_setup = setup
        .split_inclusive('\n')
        .filter(|line| !LOCALS.iter().any(|(_, declaration)| line == declaration));
    for text in remaining_setup.chain(std::iter::once(body)) {
        for identifier in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
            match identifier {
                "host" => used[0] = true,
                "thrown" => used[1] = true,
                "frame" => used[2] = true,
                "depth" => used[3] = true,
                "kiln_inheriting" => used[4] = true,
                "receiver" => used[5] = true,
                _ => {}
            }
        }
    }
    let retained = setup
        .split_inclusive('\n')
        .filter(|line| {
            !LOCALS
                .iter()
                .enumerate()
                .any(|(index, (_, declaration))| !used[index] && line == declaration)
        })
        .collect::<String>();
    output.replace_range(context, &retained);
}

pub fn retain_used_entry_context(output: &mut String, context: std::ops::Range<usize>) {
    const LOCALS: [(&str, &[&str]); 4] = [
        (
            "receiver",
            &["    kiln_val receiver = KILN_VAL_UNDEFINED; (void)receiver;\n"],
        ),
        ("depth", &["    int depth = 0;\n"]),
        (
            "thrown",
            &[
                "    kiln_value thrown_value = kiln_c99_undefined();\n",
                "    kiln_value *thrown = &thrown_value;\n",
            ],
        ),
        (
            "kiln_inheriting",
            &[
                "    const int kiln_inheriting = kiln_c99_inheriting_new_target; (void)kiln_inheriting;\n",
            ],
        ),
    ];
    let mut used = [false; LOCALS.len()];
    let setup = &output[context.clone()];
    let body = &output[context.end..];
    if body.contains("KU_BINARY(")
        || body.contains("KU_READ_PROPERTY(")
        || body.contains("KU_WRITE_PROPERTY(")
        || body.contains("KU_CALL(")
        || body.contains("KU_CALL_METHOD(")
    {
        used[1] = true;
        used[2] = true;
    }
    if body.contains("KU_LOAD_CELL(") || body.contains("KU_STATUS(") {
        used[2] = true;
    }
    let remaining_setup = setup.split_inclusive('\n').filter(|line| {
        !LOCALS
            .iter()
            .any(|(_, declarations)| declarations.contains(&line))
    });
    for text in remaining_setup.chain(std::iter::once(body)) {
        for identifier in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
            match identifier {
                "receiver" => used[0] = true,
                "depth" => used[1] = true,
                "thrown" => used[2] = true,
                "kiln_inheriting" => used[3] = true,
                _ => {}
            }
        }
    }
    let retained = setup
        .split_inclusive('\n')
        .filter(|line| {
            !LOCALS
                .iter()
                .enumerate()
                .any(|(index, (_, declarations))| !used[index] && declarations.contains(&line))
        })
        .collect::<String>();
    output.replace_range(context, &retained);
}

pub(crate) fn body_can_refuse(body: &str, compact: bool) -> bool {
    body.contains("goto kiln_refusal;")
        || (compact
            && (body.contains("KU_STATUS(")
                || body.contains("KU_LOAD_CELL(")
                || body.contains("KU_BINARY(")
                || body.contains("KU_READ_PROPERTY(")
                || body.contains("KU_WRITE_PROPERTY(")
                || body.contains("KU_CALL(")
                || body.contains("KU_CALL_METHOD(")))
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
        0,
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
    site_offset: usize,
) -> usize {
    let function_start = output.len();
    if kind == EntryKind::Boxed {
        let typed_sites = sites::collect_typed_call_sites(graph, program, plan, &plan.numbers);
        for (vid, typed_candidates) in typed_sites {
            for i in typed_candidates {
                let pfx = format!("{}_fn_{}_v{}_t_{}", symbol, index, vid.0, i);
                if plan.typed_needs[i as usize].row {
                    writeln!(
                        output,
                        "static kiln_val {pfx}_fn = 0;\nstatic const kiln_shape *{pfx}_shape = NULL;\nstatic const uint32_t *{pfx}_row = NULL;\nstatic uint64_t {pfx}_proto = 0;\nstatic uint64_t {pfx}_epoch = UINT64_MAX;\nstatic kiln_val *{pfx}_upvalues = NULL;"
                    )
                    .unwrap();
                } else {
                    writeln!(output, "static kiln_val {pfx}_fn = 0;").unwrap();
                }
            }
        }
    }
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
        EntryKind::Boxed => {
            let optnone = if proto.code.len() > 3000 {
                "__attribute__((optnone)) "
            } else {
                ""
            };
            let static_kw = if plan.should_chunk {
                ""
            } else {
                "static inline "
            };
            if plan.compact {
                writeln!(
                    output,
                    "{optnone}{static_kw}int {symbol}_fn_{index}(KU_PARAMS) {{"
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "{optnone}{static_kw}int {symbol}_fn_{index}(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues, const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {{"
                )
                .unwrap();
            }
        }
        EntryKind::Fast => {
            let static_kw = if plan.should_chunk {
                ""
            } else {
                "static inline "
            };
            let always_inline = if plan.should_chunk {
                ""
            } else {
                "__attribute__((always_inline)) "
            };
            if plan.compact {
                writeln!(
                    output,
                    "{static_kw}{always_inline}int {symbol}_fn_{index}_f(KU_PARAMS) {{"
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "{static_kw}{always_inline}int {symbol}_fn_{index}_f(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues, const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {{"
                )
                .unwrap();
            }
        }
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
            let has_user_calls = graph.blocks.iter().any(|b| {
                b.operations
                    .iter()
                    .any(|(_, op)| matches!(op, GraphOp::Call { .. }))
            });
            let inline = if plan.should_chunk {
                ""
            } else if restartable || (!needs.runtime && !has_user_calls) {
                "static inline __attribute__((always_inline)) "
            } else {
                "static inline "
            };
            let suffix = if super::parallel_clone() { "_p" } else { "_t" };
            writeln!(
                output,
                "{inline}{result} {symbol}_fn_{index}{suffix}({params}) {{"
            )
            .unwrap();
        }
    }

    // 3. Context & arguments setup. `ctx` is never null: every caller, the runtime's and the
    // emitted direct path alike, passes its own.
    let context_start = output.len();
    let receiver_cache = if !plan.generic_only && this_site_count(graph) > 0 {
        let (this_col_map, this_col_names) = this_columns(graph);
        let double_mask = if matches!(kind, EntryKind::Typed { .. }) {
            let mut mask = 0u32;
            for (vid, col_idx) in &this_col_map {
                if plan.numbers.contains(vid) && *col_idx < 32 {
                    mask |= 1u32 << col_idx;
                }
            }
            mask
        } else {
            0u32
        };
        receiver_prologue(symbol, index, &this_col_names, kind, double_mask)
    } else {
        String::new()
    };
    let typed_epoch_check: String = if matches!(kind, EntryKind::Boxed | EntryKind::Fast) {
        let typed_sites = sites::collect_typed_call_sites(graph, program, plan, &plan.numbers);
        let mut checks = String::new();
        for (vid, typed_candidates) in typed_sites {
            for i in typed_candidates {
                if plan.typed_needs[i as usize].row {
                    let pfx = format!("{}_fn_{}_v{}_t_{}", symbol, index, vid.0, i);
                    checks.push_str(&format!(
                        "    if (KILN_UNLIKELY({pfx}_epoch != kiln_c99_proto_epoch)) {pfx}_shape = NULL;\n"
                    ));
                }
            }
        }
        checks
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
        output.push_str(&typed_epoch_check);
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
        output.push_str(&typed_epoch_check);
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
    let context_end = output.len();
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
                if let Terminator::Suspend { pc, delegate, .. } = &block.terminator {
                    writeln!(output, "        case {pc}: goto kiln_resume_{pc};").unwrap();
                    if let Some((_, _, exit_pc)) = delegate {
                        writeln!(
                            output,
                            "        case {exit_pc}: goto kiln_resume_{exit_pc};"
                        )
                        .unwrap();
                    }
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
        context_start,
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
    if plan.compact && kind == EntryKind::Boxed {
        retain_used_context(output, context_start..context_end);
    }
    // The boxed clone, emitted first, declares the sites every clone of this function uses,
    // and the receiver-shape cache: four rows of shapes, a slot per read of `this` in each.
    if kind == EntryKind::Boxed && (shared_sites > 0 || this_sites > 0) {
        let mut declarations = String::new();
        if shared_sites > 0 {
            if plan.compact {
                writeln!(
                    declarations,
                    "#define {symbol}_fn_{index}_site (&{symbol}_sites[{site_offset}])"
                )
                .unwrap();
            } else {
                writeln!(
                    declarations,
                    "static kiln_c99_site {symbol}_fn_{index}_site[{shared_sites}] = {{0}};"
                )
                .unwrap();
            }
        }
        if this_sites > 0 {
            let row = vec!["UINT32_MAX"; this_sites].join(", ");
            writeln!(
                declarations,
                "static const kiln_shape *{symbol}_fn_{index}_recv_shape[4];\nstatic const kiln_shape *{symbol}_fn_{index}_recv_refused[4];\nstatic uint32_t {symbol}_fn_{index}_recv_slot[4][{this_sites}] = {{ {{{row}}}, {{{row}}}, {{{row}}}, {{{row}}} }};\nstatic const kiln_shape *{symbol}_fn_{index}_typed_recv_shape[4];\nstatic const kiln_shape *{symbol}_fn_{index}_typed_recv_refused[4];\nstatic uint32_t {symbol}_fn_{index}_typed_recv_slot[4][{this_sites}] = {{ {{{row}}}, {{{row}}}, {{{row}}}, {{{row}}} }};"
            )
            .unwrap();
        }
        output.insert_str(function_start, &declarations);
    }
    shared_sites
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
        0,
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
    _site_offset: usize,
) -> usize {
    let typed_sites = sites::collect_typed_call_sites(graph, program, plan, &plan.numbers);
    for (vid, typed_candidates) in &typed_sites {
        for &i in typed_candidates {
            let pfx = format!("{}_fn_entry_v{}_t_{}", symbol, vid.0, i);
            if plan.typed_needs[i as usize].row {
                writeln!(
                    output,
                    "    static kiln_val {pfx}_fn = 0; static const kiln_shape *{pfx}_shape = NULL; static const uint32_t *{pfx}_row = NULL; static uint64_t {pfx}_proto = 0; static uint64_t {pfx}_epoch = UINT64_MAX; static kiln_val *{pfx}_upvalues = NULL;"
                )
                .unwrap();
                writeln!(
                    output,
                    "    if (KILN_UNLIKELY({pfx}_epoch != kiln_c99_proto_epoch)) {pfx}_shape = NULL;"
                )
                .unwrap();
            } else {
                writeln!(output, "    static kiln_val {pfx}_fn = 0;").unwrap();
            }
        }
    }
    let entry_start = output.len();
    let (shared_sites, _) = emit_blocks(
        output,
        entry_start,
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
    shared_sites
}

#[allow(clippy::too_many_arguments)]
pub fn emit_async_entry_body(
    output: &mut String,
    graph: &FunctionGraph,
    proto: &FunctionProto,
    program: &Program,
    symbol: &str,
    diagnostic_site: &str,
    frame_stacks: &[usize],
    strict_functions: &[bool],
    plan: &CallPlan,
    type_profile_record: bool,
    _site_offset: usize,
) -> usize {
    let entry_start = output.len();
    writeln!(
        output,
        "static inline int {symbol}_entry_body(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues, const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {{"
    )
    .unwrap();
    output.push_str("    kiln_host *host = ctx->host; (void)host;\n");
    output.push_str("    kiln_value *thrown = ctx->thrown; (void)thrown;\n");
    output.push_str("    kiln_generator *frame = ctx->frame;\n");
    output.push_str("    int depth = ctx->depth; (void)depth;\n");
    // Every call site reads `kiln_inheriting` (see `emit_entry_prologue`); an async module body
    // is a body like any other and declares it too, or the module unit does not compile.
    output.push_str(
        "    const int kiln_inheriting = kiln_c99_inheriting_new_target; (void)kiln_inheriting;\n",
    );
    output.push_str(
        "    kiln_val receiver = receiver_ptr ? *receiver_ptr : KILN_VAL_UNDEFINED; (void)receiver;\n",
    );
    output.push_str("    (void)receiver_ptr; (void)upvalues; (void)args; (void)argc;\n");
    output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
    writeln!(output, "    kiln_val *locals = {symbol}_globals;").unwrap();
    output.push_str("    if (frame->started) {\n        switch (frame->resume) {\n");
    for block in &graph.blocks {
        if let Terminator::Suspend { pc, delegate, .. } = &block.terminator {
            writeln!(output, "        case {pc}: goto kiln_resume_{pc};").unwrap();
            if let Some((_, _, exit_pc)) = delegate {
                writeln!(
                    output,
                    "        case {exit_pc}: goto kiln_resume_{exit_pc};"
                )
                .unwrap();
            }
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
    let array_slots = (0..proto.local_count).collect::<HashSet<u32>>();
    let (shared_sites, _) = emit_blocks(
        output,
        entry_start,
        graph,
        proto,
        program,
        symbol,
        false,
        diagnostic_site,
        frame_stacks,
        strict_functions,
        &array_slots,
        &Site::AsyncEntry,
        plan,
        EntryKind::Boxed,
        None,
        type_profile_record,
    );
    output.push_str("}\n");
    shared_sites
}

fn emit_ssa_declarations(
    output: &mut String,
    graph: &FunctionGraph,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
    booleans: &HashSet<ValueId>,
    sunk: bool,
    compact: bool,
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
        if compact && graph.next_value_id >= COMPACT_INDEXED_VALUE_THRESHOLD {
            writeln!(output, "    kiln_val v[{}];", graph.next_value_id).unwrap();
        } else {
            output.push_str("    kiln_val ");
            for vid in 0..graph.next_value_id {
                if vid > 0 {
                    output.push_str(", ");
                }
                if compact {
                    write!(output, "v{vid}").unwrap();
                } else {
                    write!(output, "v{vid} = KILN_VAL_UNDEFINED").unwrap();
                }
            }
            output.push_str(";\n");
        }
    }
    if !graph.scalar_objects.is_empty() || (sunk && !graph.spread_scalar_objects.is_empty()) {
        let mut sorted_objs: Vec<_> = graph.scalar_objects.values().collect();
        if sunk {
            sorted_objs.extend(graph.spread_scalar_objects.values());
        }
        sorted_objs.sort_by_key(|s| s.alloc_id.0);
        sorted_objs.dedup_by_key(|s| s.alloc_id.0);
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
    if compact {
        let max_call_argc = graph
            .blocks
            .iter()
            .flat_map(|b| b.operations.iter())
            .map(|(_, op)| match op {
                GraphOp::Call { args, .. } | GraphOp::CallMethod { args, .. } => args.len(),
                _ => 0,
            })
            .max()
            .unwrap_or(0);
        if max_call_argc > 0 {
            writeln!(output, "    kiln_val ku_args[{max_call_argc}];").unwrap();
        }
    }
}

fn graph_value_use_counts(graph: &FunctionGraph) -> HashMap<ValueId, usize> {
    let mut counts = HashMap::new();
    for block in &graph.blocks {
        for (_, op) in &block.operations {
            for operand in op.operands() {
                *counts.entry(operand).or_insert(0) += 1;
            }
        }
        match &block.terminator {
            Terminator::Jump(_) | Terminator::Unreachable => {}
            Terminator::Branch { condition, .. }
            | Terminator::BranchNotNullish { condition, .. } => {
                *counts.entry(*condition).or_insert(0) += 1;
            }
            Terminator::Return(result) => {
                if let Some(value) = result {
                    *counts.entry(*value).or_insert(0) += 1;
                }
            }
            Terminator::Throw(value) => {
                *counts.entry(*value).or_insert(0) += 1;
            }
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                ..
            } => {
                *counts.entry(*value).or_insert(0) += 1;
                // A suspended result is also present in the frame spill list, but that entry is
                // bookkeeping for resumption rather than another semantic consumer. Other
                // spilled values remain real uses and must keep a candidate from being fused.
                for spilled_value in spilled {
                    if *spilled_value != *value {
                        *counts.entry(*spilled_value).or_insert(0) += 1;
                    }
                }
                if let Some((iterator, _, _)) = delegate {
                    *counts.entry(*iterator).or_insert(0) += 1;
                }
            }
        }
    }
    counts
}

fn await_resolve_fusion_arg(
    block: &BasicBlock,
    value: ValueId,
    op: &GraphOp,
    program: &Program,
    defs: &HashMap<ValueId, &GraphOp>,
    use_counts: &HashMap<ValueId, usize>,
) -> Option<ValueId> {
    let Terminator::Suspend {
        value: suspended,
        awaiting: true,
        ..
    } = &block.terminator
    else {
        return None;
    };
    if *suspended != value || use_counts.get(&value).copied().unwrap_or_default() != 1 {
        return None;
    }
    let Some(position) = block.operations.iter().position(|(vid, _)| *vid == value) else {
        return None;
    };
    // ResumeValue is a post-suspension placeholder and emits no pre-suspend C. Const and
    // LoadGlobal are direct reads that the emitter can place here without invoking user code;
    // calls, coercions, property access, and stores remain observable barriers.
    if block.operations[position + 1..].iter().any(|(_, op)| {
        !matches!(
            op,
            GraphOp::Const { .. } | GraphOp::LoadGlobal(_) | GraphOp::ResumeValue
        )
    }) {
        return None;
    }
    let GraphOp::CallMethod {
        object, name, args, ..
    } = op
    else {
        return None;
    };
    let Some(Constant::String(key)) = program.constants.get(*name as usize) else {
        return None;
    };
    if key != "resolve" || args.len() != 1 {
        return None;
    }
    let receiver_definition = block
        .operations
        .iter()
        .find_map(|(vid, definition)| (*vid == *object).then_some(definition))
        .or_else(|| defs.get(object).copied());
    if !matches!(
        receiver_definition,
        Some(GraphOp::LoadBuiltin(kiln_ir::Builtin::Promise))
    ) {
        return None;
    }
    Some(args[0])
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
    use_counts: &HashMap<ValueId, usize>,
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
    let boxed =
        if block_prefix == "kiln_generic_block_" || (plan.generic_only && numbers.is_empty()) {
            None
        } else {
            Some(boxed_uses(
                graph,
                program,
                numbers,
                ints,
                booleans,
                array_slots,
                kind,
                spec,
            ))
        };
    // A second read of the same `this` slot in a block, with nothing between the two that could
    // run code or write a property, reads what the first read. Only the row path copies -- an
    // accessor puts its object in dictionary mode, and a dictionary object has no row -- so the
    // generic path below it still calls a getter twice, as the language does.
    let this_copies: std::collections::HashMap<ValueId, ValueId> = {
        let mut copies = std::collections::HashMap::new();
        for block in &graph.blocks {
            let mut last: std::collections::HashMap<usize, ValueId> =
                std::collections::HashMap::new();
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
                    GraphOp::Binary { left, right, .. } => {
                        numbers.contains(left) && numbers.contains(right)
                    }
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
        Site::Entry { .. } | Site::AsyncEntry => None,
    };
    // A pure read nothing consumes is not emitted: it would only name something -- an upvalue
    // table, a slot -- that the body then appears to need. The callee read a direct self-call
    // has replaced is the common case.
    let mut used: HashSet<ValueId> = HashSet::new();
    for block in &graph.blocks {
        if block.id != graph.entry_block && block.predecessors.is_empty() {
            continue;
        }
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
            Terminator::Branch { condition, .. }
            | Terminator::BranchNotNullish { condition, .. } => {
                used.insert(*condition);
            }
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                ..
            } => {
                used.insert(*value);
                if let Some((iter, _, _)) = delegate {
                    used.insert(*iter);
                }
                used.extend(spilled.iter().copied());
            }
            _ => {}
        }
    }
    let dead_read = |vid: ValueId, op: &GraphOp| {
        !used.contains(&vid)
            && match op {
                GraphOp::LoadGlobal(_) | GraphOp::Const { .. } => true,
                GraphOp::LoadLocal(_) | GraphOp::LoadThis | GraphOp::LoadNewTarget
                    if plan.compact || plan.generic_only =>
                {
                    true
                }
                GraphOp::LoadCell(_) | GraphOp::LoadUpvalue(_) => plan.unchecked.contains(&vid),
                GraphOp::StoreLocal { slot, .. } => !array_slots.contains(slot),
                GraphOp::CheckTdz { value, .. } => {
                    graph.scalar_alias_map.contains_key(value)
                        || numbers.contains(value)
                        || plan.unchecked.contains(&vid)
                }
                GraphOp::MarkUninitialized(slot) => !array_slots.contains(slot),
                _ => false,
            }
    };
    for block in &graph.blocks {
        // A block nothing jumps to is the dead code the compiler leaves after a body's
        // explicit returns: emitted, it would be an unreachable dead block epilogue.
        if block.id != graph.entry_block && block.predecessors.is_empty() {
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
                    "    frame->delegate = kiln_c99_undefined(); frame->delegate_exit = 0;"
                )
                .unwrap();
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
            if let Terminator::Suspend {
                delegate: Some((_, exit_block, exit_pc)),
                spilled,
                ..
            } = &from.terminator
                && *exit_block == block.id
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
                writeln!(output, "kiln_resume_{exit_pc}:;").unwrap();
                writeln!(
                    output,
                    "    frame->delegate = kiln_c99_undefined(); frame->delegate_exit = 0;"
                )
                .unwrap();
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
            let mut materialize_phis = String::new();
            for &(phi_vid, ref op) in &block.operations {
                if let GraphOp::Phi { .. } = op {
                    if numbers.contains(&phi_vid) {
                        materialize_phis.push_str(&format!(
                            " v{} = kiln_c99_val_from_double(d{});",
                            phi_vid.0, phi_vid.0
                        ));
                    }
                }
            }
            writeln!(
                output,
                "    if (KILN_UNLIKELY(kiln_c99_guard_should_exit_in(kiln_spec_mode, &kiln_loop_guard_{}))) {{ {}{} goto kiln_generic_block_{}; }}",
                block.id.0,
                super::ops::materialize_all_sunk(graph, symbol, defs, array_slots),
                materialize_phis,
                block.id.0
            )
            .unwrap();
        }

        for &(vid, ref op) in &block.operations {
            if dead_read(vid, op) {
                continue;
            }
            let mut op_text = String::new();
            let target: &mut String = &mut op_text;
            let await_resolve_arg =
                await_resolve_fusion_arg(block, vid, op, program, defs, use_counts);
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
                boxed.as_ref(),
                await_resolve_arg,
                &used,
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
            if !used.contains(&vid)
                && matches!(
                    op,
                    GraphOp::Bridge {
                        instruction: Instruction::HostCall { .. },
                        ..
                    }
                )
            {
                op_text = op_text.replace(
                    &format!(" v{} = kiln_val_from_boundary(host_result);", vid.0),
                    "",
                );
            }
            // The fast clone left this operation to the generic clone: it runs here, on the
            // literal the exit rebuilt.
            if !spec.exits && spec.sinks.contains(&vid) {
                writeln!(output, "kiln_generic_resume_{}:;", vid.0).unwrap();
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
            plan.compact && graph.next_value_id >= COMPACT_INDEXED_VALUE_THRESHOLD,
            &used,
            boxed.as_ref(),
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
    program: &Program,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
    booleans: &HashSet<ValueId>,
    array_slots: &HashSet<u32>,
    kind: EntryKind,
    _spec: &Speculation,
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
        if block.id != graph.entry_block && block.predecessors.is_empty() {
            continue;
        }
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
                GraphOp::Unary { op, value }
                    if (matches!(op, UnaryOp::Plus | UnaryOp::ToNumeric | UnaryOp::Negate)
                        && numbers.contains(value))
                        || (*op == UnaryOp::BitwiseNot && ints.contains(value)) => {}
                GraphOp::Binary { op, left, right }
                    if (twin_only(op) && numbers.contains(left) && numbers.contains(right))
                        || (matches!(
                            op,
                            BinaryOp::BitwiseAnd
                                | BinaryOp::BitwiseOr
                                | BinaryOp::BitwiseXor
                                | BinaryOp::ShiftLeft
                                | BinaryOp::ShiftRight
                                | BinaryOp::UnsignedShiftRight
                        ) && ints.contains(left)
                            && ints.contains(right)) => {}
                GraphOp::GetComputed { object, key, .. } => {
                    used.insert(*object);
                    if !ints.contains(key) {
                        used.insert(*key);
                    }
                }
                GraphOp::SetComputed {
                    object, key, value, ..
                }
                | GraphOp::SetComputedStrict {
                    object, key, value, ..
                } => {
                    used.insert(*object);
                    used.insert(*value);
                    if !ints.contains(key) {
                        used.insert(*key);
                    }
                }
                GraphOp::CallMethod {
                    object, name, args, ..
                } => {
                    let builtin = match defs.get(object) {
                        Some(GraphOp::LoadBuiltin(b)) => Some(*b),
                        _ => None,
                    };
                    let key = program.constants.get(*name as usize).and_then(|c| match c {
                        Constant::String(s) => s.to_scalar_string().ok(),
                        _ => None,
                    });
                    let is_pure_math = matches!(builtin, Some(kiln_ir::Builtin::Math))
                        && match (key.as_deref(), args.len()) {
                            (
                                Some(
                                    "sqrt" | "abs" | "floor" | "ceil" | "round" | "trunc" | "sign"
                                    | "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "sinh"
                                    | "cosh" | "tanh" | "asinh" | "acosh" | "atanh" | "exp"
                                    | "expm1" | "log" | "log2" | "log10" | "log1p" | "cbrt"
                                    | "clz32" | "fround",
                                ),
                                1,
                            ) => true,
                            (Some("pow" | "min" | "max" | "atan2" | "hypot" | "imul"), 2) => true,
                            _ => false,
                        };
                    let is_number_predicate = matches!(builtin, Some(kiln_ir::Builtin::Number))
                        && matches!(
                            key.as_deref(),
                            Some("isFinite" | "isNaN" | "isInteger" | "isSafeInteger")
                        );
                    let is_array_predicate = matches!(builtin, Some(kiln_ir::Builtin::Array))
                        && key.as_deref() == Some("isArray");

                    if is_pure_math || is_number_predicate || is_array_predicate {
                        for arg in args {
                            if !numbers.contains(arg) {
                                used.insert(*arg);
                            }
                        }
                    } else {
                        used.insert(*object);
                        used.extend(args.iter().copied());
                    }
                }
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
            Terminator::BranchNotNullish { condition, .. } => {
                used.insert(*condition);
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
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                ..
            } => {
                used.insert(*value);
                if let Some((iter, _, _)) = delegate {
                    used.insert(*iter);
                }
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
    func_start: usize,
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
    let this_sites = if plan.generic_only {
        std::collections::HashMap::new()
    } else {
        this_columns(graph).0
    };
    let candidate_params = candidate_number_parameters_with_profile(graph, proto, func_profile);
    let candidate_array_params =
        candidate_array_parameters_with_profile(graph, proto, func_profile, program);
    let speculate_enabled = plan.speculate.unwrap_or(true);
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
    let mut spec_arrays: HashSet<ValueId> = ops
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
    // Parameter reads are eligible only when candidate selection proved a simple positional
    // binding. Include every read of that slot so the existing array property tier sees the same
    // receiver value throughout the body; the entry guard below checks the original argument.
    for &slot in &candidate_array_params {
        if let Some(&initial) = graph.initial_locals.get(slot) {
            spec_arrays.insert(initial);
        }
        for (vid, op) in &ops {
            if matches!(op, GraphOp::LoadLocal(load_slot) if *load_slot as usize == slot) {
                spec_arrays.insert(*vid);
            }
        }
    }
    let feeds_arithmetic = |vid: &ValueId| {
        ops.iter().any(|(_, op)| match op {
            GraphOp::Binary { op, left, right } => {
                (left == vid || right == vid)
                    && (matches!(
                        op,
                        BinaryOp::Add
                            | BinaryOp::Subtract
                            | BinaryOp::Multiply
                            | BinaryOp::Divide
                            | BinaryOp::Remainder
                            | BinaryOp::Exponentiate
                            | BinaryOp::LessThan
                            | BinaryOp::LessThanOrEqual
                            | BinaryOp::GreaterThan
                            | BinaryOp::GreaterThanOrEqual
                    ) || matches!(
                        op,
                        BinaryOp::BitwiseAnd
                            | BinaryOp::BitwiseOr
                            | BinaryOp::BitwiseXor
                            | BinaryOp::ShiftLeft
                            | BinaryOp::ShiftRight
                            | BinaryOp::UnsignedShiftRight
                    ))
            }
            _ => false,
        })
    };
    let spec_calls: HashSet<ValueId> = ops
        .iter()
        .filter_map(|(vid, op)| match op {
            GraphOp::CallMethod { .. } if !plan.numbers.contains(vid) => Some(*vid),
            _ => None,
        })
        .filter(feeds_arithmetic)
        .collect();
    // A property read whose answer feeds arithmetic is guessed to be a number and checked. An
    // object's field has no type the graph can prove -- a shape says which slots exist, never
    // what is in them -- so without this every `p.x * p.x` runs boxed, and so does everything
    // downstream of it: the accumulator of a loop over objects is never proven, and each turn
    // tests two tags and copies two doubles out of their boxes. The reads of a sunk literal
    // (`FunctionGraph::spread_sinks`) are the same guess over scalars rather than slots.
    let spec_fields: HashSet<ValueId> = ops
        .iter()
        .filter_map(|(vid, op)| match op {
            GraphOp::GetProperty { .. } if !plan.numbers.contains(vid) => Some(*vid),
            _ => None,
        })
        .filter(feeds_arithmetic)
        .collect();
    let spec_sinks: HashSet<ValueId> = graph
        .spread_sinks
        .values()
        .map(|sink| sink.spread)
        .chain(spec_fields.iter().copied())
        .collect();
    // Duplicating a large body makes both the generated source and the downstream C compiler's
    // memory grow linearly for a speedup that only matters inside small hot loops. Keep release
    // speculation bounded, and make compact/native-dev emission single-bodied throughout.
    const MAX_VERSIONED_OPERATIONS: usize = 192;
    let versioning = !plan.generic_only
        && ops.len() <= MAX_VERSIONED_OPERATIONS
        && speculate_enabled
        && !suspends
        && kind == EntryKind::Boxed
        && (!candidate_params.is_empty()
            || !candidate_array_params.is_empty()
            || !spec_arrays.is_empty()
            || !spec_calls.is_empty()
            || !spec_fields.is_empty()
            || !graph.spread_sinks.is_empty());

    let baseline_numbers = &plan.numbers;
    let baseline_ints = &plan.ints;
    let baseline_assumptions: std::collections::HashMap<ValueId, Proven> = baseline_numbers
        .iter()
        .map(|&v| (v, Proven::Number))
        .chain(baseline_ints.iter().map(|&v| (v, Proven::Int)))
        .chain(crate::direct::intrinsic_results(graph, program))
        .collect();
    let baseline_booleans = if plan.generic_only {
        HashSet::new()
    } else {
        booleans_with(graph, &baseline_assumptions)
    };
    let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    let use_counts = graph_value_use_counts(graph);
    // A read of `length` bounds a loop below 2^32.
    let length_bounds: HashSet<ValueId> = ops
        .iter()
        .filter_map(|(vid, op)| match op {
            GraphOp::GetProperty { name, .. }
                if matches!(program.constants.get(*name as usize), Some(Constant::String(s)) if s == "length") =>
            {
                Some(*vid)
            }
            _ => None,
        })
        .collect();
    let ivs = find_induction_variables(graph, &length_bounds);

    if versioning {
        // The fast clone's graph holds the sunk literals as scalar objects; the generic clone's
        // does not, and allocates them.
        let fast_graph = {
            let mut fast = graph.clone();
            fast.scalar_objects.extend(
                graph
                    .spread_scalar_objects
                    .iter()
                    .map(|(alloc, object)| (*alloc, object.clone())),
            );
            fast.scalar_alias_map
                .extend(graph.spread_alias_map.iter().map(|(a, r)| (*a, *r)));
            fast
        };
        let fast_graph = &fast_graph;
        // The speculative proof starts from the plan's: what the plan proved by a typed call
        // site or an intrinsic is not derivable from the graph alone, and a union of two
        // proofs is not a proof -- an operation whose operands each came from one would be
        // typed by neither and have no twin.
        let mut candidate_assumptions: std::collections::HashMap<ValueId, Proven> =
            baseline_assumptions.clone();
        for &slot in &candidate_params {
            if let Some(&vid) = graph.initial_locals.get(slot) {
                candidate_assumptions.insert(vid, Proven::Number);
            }
        }
        for vid in spec_calls.iter().chain(spec_fields.iter()) {
            candidate_assumptions.insert(*vid, Proven::Number);
        }
        // An array's `length` is an integer below 2^32; any other read of a guarded array is
        // a number, as before.
        for (vid, op) in &ops {
            if let GraphOp::GetProperty { object, name, .. } = op {
                let is_length = matches!(program.constants.get(*name as usize), Some(Constant::String(s)) if s == "length");
                if is_length
                    && (spec_arrays.contains(object)
                        || matches!(defs.get(object), Some(GraphOp::NewArray { .. })))
                {
                    candidate_assumptions.insert(*vid, Proven::Int);
                } else if spec_arrays.contains(object) {
                    candidate_assumptions.insert(*vid, Proven::Number);
                }
            }
        }
        let speculative_numbers =
            proven_numbers_with_assumptions(fast_graph, &candidate_assumptions);
        let mut all_numbers = baseline_numbers.clone();
        all_numbers.extend(&speculative_numbers);

        let speculative_ints = proven_ints_with_assumptions(fast_graph, &candidate_assumptions);
        let mut all_ints = baseline_ints.clone();
        all_ints.extend(&speculative_ints);

        let all_booleans = booleans_with(fast_graph, &candidate_assumptions);
        emit_ssa_declarations(
            output,
            fast_graph,
            &all_numbers,
            &all_ints,
            &all_booleans,
            true,
            false,
        );

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
        for &slot in &candidate_array_params {
            let invariant = lookup_speculation_invariant("parameter_array");
            writeln!(
                output,
                "    /* Speculation entry guard [{}]: {} */",
                invariant.name, invariant.language_rule
            )
            .unwrap();
            conds.push(format!(
                "(argc > {slot} && ((args[{slot}] & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && kiln_c99_gc_kind_of_ptr(KILN_VAL_PTR(args[{slot}])) == KILN_GC_ARRAY))"
            ));
            writeln!(
                output,
                "    const kiln_array *kiln_param_{slot}_arr = (argc > {slot}) ? (const kiln_array *)KILN_VAL_PTR(args[{slot}]) : NULL; (void)kiln_param_{slot}_arr;"
            )
            .unwrap();
            if let Some(&initial) = graph.initial_locals.get(slot) {
                writeln!(
                    output,
                    "    const kiln_array *kiln_spec_{v}_arr = kiln_param_{slot}_arr; (void)kiln_spec_{v}_arr;",
                    v = initial.0
                )
                .unwrap();
            }
            for (vid, op) in &ops {
                if matches!(op, GraphOp::LoadLocal(load_slot) if *load_slot as usize == slot) {
                    writeln!(
                        output,
                        "    const kiln_array *kiln_spec_{v}_arr = kiln_param_{slot}_arr; (void)kiln_spec_{v}_arr;",
                        v = vid.0
                    )
                    .unwrap();
                }
            }
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
                "    kiln_val kiln_spec_{v} = *(const kiln_val *)KILN_VAL_PTR(upvalues[{index}]);\n    const kiln_array *kiln_spec_{v}_arr = (const kiln_array *)KILN_VAL_PTR(kiln_spec_{v}); (void)kiln_spec_{v}_arr;",
                v = vid.0
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
            fast_graph,
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
            &use_counts,
            &ivs,
            "kiln_fast_block_",
            true,
            type_profile_record,
            &this_sites,
            &Speculation {
                arrays: spec_arrays.clone(),
                calls: spec_calls.clone(),
                fields: spec_fields.clone(),
                sinks: spec_sinks.clone(),
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
            &use_counts,
            &ivs,
            "kiln_generic_block_",
            false,
            type_profile_record,
            &this_sites,
            &Speculation {
                arrays: HashSet::new(),
                calls: spec_calls.clone(),
                fields: HashSet::new(),
                sinks: spec_sinks.clone(),
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
            false,
            plan.compact,
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
            &use_counts,
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

    // 5c. A function's sites are shared by its clones, and compact mode pools all sites into one table.
    let mut shared_sites = 0usize;
    match site {
        Site::Function { index } => {
            let (renamed, count) =
                share_sites(&output[body_start..], &format!("{symbol}_fn_{index}_site"));
            output.truncate(body_start);
            output.push_str(&renamed);
            shared_sites = count;
        }
        Site::Entry { .. } | Site::AsyncEntry if plan.compact => {
            let (renamed, count) =
                share_sites(&output[body_start..], &format!("{symbol}_entry_site"));
            output.truncate(body_start);
            output.push_str(&renamed);
            shared_sites = count;
        }
        _ => {}
    }

    // 5d. In compact mode, prune SSA value declarations that are never referenced in the body.
    if plan.compact
        && graph.next_value_id > 0
        && graph.next_value_id < COMPACT_INDEXED_VALUE_THRESHOLD
    {
        let body = &output[body_start..];
        if let Some(v_decl_idx) = body.find("    kiln_val v") {
            let v_decl_end = body[v_decl_idx..]
                .find(";\n")
                .map(|p| v_decl_idx + p + 2)
                .unwrap_or(body.len());
            let current_decl = &body[v_decl_idx..v_decl_end];
            let after_decl = &body[v_decl_end..];
            let mut used_v = HashSet::new();
            for token in after_decl.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
                if let Some(rest) = token.strip_prefix('v') {
                    if let Ok(id) = rest.parse::<u32>() {
                        if id < graph.next_value_id {
                            used_v.insert(id);
                        }
                    }
                }
            }
            let mut sorted_v: Vec<u32> = used_v.into_iter().collect();
            sorted_v.sort_unstable();
            let new_decl = if !sorted_v.is_empty() {
                let mut s = String::from("    kiln_val ");
                for (i, vid) in sorted_v.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    write!(s, "v{vid}").unwrap();
                }
                s.push_str(";\n");
                s
            } else {
                String::new()
            };
            if new_decl != current_decl {
                let abs_start = body_start + v_decl_idx;
                let abs_end = body_start + v_decl_end;
                output.replace_range(abs_start..abs_end, &new_decl);
            }
        }
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
            let func_text = &output[func_start..];
            let unwinds = func_text.contains("goto kiln_unwind;");
            let refuses = body_can_refuse(func_text, plan.compact);
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
            if plan.compact && !scalar_proto {
                output.push_str("KU_EPILOGUE()\n");
            } else {
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
        }
        Site::AsyncEntry => {
            let func_text = &output[func_start..];
            let unwinds = func_text.contains("goto kiln_unwind;");
            let refuses = body_can_refuse(func_text, plan.compact);
            if unwinds {
                output.push_str("kiln_unwind:;\n");
                output.push_str("    return KILN_C99_THROWN;\n");
            }
            if refuses {
                output.push_str("kiln_refusal:;\n");
                output.push_str(
                    "    *result = kiln_val_from_boundary(kiln_c99_control_diagnostic());\n",
                );
                output.push_str("    return KILN_C99_CONTROL_ERROR;\n");
            }
        }
        Site::Entry { exports, .. } => {
            // Entry unwinds cross either the internal module ABI or the final embedding boundary.
            // Graph module entries preserve the raw language value for dynamic-import rejection;
            // standalone entries format it at their embedding boundary.
            let func_text = &output[func_start..];
            let unwinds = func_text.contains("goto kiln_unwind;");
            let refuses = body_can_refuse(func_text, plan.compact);
            if unwinds {
                output.push_str("kiln_unwind:;\n");
                if exports.is_some() {
                    // A graph module must carry the original throw through its C ABI. The graph
                    // launcher settles dynamic import rejection with this value; formatting it
                    // here would turn an object or undefined into a diagnostic string.
                    output.push_str("    *result = *thrown;\n");
                    output.push_str("    return KILN_C99_THROWN;\n");
                } else if scalar_proto {
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
    if plan.compact && graph.next_value_id >= COMPACT_INDEXED_VALUE_THRESHOLD {
        let indexed = index_large_ssa_values(&output[body_start..]);
        output.truncate(body_start);
        output.push_str(&indexed);
    }
    (
        shared_sites,
        if plan.generic_only {
            0
        } else {
            this_site_count(graph)
        },
    )
}

/// Turn exact C identifiers such as `v123` into `v[123]` for a large body's single value array.
/// Strings, character literals, and comments are copied byte-for-byte so diagnostic/source text
/// remains observable JavaScript rather than becoming part of this C-only compaction.
fn index_large_ssa_values(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut output = String::with_capacity(source.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'"' || bytes[at] == b'\'' {
            let quote = bytes[at];
            let start = at;
            at += 1;
            while at < bytes.len() {
                if bytes[at] == b'\\' {
                    at = (at + 2).min(bytes.len());
                } else {
                    let done = bytes[at] == quote;
                    at += 1;
                    if done {
                        break;
                    }
                }
            }
            output.push_str(&source[start..at]);
            continue;
        }
        if bytes[at] == b'/' && at + 1 < bytes.len() && bytes[at + 1] == b'/' {
            let start = at;
            at += 2;
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
            output.push_str(&source[start..at]);
            continue;
        }
        if bytes[at] == b'/' && at + 1 < bytes.len() && bytes[at + 1] == b'*' {
            let start = at;
            at += 2;
            while at + 1 < bytes.len() && !(bytes[at] == b'*' && bytes[at + 1] == b'/') {
                at += 1;
            }
            at = (at + 2).min(bytes.len());
            output.push_str(&source[start..at]);
            continue;
        }
        if bytes[at].is_ascii_alphabetic() || bytes[at] == b'_' {
            let start = at;
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
            let identifier = &source[start..at];
            if let Some(index) = identifier.strip_prefix('v')
                && !index.is_empty()
                && index.bytes().all(|byte| byte.is_ascii_digit())
            {
                output.push_str("v[");
                output.push_str(index);
                output.push(']');
            } else {
                output.push_str(identifier);
            }
            continue;
        }
        let ch = source[at..]
            .chars()
            .next()
            .expect("the cursor is inside the source");
        output.push(ch);
        at += ch.len_utf8();
    }
    output
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
    indexed_values: bool,
    used: &HashSet<ValueId>,
    boxed: Option<&HashSet<ValueId>>,
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
            Site::AsyncEntry => match value {
                Some(val) => {
                    writeln!(output, "    *result = v{}; return KILN_C99_OK;", val.0).unwrap();
                }
                None => {
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
                if value.is_some() && !scalar_proto && exports.is_none() {
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
                // `kiln_val_to_boundary` dereferences a cell because the public ABI has no cell
                // tag. The graph evaluates dependencies before importers and refuses binding
                // cycles, so the initialized value is the observable answer here.
                for (index, binding) in exports.iter().flat_map(|e| e.iter()).enumerate() {
                    writeln!(
                        output,
                        "    exports[{index}] = kiln_val_binding_to_boundary(locals[{}]);",
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
                indexed_values,
                used,
                boxed,
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
                indexed_values,
                used,
                boxed,
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
                indexed_values,
                used,
                boxed,
            );
            writeln!(output, "        goto {block_prefix}{};", false_block.0).unwrap();
            output.push_str("    }\n");
        }
        Terminator::BranchNotNullish {
            condition,
            true_block,
            false_block,
        } => {
            let test = format!("!KILN_VAL_IS_NULLISH(v{})", condition.0);
            writeln!(output, "    if ({test}) {{").unwrap();
            emit_phi_assignments(
                output,
                graph,
                current_block.id,
                *true_block,
                numbers,
                ints,
                booleans,
                indexed_values,
                used,
                boxed,
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
                indexed_values,
                used,
                boxed,
            );
            writeln!(output, "        goto {block_prefix}{};", false_block.0).unwrap();
            output.push_str("    }\n");
        }
        Terminator::Suspend {
            value,
            spilled,
            pc,
            awaiting,
            delegate,
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
            let delegate_set = match delegate {
                Some((iter, _, exit_pc)) => {
                    format!(
                        " frame->delegate = kiln_val_to_boundary(v{}); frame->delegate_exit = {};",
                        iter.0, exit_pc
                    )
                }
                None => {
                    " frame->delegate = kiln_c99_undefined(); frame->delegate_exit = 0;".to_string()
                }
            };
            writeln!(
                output,
                "   {spill} frame->sp = {}; frame->resume = {pc}; frame->awaiting = {}; frame->handler_top = {covered};{delegate_set} *result = v{}; return KILN_C99_YIELDED;",
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
    indexed_values: bool,
    used: &HashSet<ValueId>,
    _boxed: Option<&HashSet<ValueId>>,
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
                if pred == from && in_vid != phi_vid && used.contains(&phi_vid) {
                    moves.push((phi_vid.0, in_vid.0));
                }
            }
        }
    }
    if moves.is_empty() {
        return;
    }
    if indexed_values && moves.len() >= 4 {
        // Merge phis are allocated together, so their destination IDs are normally one
        // contiguous run. Large bundled module initializers can have hundreds of locals and a
        // branch for every `x ||= value`; spelling both ID arrays at every edge made those tiny
        // branches account for megabytes of generated C. Copy consecutive source runs through
        // the same temporary array instead. The temporary preserves parallel-move semantics even
        // for a back edge whose source and destination ranges overlap.
        let consecutive_destinations = moves.windows(2).all(|pair| pair[1].0 == pair[0].0 + 1);
        if consecutive_destinations {
            writeln!(output, "        {{ kiln_val phi_tmp[{}];", moves.len()).unwrap();
            let mut start = 0usize;
            while start < moves.len() {
                let mut end = start + 1;
                while end < moves.len() && moves[end].1 == moves[end - 1].1 + 1 {
                    end += 1;
                }
                let len = end - start;
                if len >= 3 {
                    writeln!(
                        output,
                        "        memcpy(phi_tmp + {start}, v + {}, (size_t){len} * sizeof *phi_tmp);",
                        moves[start].1
                    )
                    .unwrap();
                } else {
                    for (offset, &(_, source)) in moves[start..end].iter().enumerate() {
                        writeln!(output, "        phi_tmp[{}] = v[{source}];", start + offset)
                            .unwrap();
                    }
                }
                start = end;
            }
            writeln!(
                output,
                "        memcpy(v + {}, phi_tmp, (size_t){} * sizeof *phi_tmp); }}",
                moves[0].0,
                moves.len()
            )
            .unwrap();
            return;
        }
        let from = moves
            .iter()
            .map(|(_, from)| from.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let to = moves
            .iter()
            .map(|(to, _)| to.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(output, "        {{ static const uint32_t phi_from[{}] = {{ {from} }}; static const uint32_t phi_to[{}] = {{ {to} }}; kiln_val phi_tmp[{}]; for (size_t phi = 0; phi < (size_t){}; ++phi) phi_tmp[phi] = v[phi_from[phi]]; for (size_t phi = 0; phi < (size_t){}; ++phi) v[phi_to[phi]] = phi_tmp[phi]; }}", moves.len(), moves.len(), moves.len(), moves.len(), moves.len()).unwrap();
        return;
    }
    if moves.len() >= 4 && numbers.is_empty() && ints.is_empty() && booleans.is_empty() {
        let values = moves
            .iter()
            .map(|(_, from)| format!("v{from}"))
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            output,
            "        {{ kiln_val phi_tmp[{}] = {{ {values} }};",
            moves.len()
        )
        .unwrap();
        for (index, (to, _)) in moves.iter().enumerate() {
            write!(output, " v{to} = phi_tmp[{index}];").unwrap();
        }
        output.push_str(" }\n");
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

#[cfg(test)]
mod tests {
    use super::{index_large_ssa_values, retain_used_context};

    #[test]
    fn compact_operation_macros_retain_the_context_their_expansions_capture() {
        let setup = "    kiln_host *host = ctx->host;\n    kiln_value *thrown = ctx->thrown;\n    kiln_generator *frame = ctx->frame;\n    int depth = ctx->depth;\n    kiln_val receiver = receiver_ptr ? *receiver_ptr : KILN_VAL_UNDEFINED;\n";
        let mut source =
            format!("{setup}    KU_BINARY(v2, 1, v0, v1, 3, \"Binary\", goto kiln_unwind;);\n");
        retain_used_context(&mut source, 0..setup.len());
        assert!(source.contains("kiln_host *host = ctx->host;"));
        assert!(source.contains("kiln_value *thrown = ctx->thrown;"));
        assert!(source.contains("int depth = ctx->depth;"));
        assert!(!source.contains("kiln_generator *frame = ctx->frame;"));
        assert!(!source.contains("kiln_val receiver = receiver_ptr"));
    }

    #[test]
    fn large_ssa_indexing_changes_only_c_identifiers() {
        let source = "v12 = v3; const char *s = \"v4 ☃\"; /* v5 */ // v6\nvalue7 = v8;";
        assert_eq!(
            index_large_ssa_values(source),
            "v[12] = v[3]; const char *s = \"v4 ☃\"; /* v5 */ // v6\nvalue7 = v[8];"
        );
    }
}
