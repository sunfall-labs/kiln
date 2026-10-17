//! C99 code generation directly from the SSA Value Graph.
//!
//! As specified in Phase 1 Step 1.3 of `fastest-native-plan.md` and ADR 0056:
//! - Eliminates the memory-bound `stack[]` array and `sp` stack pointer.
//! - Emits SSA intermediate values directly as C local variables (`kiln_val v0`, `v1`, ...).
//! - Retains `locals[]` only when needed for closure cells or generator suspension frames.
//! - Materializes call arguments into local stack arrays at call sites only, avoiding whole-body address-taking.

mod body;
mod clones;
mod ops;
pub(crate) mod sites;
mod string_accumulator;

pub(crate) use body::{
    ArgumentsLengthFact, emit_box_native_record, emit_compound_literal_fields,
};
pub use body::{
    emit_async_entry_body, emit_entry_from_graph, emit_entry_from_graph_with_profile,
    emit_function_from_graph, emit_function_from_graph_with_profile, retain_used_entry_context,
};
pub use clones::{emit_fast_clone, emit_restartable};
pub(crate) use sites::{method_candidates, method_candidates_map, this_site_count};

use crate::direct::TypedNeeds;
use crate::graph::{FunctionGraph, GraphOp, ValueId};
use kiln_ir::{ExportBinding, FunctionProto};
use std::collections::HashSet;

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
            | GraphOp::LoadGlobalBuiltin(_)
            | GraphOp::ResumeValue
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
            | GraphOp::DefineOwnProperty { .. }
            | GraphOp::GetComputed { .. }
            | GraphOp::SetComputed { .. }
            | GraphOp::SetComputedStrict { .. }
            | GraphOp::DefineOwnComputed { .. }
            | GraphOp::IsReturnSignal(_)
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
    /// Field reads of a sunk literal (`FunctionGraph::spread_sinks`) assumed to answer numbers;
    /// the fast clone checks each and leaves when one does not.
    pub fields: HashSet<ValueId>,
    /// Operations the fast clone leaves for the generic clone to run itself, after rebuilding
    /// the sunk literal on the heap: the spread whose source failed its guard, and each checked
    /// field read. The generic clone labels each before the operation.
    pub sinks: HashSet<ValueId>,
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
    /// An asynchronous unit entry body: runs top-level await using a generator frame.
    AsyncEntry,
}

/// The typed entry's declaration: the result in a register, `double` where the plan proved
/// it and `kiln_val` otherwise, with `KILN_VAL_TYPED_FAIL` for a failure; then the depth, what
/// the body needs and nothing it does not, and the parameters as the doubles they are.
pub fn typed_signature(
    proto: &FunctionProto,
    symbol: &str,
    semantics: crate::NativeSemantics,
    returns_number: bool,
    needs: TypedNeeds,
) -> (String, String) {
    let result = if returns_number {
        "double".to_string()
    } else if semantics == crate::NativeSemantics::Native && proto.return_layout().is_some() {
        let ret_lid = proto.return_layout().unwrap();
        format!("{symbol}_rec_{ret_lid}")
    } else {
        "kiln_val".to_string()
    };
    let mut params = String::from("int depth");
    if needs.upvalues {
        params.push_str(", kiln_val * restrict upvalues");
    }
    if needs.receiver {
        params.push_str(", kiln_val receiver");
    }
    if needs.row {
        params.push_str(", const uint32_t * restrict recv_row_in");
    }
    if needs.runtime {
        params.push_str(", kiln_host *host, kiln_value *thrown");
    }
    let param_layouts = proto.parameter_layouts();
    for k in 0..proto.parameter_count {
        if semantics == crate::NativeSemantics::Native
            && param_layouts.get(k as usize).copied().flatten().is_some()
        {
            let lid = param_layouts[k as usize].unwrap();
            params.push_str(&format!(", {symbol}_rec_{lid} *p{k}"));
        } else {
            params.push_str(&format!(", double p{k}"));
        }
    }
    (result, params)
}

