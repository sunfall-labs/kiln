use super::clones::{RESTART, bail_out, route_typed_failures, typed_fail};
use super::ops::emit_graph_op;
use super::sites::{self, receiver_prologue, share_sites, this_columns, typed_row_claimer};
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

/// Module globals live in the realm's block for this unit (ADR 0070, realms issue 04 step 5), and
/// `{symbol}_globals` finds it through a generation-checked per-thread cache. A body that reads or
/// writes them asks once, at entry, and indexes the pointer it got: within one activation the
/// realm cannot change, and a call between two reads would otherwise make the compiler ask again.
/// A resumed generator or async body re-enters from the top, so it asks again on every resumption.
fn cache_module_globals(output: &mut String, context_start: usize, symbol: &str) {
    let needle = format!("{symbol}_globals[");
    if !output[context_start..].contains(&needle) {
        return;
    }
    let body = output[context_start..].replace(&needle, "kiln_mg[");
    output.truncate(context_start);
    writeln!(output, "    kiln_val *const kiln_mg = {symbol}_globals;").unwrap();
    output.push_str(&body);
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
                writeln!(
                    output,
                    "static kiln_c99_method_path {pfx}_path = KILN_C99_UNLEARNED_METHOD_PATH;"
                )
                .unwrap();
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
                    "{optnone}{static_kw}int {symbol}_fn_{index}(kiln_call_context *ctx, kiln_val * restrict result, kiln_val * restrict upvalues, const kiln_val * restrict receiver_ptr, const kiln_val * restrict args, size_t argc) {{"
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
                    "{static_kw}{always_inline}int {symbol}_fn_{index}_f(kiln_call_context *ctx, kiln_val * restrict result, kiln_val * restrict upvalues, const kiln_val * restrict receiver_ptr, const kiln_val * restrict args, size_t argc) {{"
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
            let (result, mut params) = typed_signature(proto, returns_number, needs);
            if super::parallel_clone() {
                // Tier 1 (ADR 0069): each captured binding the clone reads arrives as a
                // `double` after the parameters, read and guarded once on the realm thread.
                for k in super::parallel_captures(plan, index) {
                    params.push_str(&format!(", double cap{k}"));
                }
            }
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
            // Issue 21: the entry's row lookup, callable by a method call site before it
            // enters the entry. Emitted wherever the entry has a receiver prologue; declared
            // (in `lib.rs`) and called only where the entry reads the row.
            if !super::parallel_clone() && !plan.generic_only && this_site_count(graph) > 0 {
                let (this_col_map, this_col_names) = this_columns(graph);
                let linkage = if plan.should_chunk {
                    ""
                } else {
                    "static inline __attribute__((unused)) "
                };
                output.push_str(&typed_row_claimer(
                    symbol,
                    index,
                    &this_col_names,
                    typed_double_mask(&this_col_map, plan),
                    linkage,
                ));
            }
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
            typed_double_mask(&this_col_map, plan)
        } else {
            0u32
        };
        receiver_prologue(symbol, index, &this_col_names, kind, double_mask)
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
    let context_end = output.len();
    if kind == EntryKind::Boxed {
        output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
        if type_profile_record && proto.parameter_count > 0 {
            writeln!(
                output,
                "    for (size_t _pi = 0; _pi < argc && _pi < {}; ++_pi) {{ __atomic_fetch_or(&{symbol}_fn_{index}_param_tags[_pi], kiln_profile_tag_of(args[_pi]), __ATOMIC_RELAXED); }}",
                proto.parameter_count
            )
            .unwrap();
        }
    } else if matches!(kind, EntryKind::Typed { .. })
        && type_profile_record
        && proto.parameter_count > 0
    {
        for p in 0..proto.parameter_count {
            writeln!(
                output,
                "    __atomic_fetch_or(&{symbol}_fn_{index}_param_tags[{p}], 1u, __ATOMIC_RELAXED);"
            )
            .unwrap();
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
                            "        case {}: goto kiln_resume_exit_{exit_pc};",
                            delegate_exit_id(*exit_pc)
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
    cache_module_globals(output, context_start, symbol);
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
            // A chunked unit's callers in other chunks name the shape and slot rows (`lib.rs`
            // declares them extern there), so here they are the one external definition.
            let shared = if plan.should_chunk { "" } else { "static " };
            writeln!(
                declarations,
                "{shared}const kiln_shape *{symbol}_fn_{index}_recv_shape[4];\nstatic const kiln_shape *{symbol}_fn_{index}_recv_refused[4];\n{shared}uint32_t {symbol}_fn_{index}_recv_slot[4][{this_sites}] = {{ {{{row}}}, {{{row}}}, {{{row}}}, {{{row}}} }};\n{shared}const kiln_shape *{symbol}_fn_{index}_typed_recv_shape[4];\nstatic const kiln_shape *{symbol}_fn_{index}_typed_recv_refused[4];\n{shared}uint32_t {symbol}_fn_{index}_typed_recv_slot[4][{this_sites}] = {{ {{{row}}}, {{{row}}}, {{{row}}}, {{{row}}} }};"
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
            writeln!(
                output,
                "    static kiln_c99_method_path {pfx}_path = KILN_C99_UNLEARNED_METHOD_PATH;"
            )
            .unwrap();
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
    // The same callee-cache declarations the synchronous entry makes (`emit_module_body`): a
    // module with a top-level `await` is an entry like any other, and its devirtualized sites
    // name `{symbol}_fn_entry_v{v}_t_{i}` statics that nothing declared here -- a build failure
    // on the first such module that called a typed method (`ticker.start(3)`).
    for (vid, typed_candidates) in
        sites::collect_typed_call_sites(graph, program, plan, &plan.numbers)
    {
        for i in typed_candidates {
            let pfx = format!("{}_fn_entry_v{}_t_{}", symbol, vid.0, i);
            writeln!(
                output,
                "    static kiln_c99_method_path {pfx}_path = KILN_C99_UNLEARNED_METHOD_PATH;"
            )
            .unwrap();
        }
    }
    writeln!(output, "    kiln_val *locals = {symbol}_globals;").unwrap();
    output.push_str("    if (frame->started) {\n        switch (frame->resume) {\n");
    for block in &graph.blocks {
        if let Terminator::Suspend { pc, delegate, .. } = &block.terminator {
            writeln!(output, "        case {pc}: goto kiln_resume_{pc};").unwrap();
            if let Some((_, _, exit_pc)) = delegate {
                writeln!(
                    output,
                    "        case {}: goto kiln_resume_exit_{exit_pc};",
                    delegate_exit_id(*exit_pc)
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
    live_numbers: &HashSet<ValueId>,
    live_ints: &HashSet<ValueId>,
    booleans: &HashSet<ValueId>,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    use_counts: &HashMap<ValueId, usize>,
    ivs: &std::collections::HashMap<ValueId, InductionVariable>,
    block_prefix: &str,
    is_fast_clone: bool,
    keep_boxed: &HashSet<ValueId>,
    type_profile_record: bool,
    this_sites: &std::collections::HashMap<ValueId, usize>,
    spec: &Speculation,
) {
    // A proven number nothing reads boxed is never boxed: its box statement is dropped from
    // the operation's text, and its twin is all that lives. In a versioned body the generic
    // clone always boxes, and a box the generic clone reads that the fast clone dropped
    // (`keep_boxed`, see `split_clone_twins`) is written on each exit's way out: an exit hands
    // the generic clone the fast clone's variables as they stand. (A fast clone that boxed only at its exits was tried on
    // 2026-09-06: it measured nothing, and a bundled `marked` answered wrongly under it.)
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
    // The boxes the generic clone reads that this (fast) clone did not write: each exit into
    // the generic clone writes them from their twins on its way out (see `split_clone_twins`).
    let exit_boxes: Vec<ValueId> = {
        let mut exit_boxes: Vec<ValueId> = keep_boxed
            .iter()
            .filter(|v| boxed.as_ref().is_some_and(|boxed| !boxed.contains(v)))
            .copied()
            .collect();
        exit_boxes.sort();
        exit_boxes
    };
    let sequence_start = output.len();
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
        if !block.is_reachable_from_entry(graph.entry_block) {
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
    let is_dead_effectless_op = |vid: ValueId, op: &GraphOp| {
        !used.contains(&vid)
            && match op {
                GraphOp::LoadGlobal(_)
                | GraphOp::Const { .. }
                | GraphOp::LoadLocal(_)
                | GraphOp::LoadThis
                | GraphOp::LoadNewTarget
                | GraphOp::LoadBuiltin(_)
                | GraphOp::IsReturnSignal(_)
                | GraphOp::Unary {
                    op: UnaryOp::Void, ..
                } => true,
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
        if !block.is_reachable_from_entry(graph.entry_block) {
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
                writeln!(output, "kiln_resume_exit_{exit_pc}:;").unwrap();
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
                    if live_numbers.contains(&phi_vid) {
                        materialize_phis.push_str(&format!(
                            " v{} = kiln_c99_val_from_double(d{});",
                            phi_vid.0, phi_vid.0
                        ));
                    } else if live_ints.contains(&phi_vid) {
                        materialize_phis.push_str(&format!(
                            " v{} = kiln_c99_val_from_double((double)i{});",
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
            if is_dead_effectless_op(vid, op) {
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
                live_numbers,
                live_ints,
                booleans,
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
            // reads the twin. Unboxed comparisons already assign the boolean twin directly if needed.
            let is_unboxed_cmp = matches!(
                op,
                GraphOp::Binary { op, left, right }
                    if matches!(
                        op,
                        BinaryOp::StrictEqual
                            | BinaryOp::LooseEqual
                            | BinaryOp::StrictNotEqual
                            | BinaryOp::LooseNotEqual
                            | BinaryOp::LessThan
                            | BinaryOp::LessThanOrEqual
                            | BinaryOp::GreaterThan
                            | BinaryOp::GreaterThanOrEqual
                    ) && ((ints.contains(left) && ints.contains(right))
                        || (numbers.contains(left) && numbers.contains(right)))
            );
            if booleans.contains(&vid) && !matches!(op, GraphOp::Phi { .. }) && !is_unboxed_cmp {
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
            live_numbers,
            live_ints,
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
    if is_fast_clone && !exit_boxes.is_empty() {
        route_exits_through_boxing(
            output,
            sequence_start,
            graph,
            &exit_boxes,
            live_numbers,
            live_ints,
        );
    }
}

/// The resume id of a `yield*`'s completion: its target pc, in a space of its own. A suspension
/// resumes by its own pc and a delegate's completion by the pc of the instruction after the
/// `yield*`, so `yield yield* inner()` -- whose `yield` *is* that instruction -- gave both the
/// same id: a duplicate `case` and a redefined label, and the module did not compile (the
/// release source DCE writes exactly that from `const r = yield* inner(); yield r;`;
/// parallel-builtins issue 18). `frame->resume` is a `size_t`, and bytecode stays far below it.
fn delegate_exit_id(exit_pc: u32) -> u64 {
    (1u64 << 31) | u64::from(exit_pc)
}

/// Send each exit of a fast clone's text into the generic clone through a stub that first
/// writes, from their twins, the `boxes` the generic clone will read after that exit:
/// `goto kiln_generic_resume_N;` (a speculation that failed mid-body) becomes
/// `goto kiln_fast_exit_N;`, and `goto kiln_generic_block_N;` (a loop header's exit) becomes
/// `goto kiln_fast_exit_block_N;`. An exit boxes only a value that is defined before it and
/// used after it: boxing every kept value at every exit kept every twin alive to every exit,
/// and that cost the hot path about 7% on `bench/non-io/class-poly.mjs`.
fn route_exits_through_boxing(
    output: &mut String,
    start: usize,
    graph: &FunctionGraph,
    boxes: &[ValueId],
    live_numbers: &HashSet<ValueId>,
    live_ints: &HashSet<ValueId>,
) {
    let boxes: HashSet<ValueId> = boxes.iter().copied().collect();
    let dominators = crate::graph::compute_dominators(graph);
    let block_of = |id: BlockId| graph.blocks.iter().find(|b| b.id == id);
    // Where an exit leaves: the block, and how many of its operations ran before it.
    let exit_point = |block: bool, id: u32| -> Option<(BlockId, usize)> {
        if block {
            return Some((BlockId(id), 0));
        }
        graph.blocks.iter().find_map(|b| {
            b.operations
                .iter()
                .position(|(v, _)| v.0 == id)
                .map(|p| (b.id, p))
        })
    };
    let terminator_uses = |terminator: &Terminator| -> Vec<ValueId> {
        match terminator {
            Terminator::Return(Some(v)) | Terminator::Throw(v) => vec![*v],
            Terminator::Branch { condition, .. }
            | Terminator::BranchNotNullish { condition, .. } => vec![*condition],
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                ..
            } => {
                let mut uses = vec![*value];
                uses.extend(spilled.iter().copied());
                if let Some((iter, _, _)) = delegate {
                    uses.push(*iter);
                }
                uses
            }
            _ => Vec::new(),
        }
    };
    // Backward liveness over the fast graph. A phi's inputs are live out of the predecessor they
    // come from, not live into the phi's block.
    let mut live_out: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let mut live_in: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let transfer = |b: &BasicBlock, out: &HashSet<ValueId>, from: usize| -> HashSet<ValueId> {
        let mut live = out.clone();
        live.extend(terminator_uses(&b.terminator));
        for (v, op) in b.operations[from.min(b.operations.len())..].iter().rev() {
            live.remove(v);
            if !matches!(op, GraphOp::Phi { .. }) {
                live.extend(op.operands());
            }
        }
        live
    };
    let mut changed = true;
    while changed {
        changed = false;
        for b in graph.blocks.iter().rev() {
            let mut out: HashSet<ValueId> = HashSet::new();
            for succ in b.successors.iter().chain(b.exception_handler.iter()) {
                if let Some(live) = live_in.get(succ) {
                    out.extend(live.iter().copied());
                }
                if let Some(sb) = block_of(*succ) {
                    for (_, op) in &sb.operations {
                        if let GraphOp::Phi { inputs } = op {
                            out.extend(
                                inputs
                                    .iter()
                                    .filter(|(from, _)| *from == b.id)
                                    .map(|(_, v)| *v),
                            );
                        }
                    }
                }
            }
            let inn = transfer(b, &out, 0);
            if live_out.get(&b.id) != Some(&out) {
                live_out.insert(b.id, out);
                changed = true;
            }
            if live_in.get(&b.id) != Some(&inn) {
                live_in.insert(b.id, inn);
                changed = true;
            }
        }
    }
    let boxes_at = |block: BlockId, position: usize| -> Vec<ValueId> {
        let Some(here) = block_of(block) else {
            return boxes.iter().copied().collect();
        };
        // Defined before the exit: in a block that strictly dominates this one, or earlier in it.
        let mut defined: HashSet<ValueId> = here.operations[..position.min(here.operations.len())]
            .iter()
            .map(|(v, _)| *v)
            .collect();
        if let Some(doms) = dominators.get(&block) {
            for dom in doms.iter().filter(|d| **d != block) {
                if let Some(b) = block_of(*dom) {
                    defined.extend(b.operations.iter().map(|(v, _)| *v));
                }
            }
        }
        defined.extend(graph.initial_locals.iter().copied());
        // Used after it: live at the exit's position, which the generic clone resumes from.
        let used = transfer(
            here,
            live_out.get(&block).unwrap_or(&HashSet::new()),
            position,
        );
        let mut at: Vec<ValueId> = boxes
            .iter()
            .filter(|v| defined.contains(v) && used.contains(v))
            .copied()
            .collect();
        at.sort();
        at
    };
    let text = output.split_off(start);
    let mut rewritten = String::with_capacity(text.len());
    let mut stubs: std::collections::BTreeMap<(bool, u32), String> =
        std::collections::BTreeMap::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("goto kiln_generic_") {
        rewritten.push_str(&rest[..at]);
        let tail = &rest[at + "goto kiln_generic_".len()..];
        let (block, digits_at) = if let Some(t) = tail.strip_prefix("resume_") {
            (false, t)
        } else if let Some(t) = tail.strip_prefix("block_") {
            (true, t)
        } else {
            rewritten.push_str("goto kiln_generic_");
            rest = tail;
            continue;
        };
        let digits = digits_at
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(digits_at.len());
        let Ok(id) = digits_at[..digits].parse::<u32>() else {
            rewritten.push_str("goto kiln_generic_");
            rest = tail;
            continue;
        };
        let materialize = stubs.entry((block, id)).or_insert_with(|| {
            let values = match exit_point(block, id) {
                Some((b, p)) => boxes_at(b, p),
                None => boxes.iter().copied().collect(),
            };
            let mut text = String::new();
            for value in values {
                let v = value.0;
                if live_numbers.contains(&value) {
                    text.push_str(&format!(" v{v} = kiln_c99_val_from_double(d{v});"));
                } else if live_ints.contains(&value) {
                    text.push_str(&format!(" v{v} = kiln_c99_val_from_double((double)i{v});"));
                }
            }
            text
        });
        let kind = if block { "block_" } else { "resume_" };
        if materialize.is_empty() {
            rewritten.push_str(&format!("goto kiln_generic_{kind}{id}"));
        } else {
            let label = if block { "block_" } else { "" };
            rewritten.push_str(&format!("goto kiln_fast_exit_{label}{id}"));
        }
        rest = &digits_at[digits..];
    }
    rewritten.push_str(rest);
    output.push_str(&rewritten);
    for ((block, id), materialize) in stubs {
        if materialize.is_empty() {
            continue;
        }
        let (label, kind) = if block {
            ("block_", "block_")
        } else {
            ("", "resume_")
        };
        output.push_str(&format!(
            "kiln_fast_exit_{label}{id}:;{materialize} goto kiln_generic_{kind}{id};\n"
        ));
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

/// The proven boolean values that actually have their unboxed boolean twin `b{v}` read downstream,
/// either as an out-of-line branch condition or as an input to a boolean phi. Comparisons whose
/// branch inlines `i{l} < i{r}` or `d{l} < d{r}` do not need a boolean twin variable.
fn compute_live_booleans(
    graph: &FunctionGraph,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
    booleans: &HashSet<ValueId>,
) -> HashSet<ValueId> {
    if booleans.is_empty() {
        return HashSet::new();
    }

    let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();

    let is_inlined_branch_cmp = |v: ValueId| -> bool {
        match defs.get(&v) {
            Some(GraphOp::Binary { op, left, right }) => {
                let is_cmp = matches!(
                    op,
                    BinaryOp::LessThan
                        | BinaryOp::LessThanOrEqual
                        | BinaryOp::GreaterThan
                        | BinaryOp::GreaterThanOrEqual
                        | BinaryOp::StrictEqual
                        | BinaryOp::StrictNotEqual
                        | BinaryOp::LooseEqual
                        | BinaryOp::LooseNotEqual
                );
                is_cmp
                    && ((ints.contains(left) && ints.contains(right))
                        || (numbers.contains(left) && numbers.contains(right)))
            }
            _ => false,
        }
    };

    let mut live = HashSet::new();
    let mut worklist = Vec::new();

    for block in &graph.blocks {
        if !block.is_reachable_from_entry(graph.entry_block) {
            continue;
        }
        if let Terminator::Branch { condition, .. } = &block.terminator {
            if booleans.contains(condition) && !is_inlined_branch_cmp(*condition) {
                if live.insert(*condition) {
                    worklist.push(*condition);
                }
            }
        }
    }

    let mut phi_inputs: std::collections::HashMap<ValueId, Vec<ValueId>> =
        std::collections::HashMap::new();
    for block in &graph.blocks {
        if !block.is_reachable_from_entry(graph.entry_block) {
            continue;
        }
        for (vid, op) in &block.operations {
            if let GraphOp::Phi { inputs } = op {
                if booleans.contains(vid) {
                    let ins = inputs.iter().map(|(_, in_v)| *in_v).collect();
                    phi_inputs.insert(*vid, ins);
                }
            }
        }
    }

    while let Some(vid) = worklist.pop() {
        if let Some(inputs) = phi_inputs.get(&vid) {
            for in_v in inputs {
                if booleans.contains(in_v) && live.insert(*in_v) {
                    worklist.push(*in_v);
                }
            }
        }
    }

    live
}

/// Which unboxed twins each clone of a versioned body reads and writes, and which boxes the
/// fast clone must keep because the generic clone reads them.
struct CloneTwins {
    fast_numbers: HashSet<ValueId>,
    fast_ints: HashSet<ValueId>,
    generic_numbers: HashSet<ValueId>,
    generic_ints: HashSet<ValueId>,
    keep_boxed: HashSet<ValueId>,
}

/// The two clones of a versioned body share every SSA variable: an exit from the fast clone
/// resumes the generic one mid-body with the variables as they stand. The live twins are
/// computed against the fast clone's proof, which includes the speculation. A twin is only a
/// value's representation where the value is *proven* a number, and in the generic clone a
/// speculated value is not: `at + needle.length` there is emitted boxed (`needle.length` may
/// be anything), so its twin is never written, and a call argument that read the twin because
/// it was live passed a stale `0.0` -- `indexOf("\n", 0)` in `headerListRemove`, a loop that
/// never ended (parallel-builtins issue 18).
///
/// So the generic clone reads a twin only for a value its own (baseline) proof covers, and
/// reads everything else boxed. The fast clone may be the one that defined such a value and may
/// have dropped its box (nothing in the fast clone reads it boxed), so every exit into the
/// generic clone writes those boxes from their twins on its way out (`keep_boxed`,
/// `route_exits_through_boxing`) -- on the cold path, not on every definition. A baseline number the fast
/// clone carries as an integer only is given a double twin in both, since the generic clone
/// treats it as a number.
fn split_clone_twins(
    live_numbers: &HashSet<ValueId>,
    live_ints: &HashSet<ValueId>,
    baseline_numbers: &HashSet<ValueId>,
    baseline_ints: &HashSet<ValueId>,
) -> CloneTwins {
    let generic_ints: HashSet<ValueId> = live_ints.intersection(baseline_ints).copied().collect();
    let generic_numbers: HashSet<ValueId> = live_numbers
        .iter()
        .filter(|v| baseline_numbers.contains(v))
        .chain(
            live_ints
                .iter()
                .filter(|v| baseline_numbers.contains(v) && !baseline_ints.contains(v)),
        )
        .copied()
        .collect();
    let mut fast_numbers = live_numbers.clone();
    fast_numbers.extend(generic_numbers.iter().copied());
    let keep_boxed = live_numbers
        .iter()
        .chain(live_ints.iter())
        .filter(|v| !generic_numbers.contains(v) && !generic_ints.contains(v))
        .copied()
        .collect();
    CloneTwins {
        fast_numbers,
        fast_ints: live_ints.clone(),
        generic_numbers,
        generic_ints,
        keep_boxed,
    }
}

/// Values whose double twin `d{v}` or integer twin `i{v}` is actually read downstream.
/// Computed by backward propagation from consuming operations through phis and copies.
fn compute_live_twins(
    graph: &FunctionGraph,
    _program: &Program,
    proto: &FunctionProto,
    numbers: &HashSet<ValueId>,
    ints: &HashSet<ValueId>,
    live_booleans: &HashSet<ValueId>,
    boxed: Option<&HashSet<ValueId>>,
    array_slots: &HashSet<u32>,
    kind: EntryKind,
    plan: &CallPlan,
    use_counts: &std::collections::HashMap<ValueId, usize>,
    ivs: &std::collections::HashMap<ValueId, InductionVariable>,
) -> (HashSet<ValueId>, HashSet<ValueId>) {
    let mut live_numbers = HashSet::new();
    let mut live_ints = HashSet::new();

    for &iv in ivs.keys() {
        live_ints.insert(iv);
    }

    let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();

    let need_box = |v: ValueId| boxed.map_or(true, |b| b.contains(&v));

    let is_dead = |v: &ValueId, op: &GraphOp| {
        use_counts.get(v).copied().unwrap_or(0) == 0
            && matches!(
                op,
                GraphOp::Const { .. }
                    | GraphOp::Binary { .. }
                    | GraphOp::Unary { .. }
                    | GraphOp::LoadGlobal(_)
                    | GraphOp::LoadLocal(_)
                    | GraphOp::LoadThis
                    | GraphOp::LoadNewTarget
                    | GraphOp::LoadBuiltin(_)
                    | GraphOp::IsReturnSignal(_)
            )
    };

    // Seed pass: identify operations that demand unboxed twins directly from the environment.
    for block in &graph.blocks {
        if !block.is_reachable_from_entry(graph.entry_block) {
            continue;
        }

        match &block.terminator {
            Terminator::Branch { condition, .. } => {
                if let Some(GraphOp::Binary { op, left, right }) = defs.get(condition) {
                    let is_cmp = matches!(
                        op,
                        BinaryOp::StrictEqual
                            | BinaryOp::StrictNotEqual
                            | BinaryOp::LooseEqual
                            | BinaryOp::LooseNotEqual
                            | BinaryOp::LessThan
                            | BinaryOp::LessThanOrEqual
                            | BinaryOp::GreaterThan
                            | BinaryOp::GreaterThanOrEqual
                    );
                    if is_cmp {
                        if ints.contains(left) && ints.contains(right) {
                            live_ints.insert(*left);
                            live_ints.insert(*right);
                        } else if numbers.contains(left) && numbers.contains(right) {
                            if ints.contains(left) {
                                live_ints.insert(*left);
                            } else {
                                live_numbers.insert(*left);
                            }
                            if ints.contains(right) {
                                live_ints.insert(*right);
                            } else {
                                live_numbers.insert(*right);
                            }
                        }
                    }
                }
            }
            Terminator::Return(Some(v)) => {
                if matches!(
                    kind,
                    EntryKind::Typed {
                        returns_number: true,
                        ..
                    }
                ) && numbers.contains(v)
                {
                    if ints.contains(v) {
                        live_ints.insert(*v);
                    } else {
                        live_numbers.insert(*v);
                    }
                }
            }
            _ => {}
        }

        for (vid, op) in &block.operations {
            if is_dead(vid, op) {
                continue;
            }

            match op {
                GraphOp::Binary { op, left, right } => {
                    let is_cmp = matches!(
                        op,
                        BinaryOp::StrictEqual
                            | BinaryOp::StrictNotEqual
                            | BinaryOp::LooseEqual
                            | BinaryOp::LooseNotEqual
                            | BinaryOp::LessThan
                            | BinaryOp::LessThanOrEqual
                            | BinaryOp::GreaterThan
                            | BinaryOp::GreaterThanOrEqual
                    );
                    if is_cmp {
                        if live_booleans.contains(vid)
                            || need_box(*vid)
                            || use_counts.get(vid).copied().unwrap_or(0) > 0
                        {
                            if ints.contains(left) && ints.contains(right) {
                                live_ints.insert(*left);
                                live_ints.insert(*right);
                            } else if numbers.contains(left) && numbers.contains(right) {
                                if ints.contains(left) {
                                    live_ints.insert(*left);
                                } else {
                                    live_numbers.insert(*left);
                                }
                                if ints.contains(right) {
                                    live_ints.insert(*right);
                                } else {
                                    live_numbers.insert(*right);
                                }
                            }
                        }
                    } else if ints.contains(left)
                        && ints.contains(right)
                        && !matches!(op, BinaryOp::In | BinaryOp::InstanceOf)
                    {
                        // `emit_binary_ints` writes a quotient, a power, and a sum, difference
                        // or product not proven to stay an integer into the double twin; only
                        // the rest have an integer result. `{ code: i * 3 }` in a loop was the
                        // product without a declared twin (issue 16).
                        let double_result = matches!(op, BinaryOp::Divide | BinaryOp::Exponentiate)
                            || (!ints.contains(vid)
                                && matches!(
                                    op,
                                    BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply
                                ));
                        if double_result {
                            live_numbers.insert(*vid);
                            if ints.contains(vid) {
                                live_ints.insert(*vid);
                            }
                        } else {
                            live_ints.insert(*vid);
                        }
                        live_ints.insert(*left);
                        live_ints.insert(*right);
                    } else if numbers.contains(left)
                        && numbers.contains(right)
                        && !matches!(op, BinaryOp::In | BinaryOp::InstanceOf)
                    {
                        live_numbers.insert(*vid);
                        if ints.contains(left) {
                            live_ints.insert(*left);
                        } else {
                            live_numbers.insert(*left);
                        }
                        if ints.contains(right) {
                            live_ints.insert(*right);
                        } else {
                            live_numbers.insert(*right);
                        }
                    } else if !numbers.contains(left) || !numbers.contains(right) {
                        // Mixed or generic binary op fallback: emit_binary_boxed
                        if numbers.contains(left) && !ints.contains(left) {
                            live_numbers.insert(*left);
                        } else if ints.contains(left) {
                            live_ints.insert(*left);
                        }
                        if numbers.contains(right) && !ints.contains(right) {
                            live_numbers.insert(*right);
                        } else if ints.contains(right) {
                            live_ints.insert(*right);
                        }
                    }
                }
                GraphOp::Unary { op: _, value } => {
                    if ints.contains(vid) {
                        live_ints.insert(*vid);
                        if ints.contains(value) {
                            live_ints.insert(*value);
                        } else if numbers.contains(value) {
                            live_numbers.insert(*value);
                        }
                    } else if numbers.contains(vid) {
                        live_numbers.insert(*vid);
                        if ints.contains(value) {
                            live_ints.insert(*value);
                        } else if numbers.contains(value) {
                            live_numbers.insert(*value);
                        }
                    }
                }
                GraphOp::LoadLocal(slot) => {
                    if *slot < proto.parameter_count && matches!(kind, EntryKind::Typed { .. }) {
                        if ints.contains(vid) {
                            live_ints.insert(*vid);
                        }
                        if numbers.contains(vid) {
                            live_numbers.insert(*vid);
                        }
                    }
                }
                GraphOp::GetComputed { key, .. } => {
                    if ints.contains(key) {
                        live_ints.insert(*key);
                    }
                }
                GraphOp::SetComputed { key, value, .. }
                | GraphOp::SetComputedStrict { key, value, .. } => {
                    if ints.contains(key) {
                        live_ints.insert(*key);
                    }
                    if ints.contains(value) {
                        live_ints.insert(*value);
                    } else if numbers.contains(value) {
                        live_numbers.insert(*value);
                    }
                }
                GraphOp::CallMethod { args, .. } => {
                    for arg in args {
                        if ints.contains(arg) {
                            live_ints.insert(*arg);
                        } else if numbers.contains(arg) {
                            live_numbers.insert(*arg);
                        }
                    }
                }
                GraphOp::Call { callee, args, .. } => {
                    if plan.typed_sites.contains(vid) {
                        for arg in args {
                            if ints.contains(arg) {
                                live_ints.insert(*arg);
                            } else if numbers.contains(arg) {
                                live_numbers.insert(*arg);
                            }
                        }
                    }
                    if let Some(&target) = plan.known.get(callee) {
                        if plan
                            .typed_returns
                            .get(target as usize)
                            .copied()
                            .unwrap_or(false)
                        {
                            if ints.contains(vid) {
                                live_ints.insert(*vid);
                            } else if numbers.contains(vid) {
                                live_numbers.insert(*vid);
                            }
                        }
                    }
                }
                _ => {}
            }

            if !need_box(*vid) {
                if ints.contains(vid) {
                    live_ints.insert(*vid);
                } else if numbers.contains(vid) {
                    live_numbers.insert(*vid);
                }
            }
        }
    }

    // Fixpoint propagation loop: pull twin demands backward through operations and phis.
    let mut changed = true;
    while changed {
        changed = false;
        for block in &graph.blocks {
            if !block.is_reachable_from_entry(graph.entry_block) {
                continue;
            }
            for (vid, op) in &block.operations {
                if is_dead(vid, op) {
                    continue;
                }
                if live_ints.contains(vid) {
                    match op {
                        GraphOp::Binary { op, left, right } => {
                            let is_cmp = matches!(
                                op,
                                BinaryOp::StrictEqual
                                    | BinaryOp::StrictNotEqual
                                    | BinaryOp::LooseEqual
                                    | BinaryOp::LooseNotEqual
                                    | BinaryOp::LessThan
                                    | BinaryOp::LessThanOrEqual
                                    | BinaryOp::GreaterThan
                                    | BinaryOp::GreaterThanOrEqual
                            );
                            if !is_cmp {
                                if ints.contains(left) && ints.contains(right) {
                                    if live_ints.insert(*left) {
                                        changed = true;
                                    }
                                    if live_ints.insert(*right) {
                                        changed = true;
                                    }
                                    if live_ints.insert(*vid) {
                                        changed = true;
                                    }
                                } else {
                                    if ints.contains(left) {
                                        if live_ints.insert(*left) {
                                            changed = true;
                                        }
                                    } else if numbers.contains(left) && live_numbers.insert(*left) {
                                        changed = true;
                                    }
                                    if ints.contains(right) {
                                        if live_ints.insert(*right) {
                                            changed = true;
                                        }
                                    } else if numbers.contains(right) && live_numbers.insert(*right)
                                    {
                                        changed = true;
                                    }
                                }
                            }
                        }
                        GraphOp::Unary { op: _, value } => {
                            if ints.contains(value) {
                                if live_ints.insert(*value) {
                                    changed = true;
                                }
                                if ints.contains(vid) && live_ints.insert(*vid) {
                                    changed = true;
                                }
                            } else if numbers.contains(value) && live_numbers.insert(*value) {
                                changed = true;
                            }
                        }
                        GraphOp::Phi { inputs } => {
                            for (_, in_v) in inputs {
                                if ints.contains(in_v) {
                                    if live_ints.insert(*in_v) {
                                        changed = true;
                                    }
                                } else if numbers.contains(in_v) && live_numbers.insert(*in_v) {
                                    changed = true;
                                }
                            }
                        }
                        GraphOp::StoreLocal { slot, value } if !array_slots.contains(slot) => {
                            if ints.contains(value) {
                                if live_ints.insert(*value) {
                                    changed = true;
                                }
                            } else if numbers.contains(value) && live_numbers.insert(*value) {
                                changed = true;
                            }
                        }
                        GraphOp::CheckTdz { value, .. } => {
                            if ints.contains(value) {
                                if live_ints.insert(*value) {
                                    changed = true;
                                }
                            } else if numbers.contains(value) && live_numbers.insert(*value) {
                                changed = true;
                            }
                        }
                        GraphOp::LoadLocal(_) => {
                            if numbers.contains(vid) && live_numbers.insert(*vid) {
                                changed = true;
                            }
                        }
                        _ => {}
                    }
                }
                if live_numbers.contains(vid) {
                    match op {
                        GraphOp::Binary { op, left, right } => {
                            let is_cmp = matches!(
                                op,
                                BinaryOp::StrictEqual
                                    | BinaryOp::StrictNotEqual
                                    | BinaryOp::LooseEqual
                                    | BinaryOp::LooseNotEqual
                                    | BinaryOp::LessThan
                                    | BinaryOp::LessThanOrEqual
                                    | BinaryOp::GreaterThan
                                    | BinaryOp::GreaterThanOrEqual
                            );
                            if !is_cmp {
                                if numbers.contains(left) && numbers.contains(right) {
                                    if live_numbers.insert(*left) {
                                        changed = true;
                                    }
                                    if live_numbers.insert(*right) {
                                        changed = true;
                                    }
                                } else if ints.contains(left) && ints.contains(right) {
                                    if live_ints.insert(*left) {
                                        changed = true;
                                    }
                                    if live_ints.insert(*right) {
                                        changed = true;
                                    }
                                } else {
                                    if numbers.contains(left) && live_numbers.insert(*left) {
                                        changed = true;
                                    }
                                    if ints.contains(left) && live_ints.insert(*left) {
                                        changed = true;
                                    }
                                    if numbers.contains(right) && live_numbers.insert(*right) {
                                        changed = true;
                                    }
                                    if ints.contains(right) && live_ints.insert(*right) {
                                        changed = true;
                                    }
                                }
                            }
                        }
                        GraphOp::Unary { value, .. } => {
                            if numbers.contains(value) && live_numbers.insert(*value) {
                                changed = true;
                            } else if ints.contains(value) && live_ints.insert(*value) {
                                changed = true;
                            }
                        }
                        GraphOp::Phi { inputs } => {
                            for (_, in_v) in inputs {
                                if numbers.contains(in_v) && live_numbers.insert(*in_v) {
                                    changed = true;
                                } else if ints.contains(in_v) && live_ints.insert(*in_v) {
                                    changed = true;
                                }
                            }
                        }
                        GraphOp::StoreLocal { slot, value } if !array_slots.contains(slot) => {
                            if numbers.contains(value) && live_numbers.insert(*value) {
                                changed = true;
                            } else if ints.contains(value) && live_ints.insert(*value) {
                                changed = true;
                            }
                        }
                        GraphOp::CheckTdz { value, .. } => {
                            if numbers.contains(value) && live_numbers.insert(*value) {
                                changed = true;
                            } else if ints.contains(value) && live_ints.insert(*value) {
                                changed = true;
                            }
                        }
                        _ => {}
                    }
                }
                if let GraphOp::Phi { inputs } = op {
                    for (_, in_v) in inputs {
                        if live_ints.contains(in_v) && ints.contains(vid) {
                            if live_ints.insert(*vid) {
                                changed = true;
                            }
                        }
                        if live_numbers.contains(in_v) && numbers.contains(vid) {
                            if live_numbers.insert(*vid) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }

    (live_numbers, live_ints)
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
    let is_comparison = |op: &BinaryOp| {
        matches!(
            op,
            BinaryOp::StrictEqual
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
        if !block.is_reachable_from_entry(graph.entry_block) {
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
                // `+x` of a proven number is a copy: its emitters write the result's box as
                // `v = vx`, so it reads `x` boxed exactly when something reads it boxed.
                // Treating it as twin-only dropped `x`'s box and copied an unwritten one.
                GraphOp::Unary { op, value }
                    if matches!(op, UnaryOp::Plus | UnaryOp::ToNumeric)
                        && numbers.contains(value) =>
                {
                    phis.push((*vid, vec![*value]));
                }
                GraphOp::Unary { op, value }
                    if (*op == UnaryOp::Negate && numbers.contains(value))
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
                // Only a *comparison* of two proven numbers is inlined by the branch (see
                // `emit_terminator`); an arithmetic condition (`i % 2 ? a : b`, `n - 1 ? ..`)
                // is tested for truthiness through its box, so it must keep that box. Using
                // `twin_only` here dropped the box of every arithmetic condition, and the
                // branch then read an unwritten `v` (issue 27).
                let compares_twins = matches!(
                    defs.get(condition),
                    Some(GraphOp::Binary { op, left, right })
                        if is_comparison(op) && numbers.contains(left) && numbers.contains(right)
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
    let baseline_live_booleans =
        compute_live_booleans(graph, baseline_numbers, baseline_ints, &baseline_booleans);
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
        let all_live_booleans =
            compute_live_booleans(fast_graph, &all_numbers, &all_ints, &all_booleans);
        let (all_live_numbers, all_live_ints) = compute_live_twins(
            fast_graph,
            program,
            proto,
            &all_numbers,
            &all_ints,
            &all_live_booleans,
            None,
            array_slots,
            kind,
            plan,
            &use_counts,
            &ivs,
        );
        let clone_twins = split_clone_twins(
            &all_live_numbers,
            &all_live_ints,
            baseline_numbers,
            baseline_ints,
        );
        emit_ssa_declarations(
            output,
            fast_graph,
            &clone_twins.fast_numbers,
            &clone_twins.fast_ints,
            &all_live_booleans,
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
            &clone_twins.fast_numbers,
            &clone_twins.fast_ints,
            &all_live_booleans,
            &defs,
            &use_counts,
            &ivs,
            "kiln_fast_block_",
            true,
            &clone_twins.keep_boxed,
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
            &clone_twins.generic_numbers,
            &clone_twins.generic_ints,
            &all_live_booleans,
            &defs,
            &use_counts,
            &ivs,
            "kiln_generic_block_",
            false,
            &HashSet::new(),
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
        let boxed = if plan.generic_only && baseline_numbers.is_empty() {
            None
        } else {
            Some(boxed_uses(
                graph,
                program,
                baseline_numbers,
                baseline_ints,
                &baseline_live_booleans,
                array_slots,
                kind,
                &Speculation::default(),
            ))
        };
        let (live_numbers, live_ints) = compute_live_twins(
            graph,
            program,
            proto,
            baseline_numbers,
            baseline_ints,
            &baseline_live_booleans,
            boxed.as_ref(),
            array_slots,
            kind,
            plan,
            &use_counts,
            &ivs,
        );
        emit_ssa_declarations(
            output,
            graph,
            &live_numbers,
            &live_ints,
            &baseline_live_booleans,
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
            &live_numbers,
            &live_ints,
            &baseline_live_booleans,
            &defs,
            &use_counts,
            &ivs,
            "kiln_block_",
            false,
            &HashSet::new(),
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
    live_numbers: &HashSet<ValueId>,
    live_ints: &HashSet<ValueId>,
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
                    writeln!(output, "    __atomic_fetch_or(&{symbol}_fn_{index}_return_tag, 1u, __ATOMIC_RELAXED);").unwrap();
                }
                match value {
                    Some(val) if numbers.contains(val) => {
                        if live_numbers.contains(val) {
                            writeln!(output, "    return d{};", val.0).unwrap();
                        } else if live_ints.contains(val) {
                            writeln!(output, "    return (double)i{};", val.0).unwrap();
                        } else {
                            writeln!(
                                output,
                                "    {{ double ret_d; if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{}))) memcpy(&ret_d, &v{}, sizeof(ret_d)); else ret_d = kiln_c99_as_number(kiln_val_to_boundary(v{})); return ret_d; }}",
                                val.0, val.0, val.0
                            )
                            .unwrap();
                        }
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
                            "    __atomic_fetch_or(&{symbol}_fn_{index}_return_tag, kiln_profile_tag_of(v{}), __ATOMIC_RELAXED);",
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
                            "    __atomic_fetch_or(&{symbol}_fn_{index}_return_tag, kiln_profile_tag_of(KILN_VAL_UNDEFINED), __ATOMIC_RELAXED);"
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
                live_numbers,
                live_ints,
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
                    let a = if live_numbers.contains(left) {
                        format!("d{}", left.0)
                    } else if live_ints.contains(left) {
                        format!("(double)i{}", left.0)
                    } else {
                        format!("d{}", left.0)
                    };
                    let b = if live_numbers.contains(right) {
                        format!("d{}", right.0)
                    } else if live_ints.contains(right) {
                        format!("(double)i{}", right.0)
                    } else {
                        format!("d{}", right.0)
                    };
                    sym.map(|sym| format!("{a} {sym} {b}"))
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
                live_numbers,
                live_ints,
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
                live_numbers,
                live_ints,
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
                live_numbers,
                live_ints,
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
                live_numbers,
                live_ints,
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
                        iter.0,
                        delegate_exit_id(*exit_pc)
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
    boxed: Option<&HashSet<ValueId>>,
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
        let need_box = boxed.map_or(true, |b| b.contains(&ValueId(phi_v)));
        if need_box {
            write!(output, " kiln_val phi_t{i} = v{from_v};").unwrap();
        }
        if numbers.contains(&ValueId(phi_v)) {
            if numbers.contains(&ValueId(from_v)) {
                write!(output, " double phi_td{i} = d{from_v};").unwrap();
            } else if ints.contains(&ValueId(from_v)) {
                write!(output, " double phi_td{i} = (double)i{from_v};").unwrap();
            } else {
                write!(output, " double phi_td{i}; if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{from_v}))) memcpy(&phi_td{i}, &v{from_v}, sizeof(phi_td{i})); else phi_td{i} = kiln_c99_as_number(kiln_val_to_boundary(v{from_v}));").unwrap();
            }
        }
        if ints.contains(&ValueId(phi_v)) {
            if ints.contains(&ValueId(from_v)) {
                write!(output, " int64_t phi_ti{i} = i{from_v};").unwrap();
            } else if numbers.contains(&ValueId(from_v)) {
                write!(output, " int64_t phi_ti{i} = (int64_t)d{from_v};").unwrap();
            } else {
                write!(output, " int64_t phi_ti{i}; {{ double tmp_{i}; if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{from_v}))) memcpy(&tmp_{i}, &v{from_v}, sizeof(tmp_{i})); else tmp_{i} = kiln_c99_as_number(kiln_val_to_boundary(v{from_v})); phi_ti{i} = (int64_t)tmp_{i}; }}").unwrap();
            }
        }
        if booleans.contains(&ValueId(phi_v)) {
            let src = if booleans.contains(&ValueId(from_v)) {
                format!("b{from_v}")
            } else {
                format!("(v{from_v} == KILN_VAL_TRUE)")
            };
            write!(output, " int phi_tb{i} = {src};").unwrap();
        }
    }
    for (i, &(phi_v, _)) in moves.iter().enumerate() {
        let need_box = boxed.map_or(true, |b| b.contains(&ValueId(phi_v)));
        if need_box {
            write!(output, " v{phi_v} = phi_t{i};").unwrap();
        }
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

/// The `this` columns a typed entry reads as doubles: the representation a receiver's row must
/// have for the entry to claim it.
fn typed_double_mask(this_col_map: &HashMap<ValueId, usize>, plan: &CallPlan) -> u32 {
    let mut mask = 0u32;
    for (vid, col_idx) in this_col_map {
        if plan.numbers.contains(vid) && *col_idx < 32 {
            mask |= 1u32 << col_idx;
        }
    }
    mask
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