thread_local! {
    /// Set while the parallel clone of a typed entry is being emitted (ADR 0058, application
    /// 2). It changes two things and nothing else: the entry's name ends in `_p` instead of
    /// `_t`, and a `Math.<name>` call on the builtin with proven-number arguments lowers to the
    /// bare libm call with no generic fallback, because the clone runs on a thread with no realm
    /// to fall back to. The site that calls the clone checks `kiln_c99_builtins_unmodified()` on the
    /// realm thread first, so an overridden `Math.sqrt` never reaches it. A thread-local rather
    /// than a parameter because every emitter signature between `lib.rs` and the op would
    /// otherwise carry a flag that only one op reads.
    static PARALLEL_CLONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The ids of the blocks of `graph` that lie on a cycle, following both control edges and the
/// edge to a block's exception handler (Tarjan's strongly connected components).
pub(crate) fn blocks_on_a_cycle(graph: &FunctionGraph) -> HashSet<u32> {
    use std::collections::HashMap;
    let index_of: HashMap<u32, usize> = graph
        .blocks
        .iter()
        .enumerate()
        .map(|(position, block)| (block.id.0, position))
        .collect();
    let edges: Vec<Vec<usize>> = graph
        .blocks
        .iter()
        .map(|block| {
            block
                .successors
                .iter()
                .chain(block.exception_handler.iter())
                .filter_map(|id| index_of.get(&id.0).copied())
                .collect()
        })
        .collect();
    let count = graph.blocks.len();
    let mut order = vec![usize::MAX; count];
    let mut low = vec![0usize; count];
    let mut on_stack = vec![false; count];
    let mut stack: Vec<usize> = Vec::new();
    let mut next_order = 0usize;
    let mut on_cycle = HashSet::new();
    for root in 0..count {
        if order[root] != usize::MAX {
            continue;
        }
        // Iterative Tarjan: (node, next edge to look at).
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        order[root] = next_order;
        low[root] = next_order;
        next_order += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&mut (node, ref mut edge)) = work.last_mut() {
            if *edge < edges[node].len() {
                let target = edges[node][*edge];
                *edge += 1;
                if order[target] == usize::MAX {
                    order[target] = next_order;
                    low[target] = next_order;
                    next_order += 1;
                    stack.push(target);
                    on_stack[target] = true;
                    work.push((target, 0));
                } else if on_stack[target] {
                    low[node] = low[node].min(order[target]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == order[node] {
                let mut component = Vec::new();
                loop {
                    let member = stack.pop().expect("tarjan stack");
                    on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                let self_loop = edges[node].contains(&node);
                if component.len() > 1 || self_loop {
                    for member in component {
                        on_cycle.insert(graph.blocks[member].id.0);
                    }
                }
            }
        }
    }
    on_cycle
}

/// The upvalue indices, ascending, whose values the parallel clone of proto `index` takes as
/// `double cap<k>` parameters after its own (tier 1, ADR 0069). Empty for a proto that
/// captures nothing or does not qualify.
pub fn parallel_captures(plan: &crate::direct::CallPlan, index: usize) -> Vec<u32> {
    match plan.parallel.get(index) {
        Some(Ok(shape)) => shape.captures.clone(),
        _ => Vec::new(),
    }
}

/// Whether proto `index`'s parallel clone is a predicate: it answers a boxed number or boolean
/// rather than a double (`parallel::parallel_result`), and only the filter workers call it.
pub fn parallel_boolean_result(plan: &crate::direct::CallPlan, index: usize) -> bool {
    matches!(plan.parallel.get(index), Some(Ok(shape)) if shape.boolean_result)
}

/// Whether the parallel clone is being emitted on this thread.
pub fn parallel_clone() -> bool {
    PARALLEL_CLONE.with(|c| c.get())
}

/// Run `f` with the parallel-clone flag set, restoring it after.
pub fn with_parallel_clone<R>(f: impl FnOnce() -> R) -> R {
    let before = PARALLEL_CLONE.with(|c| c.replace(true));
    let out = f();
    PARALLEL_CLONE.with(|c| c.set(before));
    out
}

/// The `parallelMap` chunk worker over a proto's parallel clone (ADR 0058, application 2): the
/// provider's pool calls it over disjoint `[start, end)` ranges of the input and output element
/// arrays, on a thread that owns no realm. That is sound only because the entry it calls reads
/// its `double` parameters and returns a `double` -- `parallel::parallel_safe` on the body, and
/// the clone's text scanned for any mention of the host, receiver, upvalues or heap before it
/// is admitted (`lib.rs`) -- so no host, receiver, upvalue table, shape row or thread-local heap
/// cache is reached from here. An entry that bails out (`KILN_VAL_TYPED_FAIL`)
/// flags `ctx` and stops; the caller then takes the sequential lowering, which recomputes.
///
/// The workers are named `{symbol}_pmap*_{index}` and call the clone `{clone_symbol}_fn_{index}_p`.
/// The two differ only in a generic-only module's worker unit (issue 14), where the workers
/// take the module's link-visible symbol and the clone keeps the unit-private name it was
/// emitted with.
pub fn emit_parallel_chunk(
    output: &mut String,
    symbol: &str,
    clone_symbol: &str,
    index: usize,
    parameter_count: u32,
    boolean_result: bool,
    captures: &[u32],
    should_chunk: bool,
) {
    use std::fmt::Write as _;
    let linkage = if should_chunk { "" } else { "static " };
    let mut index_arg = if parameter_count == 2 {
        ", (double)i".to_owned()
    } else {
        String::new()
    };
    // Tier 1 (ADR 0069): `ctx` is `struct { int failed; double caps[N]; }`, the captured values
    // the call site read and guarded on the realm thread, in the clone's `cap<k>` order.
    let caps_decl = if captures.is_empty() {
        String::new()
    } else {
        for j in 0..captures.len() {
            index_arg.push_str(&format!(", pmap_caps[{j}]"));
        }
        format!(
            "    const double * pmap_caps = ((const struct {{ int failed; double caps[{}]; }} *)ctx)->caps;\n",
            captures.len()
        )
    };
    // The family's workers (parallel issue 10) share the prelude's element read and the clone's
    // calling convention; which kinds a proto gets follows from how many parameters it declares.
    let kinds = parallel_chunk_kinds(parameter_count, boolean_result);
    emit_family_chunks(
        output,
        symbol,
        clone_symbol,
        index,
        &kinds,
        boolean_result,
        &caps_decl,
        &index_arg,
        captures.len(),
        linkage,
    );
    if !kinds.contains(&"") {
        return;
    }
    writeln!(
        output,
        "static inline uint32_t {symbol}_to_u32_{index}(double value) {{\n\
    if (!isfinite(value) || value == 0.0) return 0;\n\
    double wrapped = fmod(trunc(value), 4294967296.0);\n\
    if (wrapped < 0.0) wrapped += 4294967296.0;\n\
    return (uint32_t)wrapped;\n\
}}\n\
static inline uint8_t {symbol}_clamp_u8_{index}(double value) {{\n\
    if (isnan(value) || value <= 0.0) return 0;\n\
    if (value >= 255.0) return 255;\n\
    double rounded = floor(value);\n\
    if (value - rounded > 0.5 || (value - rounded == 0.5 && fmod(rounded, 2.0) != 0.0)) {{\n\
        rounded += 1.0;\n\
    }}\n\
    return (uint8_t)rounded;\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    const kiln_val * restrict in_arr = (const kiln_val * restrict)in;\n\
    kiln_val * restrict out_arr = (kiln_val * restrict)out;\n\
    int failed = 0;\n\
    #pragma clang loop vectorize(enable) interleave(enable)\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double d; memcpy(&d, &in_arr[i], sizeof d);\n\
        double r = {clone_symbol}_fn_{index}_p(1, d{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        out_arr[i] = kiln_c99_val_from_double(r);\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_f64_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    const double * restrict in_arr = (const double * restrict)in;\n\
    double * restrict out_arr = (double * restrict)out;\n\
    int failed = 0;\n\
    #pragma clang loop vectorize(enable) interleave(enable)\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double r = {clone_symbol}_fn_{index}_p(1, in_arr[i]{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        out_arr[i] = r;\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_f32_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    const float * restrict in_arr = (const float * restrict)in;\n\
    float * restrict out_arr = (float * restrict)out;\n\
    int failed = 0;\n\
    #pragma clang loop vectorize(enable) interleave(enable)\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double r = {clone_symbol}_fn_{index}_p(1, (double)in_arr[i]{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        out_arr[i] = (float)r;\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_i32_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    const int32_t * restrict in_arr = (const int32_t * restrict)in;\n\
    int32_t * restrict out_arr = (int32_t * restrict)out;\n\
    int failed = 0;\n\
    #pragma clang loop vectorize(enable) interleave(enable)\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double r = {clone_symbol}_fn_{index}_p(1, (double)in_arr[i]{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        out_arr[i] = (int32_t){symbol}_to_u32_{index}(r);\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_u32_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    const uint32_t * restrict in_arr = (const uint32_t * restrict)in;\n\
    uint32_t * restrict out_arr = (uint32_t * restrict)out;\n\
    int failed = 0;\n\
    #pragma clang loop vectorize(enable) interleave(enable)\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double r = {clone_symbol}_fn_{index}_p(1, (double)in_arr[i]{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        out_arr[i] = {symbol}_to_u32_{index}(r);\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_i16_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    const int16_t * restrict in_arr = (const int16_t * restrict)in;\n\
    int16_t * restrict out_arr = (int16_t * restrict)out;\n\
    int failed = 0;\n\
    #pragma clang loop vectorize(enable) interleave(enable)\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double r = {clone_symbol}_fn_{index}_p(1, (double)in_arr[i]{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        out_arr[i] = (int16_t)(uint16_t){symbol}_to_u32_{index}(r);\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_u16_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    const uint16_t * restrict in_arr = (const uint16_t * restrict)in;\n\
    uint16_t * restrict out_arr = (uint16_t * restrict)out;\n\
    int failed = 0;\n\
    #pragma clang loop vectorize(enable) interleave(enable)\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double r = {clone_symbol}_fn_{index}_p(1, (double)in_arr[i]{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        out_arr[i] = (uint16_t){symbol}_to_u32_{index}(r);\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_i8_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    const int8_t * restrict in_arr = (const int8_t * restrict)in;\n\
    int8_t * restrict out_arr = (int8_t * restrict)out;\n\
    int failed = 0;\n\
    #pragma clang loop vectorize(enable) interleave(enable)\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double r = {clone_symbol}_fn_{index}_p(1, (double)in_arr[i]{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        out_arr[i] = (int8_t)(uint8_t){symbol}_to_u32_{index}(r);\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_u8_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    const uint8_t * restrict in_arr = (const uint8_t * restrict)in;\n\
    uint8_t * restrict out_arr = (uint8_t * restrict)out;\n\
    int failed = 0;\n\
    #pragma clang loop vectorize(enable) interleave(enable)\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double r = {clone_symbol}_fn_{index}_p(1, (double)in_arr[i]{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        out_arr[i] = (uint8_t){symbol}_to_u32_{index}(r);\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_u8c_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    const uint8_t * restrict in_arr = (const uint8_t * restrict)in;\n\
    uint8_t * restrict out_arr = (uint8_t * restrict)out;\n\
    int failed = 0;\n\
    #pragma clang loop vectorize(enable) interleave(enable)\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double r = {clone_symbol}_fn_{index}_p(1, (double)in_arr[i]{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        out_arr[i] = {symbol}_clamp_u8_{index}(r);\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}\n\
{linkage}void __attribute__((unused)) {symbol}_pmap_typed_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    kiln_typed_array * restrict in_arr = (kiln_typed_array * restrict)in;\n\
    kiln_typed_array * restrict out_arr = (kiln_typed_array * restrict)out;\n\
    int failed = 0;\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double d = kiln_c99_typed_get(in_arr, i);\n\
        double r = {clone_symbol}_fn_{index}_p(1, d{index_arg});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        kiln_c99_typed_set(out_arr, i, r);\n\
    }}\n\
    if (KILN_UNLIKELY(failed)) {{ __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }}\n\
}}"
    )
    .unwrap();
}

/// The suffixes of `parallelMap`'s chunk workers, in the order they are declared:
/// `{symbol}_pmap{suffix}_{index}`.
pub const PARALLEL_CHUNK_KINDS: [&str; 11] = [
    "", "_f64", "_f32", "_i32", "_u32", "_i16", "_u16", "_i8", "_u8", "_u8c", "_typed",
];

/// The family's chunk workers (parallel issue 10): `parallelFilter`'s keep masks over an
/// array's elements and a view, `parallelReduce`'s leaf folds over the same two, and its
/// pairwise combine over a level of doubles.
pub const PARALLEL_FAMILY_CHUNK_KINDS: [&str; 5] =
    ["_filt", "_filt_typed", "_fold", "_fold_typed", "_comb"];

/// Every chunk worker `emit_parallel_chunk` defines for a proto that declares
/// `parameter_count` parameters, in declaration order: the ones whose call passes that many --
/// `map` and `filter` pass the element and the index (1 or 2), `parallelReduce`'s callback the
/// accumulator, the element and the index (2 or 3), and its combine two results (2).
///
/// A predicate's clone (`boolean_result`) answers a boxed value, which only the filter workers
/// read, so it gets those alone.
pub fn parallel_chunk_kinds(parameter_count: u32, boolean_result: bool) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    if boolean_result {
        if (1..=2).contains(&parameter_count) {
            kinds.extend(["_filt", "_filt_typed"]);
        }
        return kinds;
    }
    if (1..=2).contains(&parameter_count) {
        kinds.extend(PARALLEL_CHUNK_KINDS);
        kinds.extend(["_filt", "_filt_typed"]);
    }
    if (2..=3).contains(&parameter_count) {
        kinds.extend(["_fold", "_fold_typed"]);
    }
    if parameter_count == 2 {
        kinds.push("_comb");
    }
    kinds
}

/// The family's workers for one clone (see [`PARALLEL_FAMILY_CHUNK_KINDS`]), each only where
/// `kinds` has it. Their contracts, which the call site in `call_method.rs` keeps:
///
/// - `_filt`: `in` an array's elements (numbers), `out` a byte per element, 1 where the callback's
///   result is truthy (`ToBoolean` of a number: neither `0`, `-0` nor `NaN`).
/// - `_filt_typed`: the same over a view (`in` the view).
/// - `_fold`/`_fold_typed`: `in` a `struct { const void *elems; size_t count; double initial; }`,
///   `out` one double per leaf of `REDUCE_LEAF` elements, `[start, end)` a range of leaves; each
///   leaf folded left to right from `initial`, as `parallel.ts` folds it.
/// - `_comb`: `in` and `out` doubles, `out[k] = combine(in[2k], in[2k + 1])`.
#[allow(clippy::too_many_arguments)]
fn emit_family_chunks(
    output: &mut String,
    symbol: &str,
    clone_symbol: &str,
    index: usize,
    kinds: &[&str],
    boolean_result: bool,
    caps_decl: &str,
    index_arg: &str,
    capture_count: usize,
    linkage: &str,
) {
    use std::fmt::Write as _;
    let caps_args: String = (0..capture_count)
        .map(|j| format!(", pmap_caps[{j}]"))
        .collect();
    let leaf = crate::parallel::REDUCE_LEAF;
    let prologue = |name: &str| {
        format!(
            "{linkage}void __attribute__((unused)) {symbol}_pmap{name}_{index}(const void * restrict in, void * restrict out, size_t start, size_t end, void * restrict ctx) {{\n\
    if (KILN_UNLIKELY(__atomic_load_n((int *)ctx, __ATOMIC_RELAXED))) return;\n{caps_decl}\
    int failed = 0;\n"
        )
    };
    let epilogue = "    if (KILN_UNLIKELY(failed)) { __atomic_store_n((int *)ctx, 1, __ATOMIC_RELAXED); return; }\n}\n";
    // `ToBoolean` of the answer: a double (neither 0, -0 nor NaN), or a predicate's boxed value.
    let keep_stmt = if boolean_result {
        format!(
            "kiln_val r = {clone_symbol}_fn_{index}_p(1, d{index_arg}); failed |= (r == KILN_VAL_TYPED_FAIL); keep[i] = (uint8_t)kiln_c99_val_truthy(r);"
        )
    } else {
        format!(
            "double r = {clone_symbol}_fn_{index}_p(1, d{index_arg}); failed |= kiln_c99_typed_failed(r); keep[i] = (uint8_t)!(r == 0.0 || isnan(r));"
        )
    };
    if kinds.contains(&"_filt") {
        write!(
            output,
            "{}\
    const kiln_val * restrict in_arr = (const kiln_val * restrict)in;\n\
    uint8_t * restrict keep = (uint8_t * restrict)out;\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double d; memcpy(&d, &in_arr[i], sizeof d);\n\
        {keep_stmt}\n\
    }}\n{epilogue}",
            prologue("_filt")
        )
        .unwrap();
    }
    if kinds.contains(&"_filt_typed") {
        write!(
            output,
            "{}\
    const kiln_typed_array *view = (const kiln_typed_array *)in;\n\
    uint8_t * restrict keep = (uint8_t * restrict)out;\n\
    for (size_t i = start; i < end; ++i) {{\n\
        double d = kiln_typed_load(view, i);\n\
        {keep_stmt}\n\
    }}\n{epilogue}",
            prologue("_filt_typed")
        )
        .unwrap();
    }
    // `parallelReduce`'s callback: (accumulator, element) or (accumulator, element, index).
    let fold_args = if kinds.contains(&"_fold") && !kinds.contains(&"_comb") {
        format!(", (double)i{caps_args}")
    } else {
        caps_args.clone()
    };
    for (name, element) in [
        (
            "_fold",
            "({ double d; memcpy(&d, &((const kiln_val *)src->elems)[i], sizeof d); d; })",
        ),
        (
            "_fold_typed",
            "kiln_typed_load((const kiln_typed_array *)src->elems, i)",
        ),
    ] {
        if !kinds.contains(&name) {
            continue;
        }
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
            acc = {clone_symbol}_fn_{index}_p(1, acc, {element}{fold_args});\n\
            failed |= kiln_c99_typed_failed(acc);\n\
        }}\n\
        leaves[l] = acc;\n\
    }}\n{epilogue}",
            prologue(name)
        )
        .unwrap();
    }
    if kinds.contains(&"_comb") {
        write!(
            output,
            "{}\
    const double * restrict level = (const double * restrict)in;\n\
    double * restrict next = (double * restrict)out;\n\
    for (size_t k = start; k < end; ++k) {{\n\
        double r = {clone_symbol}_fn_{index}_p(1, level[2 * k], level[2 * k + 1]{caps_args});\n\
        failed |= kiln_c99_typed_failed(r);\n\
        next[k] = r;\n\
    }}\n{epilogue}",
            prologue("_comb")
        )
        .unwrap();
    }
}

pub(crate) use crate::graph::array_slots;

/// Whether every operation in `graph` has a lowering below. This is the one place that decides
/// what the emitter takes; `graph_for` asks it and fails the unit's emission otherwise.
pub fn can_lower(graph: &FunctionGraph) -> bool {
    first_unlowerable(graph).is_none()
}

/// The built-in member guards a typed body's refusing speculations rely on (issue 20): every
/// `kiln_c99_builtin_member_unmodified(..)` term between a relies mark and the refusal that
/// follows it. What the typed entry's callers check before entering it.
pub fn typed_relies(body: &str) -> std::collections::BTreeSet<String> {
    const TERM: &str = "kiln_c99_builtin_member_unmodified(";
    let mut found = std::collections::BTreeSet::new();
    let mut rest = body;
    while let Some(at) = rest.find(ops::call_method::TYPED_RELIES_MARK) {
        rest = &rest[at + ops::call_method::TYPED_RELIES_MARK.len()..];
        let end = rest.find("goto kiln_refusal;").unwrap_or(rest.len());
        let mut segment = &rest[..end];
        while let Some(start) = segment.find(TERM) {
            let term = &segment[start..];
            match term.find(')') {
                Some(close) => {
                    found.insert(term[..=close].to_owned());
                    segment = &term[close + 1..];
                }
                None => break,
            }
        }
        rest = &rest[end..];
    }
    found
}
