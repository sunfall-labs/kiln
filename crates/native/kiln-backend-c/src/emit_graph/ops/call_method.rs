use super::OpContext;
use super::callback_loops;
use super::{method_candidates, status_check};
use crate::diagnostic_source;
use crate::direct::TypedNeeds;
use crate::graph::{GraphOp, ValueId};
use crate::prelude::lookup_speculation_invariant;
use kiln_ir::Constant;
use std::fmt::Write;

/// The guard of a speculation on own members of built-ins (issue 20): each `(builtin, member)`
/// pair names the built-in by its `KILN_C99_BUILTIN_` suffix and the member it relies on. It holds
/// while the realm's program has written none of them, so a store onto another member or another
/// built-in leaves the speculation fast.
pub(crate) fn static_member_guard(members: &[(&str, &str)]) -> String {
    members
        .iter()
        .map(|(builtin, member)| {
            format!("kiln_c99_builtin_member_unmodified(KILN_C99_BUILTIN_{builtin}, \"{member}\")")
        })
        .collect::<Vec<_>>()
        .join(" && ")
}

/// ` && (guard)` for a call site about to enter proto `target`'s typed entry, when that entry
/// relies on built-ins it cannot fall back from ([`crate::direct::CallPlan::typed_guards`]);
/// empty otherwise.
pub(crate) fn typed_entry_guard(plan: &crate::direct::CallPlan, target: u32) -> String {
    match plan.typed_guards.get(target as usize) {
        Some(guard) if !guard.is_empty() => format!(" && ({guard})"),
        _ => String::new(),
    }
}

/// Marks a speculation in a typed entry whose failure can only refuse; see
/// [`crate::emit_graph::typed_relies`].
pub(crate) const TYPED_RELIES_MARK: &str = "/* kiln: typed entry relies on */";

/// An array method's fast path relies on Array.prototype (the method) and Array (its species),
/// with the prototypes every lookup falls through to folded in by the runtime.
pub(crate) const ARRAY_METHOD_GUARD: &str =
    "kiln_c99_builtin_pair_unmodified(KILN_C99_BUILTIN_ARRAY, KILN_C99_BUILTIN_ARRAY_PROTOTYPE)";

/// A string method's fast path relies on String.prototype (the method) and String.
pub(crate) const STRING_METHOD_GUARD: &str =
    "kiln_c99_builtin_pair_unmodified(KILN_C99_BUILTIN_STRING, KILN_C99_BUILTIN_STRING_PROTOTYPE)";

/// A callback argument of an intrinsic array loop (`map`, `filter`, `reduce`) whose proto this
/// compile *proves* -- `plan.known`, not a cache -- and whose declared arity the loop can call
/// typed. `arity` is the proto's own declared parameter count (1 or 2 for map/filter's element
/// and optional index; always 2 for reduce's accumulator and element), already checked against
/// the loop's own expectation by [`typed_callback`].
pub(super) struct TypedCallback {
    pub(super) target: u32,
    pub(super) needs: TypedNeeds,
    pub(super) returns_double: bool,
    pub(super) arity: u32,
    /// ` && (guard)` over the built-ins the callback's typed entry relies on (issue 20), for the
    /// loop's choice of its typed form; empty when there are none.
    pub(super) relies: String,
}

/// The callback argument's typed entry, when this callsite may call it typed: known by proto,
/// entered (referenced in [`crate::direct::DirectPlan::plan`]'s `callback_targets`, the same way
/// this reads it here), and declared at one of `expected_arity`'s parameter counts.
fn typed_callback(
    plan: &crate::direct::CallPlan,
    program: &kiln_ir::Program,
    callback: ValueId,
    expected_arity: &[u32],
) -> Option<TypedCallback> {
    let &target = plan.known.get(&callback)?;
    if !plan
        .typed_entries
        .get(target as usize)
        .copied()
        .unwrap_or(false)
    {
        return None;
    }
    let arity = program.functions[target as usize].parameter_count;
    if !expected_arity.contains(&arity) {
        return None;
    }
    Some(TypedCallback {
        target,
        needs: plan.typed_needs[target as usize],
        returns_double: plan.typed_returns[target as usize],
        arity,
        relies: typed_entry_guard(plan, target),
    })
}

/// What a parallel call site lowers to once [`parallel_site`] admits it: the protos of its
/// callbacks, in argument order (`parallelReduce`: the callback, then the combine), and for
/// `parallelSort` the order the pool runs.
struct ParallelSitePlan {
    method: crate::parallel::ParallelMethod,
    targets: Vec<u32>,
    order: Option<crate::parallel::SortOrder>,
    /// Per target: it reads the heap (tier 2), and runs as its heap clone's workers.
    heap: Vec<bool>,
}

/// The decision at one parallel call site (`parallelMap` and the family, ADR 0069): the plan the
/// site may run, or the first rule that kept it sequential, with the site's position -- one
/// function, so the lowering below and the record `--explain-parallel` reads are the same answer.
///
/// The order is the order the lowering tests: the unit's plan, the arguments, then per callback
/// its identity, its own verdict (`parallel::parallel_verdict`), the parameters this method
/// passes, its typed entry and the clone check in `lib.rs`. A `parallelSort` comparator is never
/// called on the pool -- the order it is proven to be is -- so it needs the verdict and a
/// proven order ([`crate::parallel::comparator_order`]), and nothing emitted.
fn parallel_site(
    plan: &crate::direct::CallPlan,
    program: &kiln_ir::Program,
    enclosing: Option<u32>,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    method: crate::parallel::ParallelMethod,
    args: &[ValueId],
    vid: ValueId,
) -> (crate::parallel::SiteRecord, Option<ParallelSitePlan>) {
    use crate::parallel::{ParallelMethod, Reason, Refusal, SiteRecord};
    let first = args.first().and_then(|a| plan.known.get(a)).copied();
    // Where the site is, even where the plan knows no callee (a generic unit knows none): the
    // closure written at the call.
    let written = args.first().and_then(|a| match defs.get(a) {
        Some(GraphOp::MakeFunction { index, .. }) => Some(*index),
        _ => None,
    });
    let (line, column) = if args.is_empty() {
        // No callback to place it by: the how-many-th such call of this body it is.
        let ordinal = defs
            .iter()
            .filter(|(id, op)| {
                id.0 < vid.0
                    && matches!(op, GraphOp::CallMethod { name, args, .. }
                        if args.is_empty()
                            && matches!(program.constants.get(*name as usize),
                                Some(Constant::String(k)) if k == method.name()))
            })
            .count();
        SiteRecord::locate_bare(program, method, enclosing, ordinal)
    } else {
        SiteRecord::locate(program, method, enclosing, first.or(written))
    };
    let mut record = SiteRecord {
        method,
        line,
        column,
        refusal: None,
        captures: 0,
        at: None,
        default_order: false,
        heap: false,
        alloc: false,
        calls: false,
    };
    let refuse = |record: &mut SiteRecord, callback: Option<u32>, refusal: Refusal| {
        if let (Some(cb), Some(name)) = (callback, refusal.detail.as_deref())
            && matches!(
                refusal.reason,
                Reason::CapturesReassigned | Reason::CapturesUnresolved | Reason::WritesCapture
            )
        {
            record.at = SiteRecord::find_name(program, cb, name);
        }
        record.refusal = Some(refusal);
    };
    // A generic-only unit lowers its qualifying sites unless it is a development build
    // (`DirectPlan::plan_parallel_sites`); the checks below then read the narrow plan.
    if plan.generic_only && !plan.parallel_in_generic {
        refuse(&mut record, None, Refusal::new(Reason::DevProfile));
        return (record, None);
    }
    // `parallelSort()`: a typed array's default order is numeric, and the pool runs it.
    if method == ParallelMethod::Sort && args.is_empty() {
        record.default_order = true;
        return (
            record,
            Some(ParallelSitePlan {
                method,
                targets: Vec::new(),
                order: None,
                heap: Vec::new(),
            }),
        );
    }
    let callbacks = method.callbacks(args.len());
    if callbacks.is_empty() {
        let reason = match method {
            ParallelMethod::Map | ParallelMethod::Filter | ParallelMethod::FlatMap
                if args.len() > 1 =>
            {
                Reason::ExtraArguments
            }
            ParallelMethod::Map | ParallelMethod::Filter | ParallelMethod::FlatMap => {
                Reason::CallbackUnknown
            }
            _ => Reason::ArgumentShape,
        };
        refuse(&mut record, None, Refusal::new(reason));
        return (record, None);
    }
    let mut targets = Vec::with_capacity(callbacks.len());
    let mut heap = Vec::with_capacity(callbacks.len());
    let mut order = None;
    for &(position, arities) in callbacks {
        let Some(target) = plan.known.get(&args[position]).copied() else {
            refuse(&mut record, None, Refusal::new(Reason::CallbackUnknown));
            return (record, None);
        };
        // The parameters this method passes, first: a callback declared for another method has
        // no typed entry or clone for this one, and its verdict would say so less plainly.
        let declared = program.functions[target as usize].parameter_count;
        if !arities.contains(&declared) {
            refuse(
                &mut record,
                Some(target),
                Refusal::with(Reason::Arity, format!("{declared} parameters")),
            );
            return (record, None);
        }
        let verdict = plan.parallel.get(target as usize).cloned();
        let shape = match verdict {
            Some(Ok(shape)) => shape,
            Some(Err(refusal)) => {
                refuse(&mut record, Some(target), refusal);
                return (record, None);
            }
            None => {
                refuse(
                    &mut record,
                    Some(target),
                    Refusal::new(Reason::CallbackUnknown),
                );
                return (record, None);
            }
        };
        // Tier 2: the heap clone, whose answers the workers check per element, and which needs
        // no typed entry. A comparator reads the heap only as a key comparator.
        if let Some(reads) = &shape.heap {
            // Tier 3 builds values only where the method keeps them (ADR 0069): `parallelReduce`
            // folds numbers and `parallelSort` compares keys.
            if matches!(method, ParallelMethod::Reduce | ParallelMethod::Sort) {
                if reads.allocates {
                    refuse(
                        &mut record,
                        Some(target),
                        Refusal::new(Reason::AllocatesForMethod),
                    );
                    return (record, None);
                }
                if reads.answers_value {
                    refuse(
                        &mut record,
                        Some(target),
                        Refusal::new(Reason::ReturnsNonNumber),
                    );
                    return (record, None);
                }
            } else if reads.builds() {
                record.alloc = true;
            }
            if reads.calls {
                record.calls = true;
            }
            if method == ParallelMethod::Sort {
                let Some((proven, _)) = reads.key else {
                    refuse(
                        &mut record,
                        Some(target),
                        Refusal::new(Reason::ComparatorUnproven),
                    );
                    return (record, None);
                };
                order = Some(proven);
            }
            record.heap = true;
            record.captures += reads.captures.len();
            targets.push(target);
            heap.push(true);
            continue;
        }
        heap.push(false);
        // A predicate's clone answers boxed, which only `parallelFilter`'s workers read.
        if shape.boolean_result
            && method != ParallelMethod::Filter
            && method != ParallelMethod::Sort
        {
            refuse(
                &mut record,
                Some(target),
                Refusal::new(Reason::ReturnsNonNumber),
            );
            return (record, None);
        }
        if method == ParallelMethod::Sort {
            let Some(proven) = shape.comparator else {
                refuse(
                    &mut record,
                    Some(target),
                    Refusal::new(Reason::ComparatorUnproven),
                );
                return (record, None);
            };
            order = Some(proven);
            targets.push(target);
            continue;
        }
        // The typed entry the sequential lowering and the clone's references hang off.
        if typed_callback(plan, program, args[position], arities).is_none() {
            refuse(&mut record, Some(target), Refusal::new(Reason::NotTyped));
            return (record, None);
        }
        if !plan
            .parallel_safe
            .get(target as usize)
            .copied()
            .unwrap_or(false)
        {
            refuse(
                &mut record,
                Some(target),
                Refusal::new(Reason::CloneRejected),
            );
            return (record, None);
        }
        record.captures += shape.captures.len();
        targets.push(target);
    }
    (
        record,
        Some(ParallelSitePlan {
            method,
            targets,
            order,
            heap,
        }),
    )
}

/// The tier-1 environment of one pooled callback (ADR 0069): the captured bindings its clone
/// reads, each written once in the unit, read here on the realm thread from `closure`'s upvalue
/// table -- after the closure is known to be the target, so the table is the one the proof is
/// about -- and required to be numbers; otherwise `{prefix}_caps_ok` stays clear and the
/// sequential lowering answers. Nothing can write them while the pool runs: the callback writes
/// nothing and the realm thread is blocked in the call.
///
/// Returns the declarations, the `ctx` pointer the chunk workers take, the failure flag they set,
/// and the name of the guard. The layout (`struct { int failed; double caps[N]; }`, the flag at
/// offset 0) is the one `emit_parallel_chunk` reads.
fn parallel_env(
    plan: &crate::direct::CallPlan,
    target: u32,
    closure: &str,
    prefix: &str,
) -> (String, String, String, String) {
    let captures = crate::emit_graph::parallel_captures(plan, target as usize);
    let ok = format!("{prefix}_caps_ok");
    if captures.is_empty() {
        return (
            format!("int {prefix}_failed = 0; int {ok} = 1;"),
            format!("&{prefix}_failed"),
            format!("{prefix}_failed"),
            ok,
        );
    }
    let mut decl = format!(
        "struct {{ int failed; double caps[{}]; }} {prefix}_env; {prefix}_env.failed = 0; int {ok} = 1;",
        captures.len()
    );
    for (j, k) in captures.iter().enumerate() {
        decl.push_str(&format!(
            " {{ kiln_val {prefix}_cap = *(const kiln_val *)KILN_VAL_PTR({closure}->upvalues[{k}]); if (KILN_VAL_IS_NUMBER({prefix}_cap)) memcpy(&{prefix}_env.caps[{j}], &{prefix}_cap, sizeof(double)); else {ok} = 0; }}"
        ));
    }
    (
        decl,
        format!("&{prefix}_env"),
        format!("{prefix}_env.failed"),
        ok,
    )
}

/// The pooled lowering of one admitted parallel site (see [`parallel_site`]): tried first,
/// leaving `handled` clear -- for the sequential lowering or the generic call after it --
/// whenever it does not answer. Shared by the specialized emission and the generic-only one
/// (issue 13). Every form stands down when the realm withholds kiln's extensions (ADR 0068) or
/// the current realm has written some built-in since its bootstrap (`kiln_c99_builtins_unmodified`), which
/// covers a program that replaced the method itself.
fn parallel_lowering(
    plan: &crate::direct::CallPlan,
    symbol: &str,
    site: &ParallelSitePlan,
    object: u32,
    args: &[ValueId],
    v: u32,
) -> String {
    use crate::parallel::ParallelMethod;
    let heap = |k: usize| site.heap.get(k).copied().unwrap_or(false);
    match site.method {
        ParallelMethod::Map if heap(0) => {
            heap_map_lowering(plan, symbol, site.targets[0], object, args[0].0, v, false)
        }
        ParallelMethod::Map => {
            parallel_pool_lowering(plan, symbol, site.targets[0], object, args[0].0, v, false)
        }
        // A heap callback's answers are flattened on the realm thread (tier 3).
        ParallelMethod::FlatMap if heap(0) => {
            heap_map_lowering(plan, symbol, site.targets[0], object, args[0].0, v, true)
        }
        // A numeric callback's result is never an array, so `flatMap` flattens nothing: its
        // answer is `map`'s. Arrays only: typed arrays have no `flatMap`.
        ParallelMethod::FlatMap => {
            parallel_pool_lowering(plan, symbol, site.targets[0], object, args[0].0, v, true)
        }
        ParallelMethod::Filter if heap(0) => {
            heap_filter_lowering(plan, symbol, site.targets[0], object, args[0].0, v)
        }
        ParallelMethod::Filter => {
            parallel_filter_lowering(plan, symbol, site.targets[0], object, args[0].0, v)
        }
        ParallelMethod::Reduce => parallel_reduce_lowering(
            plan,
            symbol,
            (site.targets[0], heap(0)),
            (site.targets[1], heap(1)),
            object,
            args,
            v,
        ),
        ParallelMethod::Sort if heap(0) => heap_sort_lowering(
            plan,
            symbol,
            site.targets[0],
            site.order.unwrap_or(crate::parallel::SortOrder::Ascending),
            object,
            args[0].0,
            v,
        ),
        ParallelMethod::Sort => parallel_sort_lowering(
            symbol,
            site.targets.first().copied(),
            site.order,
            object,
            args.first().map(|a| a.0),
            v,
        ),
    }
}

/// The tier-2 environment of one pooled callback (ADR 0069, parallel issue 07): a
/// `kiln_c99_par_env` holding the frozen heap, the captured bindings (each written once in the
/// unit; read here on the realm thread from `closure`'s upvalue table, of any kind -- the clone
/// reads them through the same guarded readers as an element -- and required to be past their
/// dead zone), the unit's interned keys the body reads by name, and its string literals, in
/// [`crate::parallel::HeapShape`]'s order. The lazy state the readers reach is built here, on
/// the realm thread, before any chunk runs (`kiln_c99_par_prepare`).
///
/// Nothing is allocated between building it and the pool's return: the caller allocates its
/// result first, so every pointer here stays what the workers read.
///
/// Returns the declarations, the `ctx` pointer, the failure flag and the guard, as
/// [`parallel_env`] does.
fn heap_env(
    plan: &crate::direct::CallPlan,
    symbol: &str,
    target: u32,
    closure: &str,
    prefix: &str,
) -> (String, String, String, String) {
    let shape = match plan.parallel.get(target as usize) {
        Some(Ok(shape)) => shape.heap.clone().unwrap_or_default(),
        _ => Default::default(),
    };
    let ok = format!("{prefix}_ok");
    let env = format!("{prefix}_env");
    let mut decl = format!(
        "kiln_val {prefix}_caps[{}]; const kiln_string *{prefix}_keys[{}]; kiln_val {prefix}_strs[{}]; const kiln_string *{prefix}_defs[{}];          kiln_c99_par_env {env}; memset(&{env}, 0, sizeof {env}); int {ok} = 1;          {env}.heap = kiln_c99_par_prepare(); {env}.caps = {prefix}_caps; {env}.keys = {prefix}_keys; {env}.strs = {prefix}_strs; {env}.defines = {prefix}_defs;",
        shape.captures.len().max(1),
        shape.keys.len().max(1),
        shape.strings.len().max(1),
        shape.defines.len().max(1),
    );
    for (j, k) in shape.captures.iter().enumerate() {
        decl.push_str(&format!(
            " {prefix}_caps[{j}] = *(const kiln_val *)KILN_VAL_PTR({closure}->upvalues[{k}]); if ({prefix}_caps[{j}] == KILN_VAL_UNINITIALIZED) {ok} = 0;"
        ));
    }
    for (j, name) in shape.keys.iter().enumerate() {
        decl.push_str(&format!(" {prefix}_keys[{j}] = {symbol}_key_{name};"));
    }
    for (j, index) in shape.strings.iter().enumerate() {
        decl.push_str(&format!(
            " {prefix}_strs[{j}] = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &{symbol}_string_{index});"
        ));
    }
    // Tier 3: the member names the callback defines, interned here as the sequential
    // definition would intern them, so no worker ever interns.
    for (j, index) in shape.defines.iter().enumerate() {
        decl.push_str(&format!(
            " if (({prefix}_defs[{j}] = kiln_c99_par_intern((const kiln_string *)&{symbol}_string_{index})) == NULL) {ok} = 0;"
        ));
    }
    // Tier 4: the bodies of the functions it calls through a closure value, which each clone
    // compares with the closure it finds before calling that function's clone.
    if !shape.bodies.is_empty() {
        decl.push_str(&format!(
            " kiln_body_fn {prefix}_bodies[{}];",
            shape.bodies.len()
        ));
        for (j, target) in shape.bodies.iter().enumerate() {
            decl.push_str(&format!(
                " {prefix}_bodies[{j}] = (kiln_body_fn){symbol}_fn_{target};"
            ));
        }
        decl.push_str(&format!(" {env}.bodies = {prefix}_bodies;"));
    }
    (decl, format!("&{env}"), format!("{env}.failed"), ok)
}

/// Whether the heap callback `target` builds values (tier 3): its typed-array workers, which get
/// no arena, would decline at the first allocation, so a site does not try them.
fn heap_builds(plan: &crate::direct::CallPlan, target: u32) -> bool {
    matches!(plan.parallel.get(target as usize), Some(Ok(shape)) if shape.heap.as_ref().is_some_and(|h| h.allocates))
}

/// The pooled heap `parallelMap` (and `parallelFlatMap`) of one site (tiers 2 and 3). Over an
/// array -- of anything; holes stay holes -- with no named properties, the workers write the
/// callback's answers into the result's elements: any value, including what they built in their
/// arenas (tier 3, ADR 0069), which are opened just before the pool and joined to the heap just
/// after it (`kiln_c99_par_arenas_open`/`_join`) -- or freed, when a worker declined or ran out of
/// its budget and the site runs the sequential method from the first element instead. The answer
/// is numeric unless a worker wrote something else (`env.mixed`). With a pool of one thread
/// (`kiln_parallel_threads`) the array branch stands down and `map`'s own inline-cached loop runs:
/// the boxed, guarded clone on one thread is slower than it (parallel issue 22). `parallelFlatMap` flattens the
/// answers on the realm thread once the arenas have joined (`kiln_c99_par_flatten`). Over a view
/// (`parallelMap` only; a callback that builds values does not try, its workers have no arena),
/// the answers convert as `ToNumber` converts them. Each pool run is bracketed by
/// `kiln_c99_par_section`, issue 06's write detector (a no-op unless `KILN_PARALLEL_PROTECT=1`),
/// except when the result's elements live inside its own heap block, which the workers write.
fn heap_map_lowering(
    plan: &crate::direct::CallPlan,
    symbol: &str,
    target: u32,
    object: u32,
    callback: u32,
    v: u32,
    flat: bool,
) -> String {
    let (env_decl, env_ptr, env_failed, env_ok) = heap_env(plan, symbol, target, "plain", "phmap");
    let typed_branch = if flat || heap_builds(plan, target) {
        String::new()
    } else {
        format!(
            "else if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY) {{ \
                 kiln_typed_array *view = (kiln_typed_array *)ptr; \
                 if (!view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64) {{ \
                     size_t count0 = view->length; \
                     kiln_value built_val; \
                     int status = kiln_c99_typed_alloc_ex(&built_val, view->kind, count0, 0); \
                     if (status != KILN_C99_OK) return status; \
                     kiln_typed_array *built_view = kiln_c99_typed_array_of(built_val); \
                     {env_decl} \
                     int phmap_ran = -1; \
                     if ({env_ok}) {{ \
                         kiln_c99_par_section(1); \
                         phmap_ran = kiln_parallel_map(view, count0, built_view, {symbol}_pmap_hmap_typed_{target}, {env_ptr}); \
                         kiln_c99_par_section(0); \
                     }} \
                     if (phmap_ran == 0 && !{env_failed}) {{ \
                         v{v} = kiln_val_from_boundary(built_val); \
                         handled = 1; \
                     }} \
                 }} \
             }}"
        )
    };
    let publish = if flat {
        format!(
            "built_arr->count = count0; \
             kiln_val phmap_flat; \
             if (kiln_c99_par_flatten(kiln_val_from_boundary(built_val), count0, &phmap_flat)) {{ \
                 v{v} = phmap_flat; \
                 handled = 1; \
             }} else {{ \
                 built_arr->count = 0; \
             }}"
        )
    } else {
        format!(
            "built_arr->count = count0; \
             built_arr->is_numeric = !phmap_env.mixed; \
             v{v} = kiln_val_from_boundary(built_val); \
             handled = 1;"
        )
    };
    format!(
        "if (!handled && kiln_c99_kiln_extensions_on() && kiln_c99_builtins_unmodified() && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
             void *ptr = KILN_VAL_PTR(v{o}); \
             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
             if (plain->body == (kiln_body_fn){symbol}_fn_{target} && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                 if (kiln_parallel_threads() > 1 && kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                     kiln_array *arr = (kiln_array *)ptr; \
                     if (arr->properties.tag == KILN_VALUE_UNDEFINED) {{ \
                         size_t count0 = arr->count; \
                         kiln_value built_val; \
                         int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                         if (status != KILN_C99_OK) return status; \
                         kiln_array *built_arr = kiln_c99_array_of(built_val); \
                         {env_decl} \
                         int phmap_joined = 0; \
                         if ({env_ok} && kiln_c99_par_arenas_open({env_ptr}, kiln_parallel_threads())) {{ \
                             int phmap_frozen = kiln_c99_array_elements_owned(built_arr); \
                             kiln_c99_par_section(phmap_frozen); \
                             int phmap_ran = kiln_parallel_map(arr->elements, count0, built_arr->elements, {symbol}_pmap_hmap_{target}, {env_ptr}); \
                             if (phmap_frozen) kiln_c99_par_section(0); \
                             phmap_joined = kiln_c99_par_arenas_join({env_ptr}, phmap_ran == 0 && !{env_failed}); \
                         }} \
                         if (phmap_joined) {{ {publish} }} \
                     }} \
                 }} {typed_branch} \
             }} \
         }}",
        o = object,
        a0 = callback,
    )
}

/// The pooled heap `parallelFilter` (tiers 2 and 3): the workers write a keep mask, and the realm
/// thread copies the kept elements -- whatever they are; the predicate only read them -- in index
/// order. Over an array the workers get arenas for whatever the predicate builds (tier 3); it is
/// garbage once the predicate has answered, and joins the heap to be collected like any other.
fn heap_filter_lowering(
    plan: &crate::direct::CallPlan,
    symbol: &str,
    target: u32,
    object: u32,
    callback: u32,
    v: u32,
) -> String {
    let (env_decl, env_ptr, env_failed, env_ok) = heap_env(plan, symbol, target, "plain", "phflt");
    // A predicate that builds values keeps them in arenas only over an array.
    let typed_branch = if heap_builds(plan, target) {
        String::new()
    } else {
        format!(
            "else if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY) {{ \
                 kiln_typed_array *view = (kiln_typed_array *)ptr; \
                 if (!view->buffer->detached && !view->is_buffer && view->kind <= KILN_TYPED_FLOAT64) {{ \
                     size_t count0 = view->length; \
                     {env_decl} \
                     uint8_t *phflt_mask = {env_ok} ? (uint8_t *)malloc(count0 + 1) : NULL; \
                     if (phflt_mask != NULL) {{ \
                         kiln_c99_par_section(1); \
                         int phflt_ran = kiln_parallel_map(view, count0, phflt_mask, {symbol}_pmap_hfilt_typed_{target}, {env_ptr}); \
                         kiln_c99_par_section(0); \
                         if (phflt_ran == 0 && !{env_failed}) {{ \
                             size_t kept = 0; \
                             for (size_t i = 0; i < count0; ++i) kept += phflt_mask[i]; \
                             kiln_value built_val; \
                             int status = kiln_c99_typed_alloc_ex(&built_val, view->kind, kept, 0); \
                             if (status != KILN_C99_OK) {{ free(phflt_mask); return status; }} \
                             kiln_typed_array *built_view = kiln_c99_typed_array_of(built_val); \
                             size_t width = view->kind <= KILN_TYPED_UINT8_CLAMPED ? 1 : view->kind <= KILN_TYPED_UINT16 ? 2 : view->kind <= KILN_TYPED_FLOAT32 ? 4 : 8; \
                             const uint8_t *src_bytes = view->buffer->bytes + view->byte_offset; \
                             uint8_t *dst_bytes = built_view->buffer->bytes + built_view->byte_offset; \
                             size_t n = 0; \
                             for (size_t i = 0; i < count0; ++i) if (phflt_mask[i]) {{ memcpy(dst_bytes + n * width, src_bytes + i * width, width); ++n; }} \
                             v{v} = kiln_val_from_boundary(built_val); \
                             handled = 1; \
                         }} \
                         free(phflt_mask); \
                     }} \
                 }} }}"
        )
    };
    format!(
        "if (!handled && kiln_c99_kiln_extensions_on() && kiln_c99_builtins_unmodified() && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
             void *ptr = KILN_VAL_PTR(v{o}); \
             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
             if (plain->body == (kiln_body_fn){symbol}_fn_{target} && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
             if (kiln_parallel_threads() > 1 && kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                 kiln_array *arr = (kiln_array *)ptr; \
                 if (arr->properties.tag == KILN_VALUE_UNDEFINED) {{ \
                     size_t count0 = arr->count; \
                     {env_decl} \
                     uint8_t *phflt_mask = {env_ok} ? (uint8_t *)malloc(count0 + 1) : NULL; \
                     if (phflt_mask != NULL && !kiln_c99_par_arenas_open({env_ptr}, kiln_parallel_threads())) {{ free(phflt_mask); phflt_mask = NULL; }} \
                     if (phflt_mask != NULL) {{ \
                         kiln_c99_par_section(1); \
                         int phflt_ran = kiln_parallel_map(arr->elements, count0, phflt_mask, {symbol}_pmap_hfilt_{target}, {env_ptr}); \
                         kiln_c99_par_section(0); \
                         if (kiln_c99_par_arenas_join({env_ptr}, phflt_ran == 0 && !{env_failed})) {{ \
                             size_t kept = 0; \
                             for (size_t i = 0; i < count0; ++i) kept += phflt_mask[i]; \
                             kiln_value built_val; \
                             int status = kiln_c99_new_array_with_capacity(&built_val, kept, 1); \
                             if (status != KILN_C99_OK) {{ free(phflt_mask); return status; }} \
                             kiln_array *built_arr = kiln_c99_array_of(built_val); \
                             const kiln_val *in_elems = arr->elements; \
                             size_t n = 0; \
                             int numeric = 1; \
                             for (size_t i = 0; i < count0; ++i) if (phflt_mask[i]) {{ numeric &= KILN_VAL_IS_NUMBER(in_elems[i]) ? 1 : 0; kiln_c99_heap_store_val(built_arr, &built_arr->elements[n++], in_elems[i]); }} \
                             built_arr->count = kept; \
                             built_arr->is_numeric = numeric; \
                             v{v} = kiln_val_from_boundary(built_val); \
                             handled = 1; \
                         }} \
                         free(phflt_mask); \
                     }} \
                 }} \
             }} {typed_branch} \
             }} \
         }}",
        o = object,
        a0 = callback,
    )
}

/// The pooled tier-2 `parallelSort` under a key comparator `(a, b) => K(a) - K(b)` (or its
/// reverse; [`crate::parallel::key_comparator`]): the workers compute every element's key; the
/// provider sorts a copy of the keys; the workers find each key's first place among them; and
/// the realm thread places the elements, those with equal keys in their original order. That is
/// a stable sort by `K` -- the unique answer of a stable sort under a consistent comparator, so
/// `sort`'s -- and it calls no comparator at all. A key that is not a number, or is `NaN`, or an
/// element that is `undefined` or a hole, declines the call and `sort` answers.
fn heap_sort_lowering(
    plan: &crate::direct::CallPlan,
    symbol: &str,
    target: u32,
    order: crate::parallel::SortOrder,
    object: u32,
    comparator: u32,
    v: u32,
) -> String {
    let (env_decl, env_ptr, env_failed, env_ok) = heap_env(plan, symbol, target, "plain", "phsenv");
    let negate = if order == crate::parallel::SortOrder::Descending {
        "for (size_t i = 0; i < count0; ++i) phsort_keys[i] = -phsort_keys[i]; "
    } else {
        ""
    };
    format!(
        "if (!handled && kiln_c99_kiln_extensions_on() && kiln_c99_builtins_unmodified() && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{c})) {{ \
             void *ptr = KILN_VAL_PTR(v{o}); \
             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{c}); \
             if (plain->body == (kiln_body_fn){symbol}_fn_{target} && depth < KILN_C99_MAX_CALL_DEPTH && kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                 kiln_array *arr = (kiln_array *)ptr; \
                 size_t count0 = arr->count; \
                 if (!arr->frozen && arr->properties.tag == KILN_VALUE_UNDEFINED && count0 <= (size_t)UINT32_MAX) {{ \
                     {env_decl} \
                     double *phsort_keys = {env_ok} ? (double *)malloc((2 * count0 + 2) * sizeof(double)) : NULL; \
                     uint32_t *phsort_ranks = phsort_keys != NULL ? (uint32_t *)calloc(2 * count0 + 2, sizeof(uint32_t)) : NULL; \
                     kiln_val *phsort_copy = phsort_ranks != NULL ? (kiln_val *)malloc((count0 + 1) * sizeof(kiln_val)) : NULL; \
                     int phsort_ran = -1; \
                     if (phsort_copy != NULL) {{ \
                         kiln_c99_par_section(1); \
                         phsort_ran = kiln_parallel_map(arr->elements, count0, phsort_keys, {symbol}_pmap_hkey_{target}, {env_ptr}); \
                         kiln_c99_par_section(0); \
                     }} \
                     if (phsort_ran == 0 && !{env_failed}) {{ \
                         double *phsort_sorted = phsort_keys + count0 + 1; \
                         {negate}\
                         if (count0 > 0) memcpy(phsort_sorted, phsort_keys, count0 * sizeof(double)); \
                         struct {{ const double *sorted; size_t count; }} phsort_order = {{ phsort_sorted, count0 }}; \
                         if (kiln_parallel_sort(phsort_sorted, count0, KILN_TYPED_FLOAT64, 1) == 0 && kiln_parallel_map(phsort_keys, count0, phsort_ranks, {symbol}_pmap_hrank_{target}, &phsort_order) == 0) {{ \
                             uint32_t *phsort_seen = phsort_ranks + count0 + 1; \
                             if (count0 > 0) memcpy(phsort_copy, arr->elements, count0 * sizeof(kiln_val)); \
                             for (size_t i = 0; i < count0; ++i) {{ uint32_t r = phsort_ranks[i]; arr->elements[r + phsort_seen[r]++] = phsort_copy[i]; }} \
                             v{v} = v{o}; \
                             handled = 1; \
                         }} \
                     }} \
                     free(phsort_copy); \
                     free(phsort_ranks); \
                     free(phsort_keys); \
                 }} \
             }} \
         }}",
        o = object,
        c = comparator,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_key_sort_uses_the_float64_kind_constant_for_its_scratch_keys() {
        let emitted = heap_sort_lowering(
            &crate::direct::CallPlan::default(),
            "fixture",
            2,
            crate::parallel::SortOrder::Ascending,
            0,
            1,
            3,
        );
        assert!(
            emitted.contains("kiln_parallel_sort(phsort_sorted, count0, KILN_TYPED_FLOAT64, 1)")
        );
        assert!(!emitted.contains("kiln_parallel_sort(phsort_sorted, count0, 8, 1)"));
    }
}

/// The pooled `parallelMap` (and `parallelFlatMap`) lowering of one site whose callback is proto
/// `target`: the chunk workers write the result array (or view) directly.
fn parallel_pool_lowering(
    plan: &crate::direct::CallPlan,
    symbol: &str,
    target: u32,
    object: u32,
    callback: u32,
    v: u32,
    arrays_only: bool,
) -> String {
    let (env_decl, env_ptr, env_failed, env_ok) = parallel_env(plan, target, "plain", "pmap");
    let typed_branch = if arrays_only {
        String::new()
    } else {
        format!(
            "else if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY) {{ \
                     kiln_typed_array *view = (kiln_typed_array *)ptr; \
                     kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                     if (kiln_c99_builtins_unmodified() && !kiln_c99_inheriting_new_target && !view->buffer->detached && depth < KILN_C99_MAX_CALL_DEPTH && plain->body == (kiln_body_fn){symbol}_fn_{target}) {{ \
                         {env_decl} \
                         if ({env_ok}) {{ \
                         size_t count0 = view->length; \
                         kiln_value built_val; \
                         int status = kiln_c99_typed_alloc_ex(&built_val, view->kind, count0, 0); \
                         if (status != KILN_C99_OK) return status; \
                         kiln_typed_array *built_view = kiln_c99_typed_array_of(built_val); \
                         const void * restrict src_bytes = (const void * restrict)(view->buffer->bytes + view->byte_offset); \
                         void * restrict dst_bytes = (void * restrict)(built_view->buffer->bytes + built_view->byte_offset); \
                         kiln_parallel_chunk_fn chunk_fn = NULL; \
                         switch (view->kind) {{ \
                         case KILN_TYPED_FLOAT64: chunk_fn = {symbol}_pmap_f64_{target}; break; \
                         case KILN_TYPED_FLOAT32: chunk_fn = {symbol}_pmap_f32_{target}; break; \
                         case KILN_TYPED_INT32:   chunk_fn = {symbol}_pmap_i32_{target}; break; \
                         case KILN_TYPED_UINT32:  chunk_fn = {symbol}_pmap_u32_{target}; break; \
                         case KILN_TYPED_INT16:   chunk_fn = {symbol}_pmap_i16_{target}; break; \
                         case KILN_TYPED_UINT16:  chunk_fn = {symbol}_pmap_u16_{target}; break; \
                         case KILN_TYPED_INT8:    chunk_fn = {symbol}_pmap_i8_{target}; break; \
                         case KILN_TYPED_UINT8:   chunk_fn = {symbol}_pmap_u8_{target}; break; \
                         case KILN_TYPED_UINT8_CLAMPED: chunk_fn = {symbol}_pmap_u8c_{target}; break; \
                         default: break; \
                         }} \
                         if (chunk_fn) {{ \
                             if (kiln_parallel_map(src_bytes, count0, dst_bytes, chunk_fn, {env_ptr}) == 0 && !{env_failed}) {{ \
                                 v{v} = kiln_val_from_boundary(built_val); \
                                 handled = 1; \
                             }} \
                         }} else {{ \
                             if (kiln_parallel_map(view, count0, built_view, {symbol}_pmap_typed_{target}, {env_ptr}) == 0 && !{env_failed}) {{ \
                                 v{v} = kiln_val_from_boundary(built_val); \
                                 handled = 1; \
                             }} \
                         }} \
                         }} \
                     }} \
                 }}",
            a0 = callback,
        )
    };
    format!(
        "if (!handled && kiln_c99_kiln_extensions_on() && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                 void *ptr = KILN_VAL_PTR(v{o}); \
                 if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                     kiln_array *arr = (kiln_array *)ptr; \
                     kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                     if (arr->is_numeric && kiln_c99_builtins_unmodified() && arr->properties.tag == KILN_VALUE_UNDEFINED && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH && plain->body == (kiln_body_fn){symbol}_fn_{target}) {{ \
                         {env_decl} \
                         if ({env_ok}) {{ \
                         size_t count0 = arr->count; \
                         kiln_value built_val; \
                         int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                         if (status != KILN_C99_OK) return status; \
                         kiln_array *built_arr = kiln_c99_array_of(built_val); \
                         const kiln_val * restrict in_elems = (const kiln_val * restrict)arr->elements; \
                         kiln_val * restrict out_elems = (kiln_val * restrict)built_arr->elements; \
                         if (kiln_parallel_map(in_elems, count0, out_elems, {symbol}_pmap_{target}, {env_ptr}) == 0 && !{env_failed}) {{ \
                             built_arr->count = count0; \
                             v{v} = kiln_val_from_boundary(built_val); \
                             handled = 1; \
                         }} \
                         }} \
                     }} \
                 }} {typed_branch} \
             }}",
        o = object,
        a0 = callback,
        v = v,
    )
}

/// The pooled `parallelFilter` lowering: the chunk workers write a keep mask (one byte per
/// element, the callback's result through `ToBoolean`), and the realm thread copies the kept
/// elements into the result in index order -- per-chunk answers concatenated in order, which is
/// `filter`'s. The mask is `malloc`'d: the collector never sees it.
fn parallel_filter_lowering(
    plan: &crate::direct::CallPlan,
    symbol: &str,
    target: u32,
    object: u32,
    callback: u32,
    v: u32,
) -> String {
    let (env_decl, env_ptr, env_failed, env_ok) = parallel_env(plan, target, "plain", "pflt");
    format!(
        "if (!handled && kiln_c99_kiln_extensions_on() && kiln_c99_builtins_unmodified() && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                 void *ptr = KILN_VAL_PTR(v{o}); \
                 kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                 if (plain->body == (kiln_body_fn){symbol}_fn_{target} && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                 if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                     kiln_array *arr = (kiln_array *)ptr; \
                     if (arr->is_numeric && arr->properties.tag == KILN_VALUE_UNDEFINED) {{ \
                         {env_decl} \
                         size_t count0 = arr->count; \
                         uint8_t *pflt_mask = {env_ok} ? (uint8_t *)malloc(count0 + 1) : NULL; \
                         if (pflt_mask != NULL) {{ \
                             if (kiln_parallel_map(arr->elements, count0, pflt_mask, {symbol}_pmap_filt_{target}, {env_ptr}) == 0 && !{env_failed}) {{ \
                                 size_t kept = 0; \
                                 for (size_t i = 0; i < count0; ++i) kept += pflt_mask[i]; \
                                 kiln_value built_val; \
                                 int status = kiln_c99_new_array_with_capacity(&built_val, kept, 1); \
                                 if (status != KILN_C99_OK) {{ free(pflt_mask); return status; }} \
                                 kiln_array *built_arr = kiln_c99_array_of(built_val); \
                                 const kiln_val *in_elems = arr->elements; \
                                 size_t n = 0; \
                                 for (size_t i = 0; i < count0; ++i) if (pflt_mask[i]) kiln_c99_heap_store_val(built_arr, &built_arr->elements[n++], in_elems[i]); \
                                 built_arr->count = kept; \
                                 v{v} = kiln_val_from_boundary(built_val); \
                                 handled = 1; \
                             }} \
                             free(pflt_mask); \
                         }} \
                     }} \
                 }} else if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY) {{ \
                     kiln_typed_array *view = (kiln_typed_array *)ptr; \
                     if (!view->buffer->detached && !view->is_buffer && view->kind <= KILN_TYPED_FLOAT64) {{ \
                         {env_decl} \
                         size_t count0 = view->length; \
                         uint8_t *pflt_mask = {env_ok} ? (uint8_t *)malloc(count0 + 1) : NULL; \
                         if (pflt_mask != NULL) {{ \
                             if (kiln_parallel_map(view, count0, pflt_mask, {symbol}_pmap_filt_typed_{target}, {env_ptr}) == 0 && !{env_failed}) {{ \
                                 size_t kept = 0; \
                                 for (size_t i = 0; i < count0; ++i) kept += pflt_mask[i]; \
                                 kiln_value built_val; \
                                 int status = kiln_c99_typed_alloc_ex(&built_val, view->kind, kept, 0); \
                                 if (status != KILN_C99_OK) {{ free(pflt_mask); return status; }} \
                                 kiln_typed_array *built_view = kiln_c99_typed_array_of(built_val); \
                                 size_t width = view->kind <= KILN_TYPED_UINT8_CLAMPED ? 1 : view->kind <= KILN_TYPED_UINT16 ? 2 : view->kind <= KILN_TYPED_FLOAT32 ? 4 : 8; \
                                 const uint8_t *src_bytes = view->buffer->bytes + view->byte_offset; \
                                 uint8_t *dst_bytes = built_view->buffer->bytes + built_view->byte_offset; \
                                 size_t n = 0; \
                                 for (size_t i = 0; i < count0; ++i) if (pflt_mask[i]) {{ memcpy(dst_bytes + n * width, src_bytes + i * width, width); ++n; }} \
                                 v{v} = kiln_val_from_boundary(built_val); \
                                 handled = 1; \
                             }} \
                             free(pflt_mask); \
                         }} \
                     }} \
                 }} \
                 }} \
             }}",
        o = object,
        a0 = callback,
        v = v,
    )
}

/// The pooled `parallelReduce(callback, combine, initial)` lowering (ADR 0069): the tree the
/// TypeScript definition applies, over the same leaves. The chunk workers fold whole leaves of
/// [`crate::parallel::REDUCE_LEAF`] from `initial` into one double each; then each level of the
/// tree is one more pooled call, `combine(r[2k], r[2k + 1])` for every pair, an odd last one
/// carried up. So the answer is fixed by the input length, whatever the core count, and equals
/// the interpreter's. `initial` must be a number here (the clones take doubles); anything else
/// is the sequential definition's.
fn parallel_reduce_lowering(
    plan: &crate::direct::CallPlan,
    symbol: &str,
    (fold, fold_heap): (u32, bool),
    (combine, combine_heap): (u32, bool),
    object: u32,
    args: &[ValueId],
    v: u32,
) -> String {
    // Each callback runs as its own tier's workers: tier 1's typed clone over numbers, or tier
    // 2's heap clone, whose fold reads any array (holes skipped, as the tree skips them).
    let (fold_decl, fold_ptr, fold_failed, fold_ok) = if fold_heap {
        heap_env(plan, symbol, fold, "plain", "pfold")
    } else {
        parallel_env(plan, fold, "plain", "pfold")
    };
    let (comb_decl, comb_ptr, comb_failed, comb_ok) = if combine_heap {
        heap_env(plan, symbol, combine, "pcomb_closure", "pcomb")
    } else {
        parallel_env(plan, combine, "pcomb_closure", "pcomb")
    };
    let (fold_kind, fold_array_guard) = if fold_heap {
        ("_hfold", "arr->properties.tag == KILN_VALUE_UNDEFINED")
    } else {
        (
            "_fold",
            "arr->is_numeric && arr->properties.tag == KILN_VALUE_UNDEFINED",
        )
    };
    let comb_kind = if combine_heap { "_hcomb" } else { "_comb" };
    // Issue 06's write detector brackets every pool run that reads the heap.
    let (section_on, section_off) = if fold_heap || combine_heap {
        ("kiln_c99_par_section(1);", "kiln_c99_par_section(0);")
    } else {
        ("", "")
    };
    let leaf = crate::parallel::REDUCE_LEAF;
    format!(
        "if (!handled && kiln_c99_kiln_extensions_on() && kiln_c99_builtins_unmodified() && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0}) && KILN_VAL_IS_CLOSURE(v{a1}) && KILN_VAL_IS_NUMBER(v{a2})) {{ \
                 void *ptr = KILN_VAL_PTR(v{o}); \
                 kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                 kiln_closure *pcomb_closure = (kiln_closure *)KILN_VAL_PTR(v{a1}); \
                 if (plain->body == (kiln_body_fn){symbol}_fn_{fold} && pcomb_closure->body == (kiln_body_fn){symbol}_fn_{combine} && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                     struct {{ const void *elems; size_t count; double initial; }} pfold_in = {{ NULL, 0, 0.0 }}; \
                     kiln_parallel_chunk_fn pfold_fn = NULL; \
                     memcpy(&pfold_in.initial, &v{a2}, sizeof(double)); \
                     if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                         kiln_array *arr = (kiln_array *)ptr; \
                         if ({fold_array_guard}) {{ pfold_in.elems = arr->elements; pfold_in.count = arr->count; pfold_fn = {symbol}_pmap{fold_kind}_{fold}; }} \
                     }} else if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY) {{ \
                         kiln_typed_array *view = (kiln_typed_array *)ptr; \
                         if (!view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64) {{ pfold_in.elems = view; pfold_in.count = view->length; pfold_fn = {symbol}_pmap{fold_kind}_typed_{fold}; }} \
                     }} \
                     if (pfold_fn != NULL) {{ \
                         {fold_decl} \
                         {comb_decl} \
                         size_t leaves = (pfold_in.count + {leaf} - 1) / {leaf}; \
                         if (leaves == 0) {{ \
                             v{v} = v{a2}; \
                             handled = 1; \
                         }} else if ({fold_ok} && {comb_ok}) {{ \
                             double *pfold_buf = (double *)malloc(2 * leaves * sizeof(double)); \
                             if (pfold_buf != NULL) {{ \
                                 {section_on} \
                                 int pfold_ran = kiln_parallel_map(&pfold_in, leaves, pfold_buf, pfold_fn, {fold_ptr}); \
                                 {section_off} \
                                 if (pfold_ran == 0 && !{fold_failed}) {{ \
                                     double *cur = pfold_buf; \
                                     double *nxt = pfold_buf + leaves; \
                                     size_t m = leaves; \
                                     int pfold_ok = 1; \
                                     while (m > 1) {{ \
                                         size_t pairs = m / 2; \
                                         {section_on} \
                                         int pcomb_ran = kiln_parallel_map(cur, pairs, nxt, {symbol}_pmap{comb_kind}_{combine}, {comb_ptr}); \
                                         {section_off} \
                                         if (pcomb_ran != 0 || {comb_failed}) {{ pfold_ok = 0; break; }} \
                                         if (m & 1) nxt[pairs] = cur[m - 1]; \
                                         m = pairs + (m & 1); \
                                         double *swap = cur; cur = nxt; nxt = swap; \
                                     }} \
                                     if (pfold_ok) {{ \
                                         v{v} = kiln_c99_val_from_double(cur[0]); \
                                         handled = 1; \
                                     }} \
                                 }} \
                                 free(pfold_buf); \
                             }} \
                         }} \
                     }} \
                 }} \
             }}",
        o = object,
        a0 = args[0].0,
        a1 = args[1].0,
        a2 = args[2].0,
        v = v,
    )
}

/// The pooled `parallelSort` lowering: a stable sort on the provider's pool
/// (`kiln_parallel_sort`) of a typed array's elements by the default order, or of a typed
/// array's or a numeric array's by a comparator proven to be `a - b` or `b - a`. The answer of a
/// stable sort under a consistent order is unique, so it is `sort`'s; a `NaN` under a comparator
/// (whose order `sort` leaves to the engine) makes the provider decline, and `sort` answers.
fn parallel_sort_lowering(
    symbol: &str,
    comparator: Option<u32>,
    order: Option<crate::parallel::SortOrder>,
    object: u32,
    comparator_value: Option<u32>,
    v: u32,
) -> String {
    let code = order.map_or(0, crate::parallel::SortOrder::provider_code);
    let (guard, identity) = match (comparator, comparator_value) {
        (Some(target), Some(value)) => (
            format!(" && KILN_VAL_IS_CLOSURE(v{value})"),
            format!(
                " && ((kiln_closure *)KILN_VAL_PTR(v{value}))->body == (kiln_body_fn){symbol}_fn_{target}"
            ),
        ),
        _ => (String::new(), String::new()),
    };
    // An array's default order compares strings: only a proven comparator sorts one here.
    let array_branch = if comparator.is_some() {
        format!(
            "else if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                 kiln_array *arr = (kiln_array *)ptr; \
                 if (arr->is_numeric && !arr->frozen && arr->properties.tag == KILN_VALUE_UNDEFINED && kiln_parallel_sort(arr->elements, arr->count, 100, {code}) == 0) {{ \
                     v{v} = v{object}; \
                     handled = 1; \
                 }} \
             }}"
        )
    } else {
        String::new()
    };
    format!(
        "if (!handled && kiln_c99_kiln_extensions_on() && kiln_c99_builtins_unmodified() && (v{object} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX{guard}{identity}) {{ \
             void *ptr = KILN_VAL_PTR(v{object}); \
             if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY) {{ \
                 kiln_typed_array *view = (kiln_typed_array *)ptr; \
                 if (!view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64 && kiln_parallel_sort(view->buffer->bytes + view->byte_offset, view->length, (int32_t)view->kind, {code}) == 0) {{ \
                     v{v} = v{object}; \
                     handled = 1; \
                 }} \
             }} {array_branch} \
         }}"
    )
}

/// The typed entry's own argument list, in its declared order: depth, then whatever
/// `TypedNeeds` says it reaches for beyond its parameters (upvalues, the receiver `vrecv`, a
/// `NULL` receiver-shape row -- this call site keeps none, exactly as the direct `Call` arm
/// does -- and the runtime), then one `double` per declared parameter, named by `params`.
fn typed_call_args(needs: TypedNeeds, params: &[&str]) -> String {
    typed_call_args_for(needs, "plain", "vrecv", params)
}

/// [`typed_call_args`] for the closure named `closure`, whose receiver is `recv`.
pub(super) fn typed_call_args_for(
    needs: TypedNeeds,
    closure: &str,
    recv: &str,
    params: &[&str],
) -> String {
    let mut args = String::from("depth + 1");
    if needs.upvalues {
        args.push_str(&format!(", {closure}->upvalues"));
    }
    if needs.receiver {
        args.push_str(&format!(", {recv}"));
    }
    if needs.row {
        args.push_str(", NULL");
    }
    if needs.runtime {
        args.push_str(", host, thrown");
    }
    for p in params {
        args.push_str(", ");
        args.push_str(p);
    }
    args
}

/// `map`'s typed loop: entered only when `arr->is_numeric` and the callback closure's own body
/// is this call's known proto -- both checked once, ahead of the loop, exactly as the module
/// doc for `DirectPlan` asks. Every element is then already a number (that is what
/// `is_numeric` means -- see `runtime/arrays.c` and the writes that clear it), so there is
/// nothing to test per element: read it as a double, call the typed entry, and box or keep the
/// result depending on what the entry proves it returns. `arr->count` is captured once, into
/// `count0`, before the loop -- unlike the generic loop below it, which re-reads `arr->count`
/// each iteration because a callback may mutate the array; `map` does not let such a mutation
/// change which indices are visited (the interpreter's own `generic_array_callback` captures the
/// length once too, for the same reason), so the built array's capacity, fixed to `count0` at
/// creation, is exactly what the loop writes.
/// The statements that settle a hole met by a call-site array loop. The loop opened with
/// `ARRAY_METHOD_GUARD`, `arr->properties.tag == UNDEFINED`, so while those still hold a hole has
/// no inherited indexed property and stays a hole; a callback may have changed that since (it can
/// write `Array.prototype[i]` or re-parent the receiver), in which case HasProperty/Get walk the
/// real chain exactly as the specification does for this index. `elem` is the kiln_val read from
/// the array (`KILN_VAL_HOLE` for a hole or an index past the live length) and is replaced by
/// the inherited value when there is one; it is still `KILN_VAL_HOLE` afterwards when the index
/// is genuinely absent.
fn inherited_element(o: u32, check: &str) -> String {
    format!(
        "if (elem == KILN_VAL_HOLE && KILN_UNLIKELY(!({ARRAY_METHOD_GUARD} && arr->properties.tag == KILN_VALUE_UNDEFINED))) {{ \
             kiln_value hole_key = kiln_c99_number((double)idx); int hole_present = 0; \
             int hole_status = kiln_c99_has_computed(host, kiln_val_to_boundary(v{o}), hole_key, &hole_present, depth, thrown); \
             if (hole_status != KILN_C99_OK) {{ int status = hole_status; {check} }} \
             if (hole_present) {{ \
                 kiln_value hole_value = kiln_c99_undefined(); \
                 hole_status = kiln_c99_get_computed(host, kiln_val_to_boundary(v{o}), hole_key, &hole_value, depth, thrown); \
                 if (hole_status != KILN_C99_OK) {{ int status = hole_status; {check} }} \
                 elem = kiln_val_from_boundary(hole_value); \
             }} \
         }}"
    )
}

fn emit_map_typed(
    symbol: &str,
    o: u32,
    a0: u32,
    v: u32,
    check: &str,
    typed_failed: &str,
    c: &TypedCallback,
    known_arr: bool,
    (this_arg, this_ok): (&str, &str),
) -> String {
    let target = c.target;
    let mut params = vec!["d_elem".to_string()];
    if c.arity == 2 {
        params.push("(double)idx".to_string());
    }
    let param_refs: Vec<&str> = params.iter().map(String::as_str).collect();
    let call_args = typed_call_args(c.needs, &param_refs);
    let result = if c.returns_double {
        format!(
            "double r_elem = {symbol}_fn_{target}_t({call_args}); \
             if (KILN_UNLIKELY(kiln_c99_typed_failed(r_elem))) {typed_failed} \
             out_elems[idx] = kiln_c99_val_from_double(r_elem);"
        )
    } else {
        format!(
            "kiln_val r_elem = {symbol}_fn_{target}_t({call_args}); \
             if (KILN_UNLIKELY(r_elem == KILN_VAL_TYPED_FAIL)) {typed_failed} \
             if (built_arr->is_numeric && !KILN_VAL_IS_NUMBER(r_elem)) built_arr->is_numeric = 0; \
             out_elems[idx] = r_elem; \
             built_arr->count = idx + 1;"
        )
    };
    // A callback that reaches the runtime can run anything, the receiver's own methods
    // included: it may grow the array (reallocating its elements), shrink it, or store a
    // non-number or a hole into it. The element is then read at its visit, an index past the
    // current length is a hole, and an element that is no longer a number goes through the
    // boxed body -- `map`'s own steps. A callback that reaches nothing keeps the plain loop.
    let resolve = inherited_element(o, check);
    let (elements, element, step) = if c.needs.runtime {
        (
            String::new(),
            "arr->elements[idx]".to_owned(),
            format!(
                "if (KILN_UNLIKELY(idx >= arr->count || !KILN_VAL_IS_NUMBER(arr->elements[idx]))) {{ \
                     kiln_val elem = idx < arr->count ? arr->elements[idx] : KILN_VAL_HOLE; \
                     {resolve} \
                     if (elem == KILN_VAL_HOLE) {{ \
                         out_elems[idx] = KILN_VAL_HOLE; built_arr->is_numeric = 0; built_arr->count = idx + 1; continue; \
                     }} \
                     kiln_val vargs[3] = {{ elem, kiln_c99_val_from_double((double)idx), v{o} }}; \
                     kiln_val prod = KILN_VAL_UNDEFINED; \
                     int status; \
                     if (!plain->publishes_frame) {{ kiln_host call_host; kiln_call_context ctx_call = {{ kiln_c99_closure_host(host, plain, &call_host), thrown, NULL, depth + 1 }}; status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)3); }} \
                     else status = kiln_c99_invoke_closure(host, &prod, plain, vrecv, vargs, (size_t)3, depth, thrown, NULL, 0); \
                     if (KILN_UNLIKELY(status != KILN_C99_OK)) {{ {check} }} \
                     if (built_arr->is_numeric && !KILN_VAL_IS_NUMBER(prod)) built_arr->is_numeric = 0; \
                     out_elems[idx] = prod; built_arr->count = idx + 1; continue; \
                 }}"
            ),
        )
    } else {
        (
            "const kiln_val * restrict in_elems = (const kiln_val * restrict)arr->elements;"
                .to_owned(),
            "in_elems[idx]".to_owned(),
            String::new(),
        )
    };
    let (header, footer) = if known_arr {
        (
            format!(
                "if (!handled && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                     kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                     kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                a0 = a0,
                o = o
            ),
            "}",
        )
    } else {
        (
            format!(
                "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                     void *ptr = KILN_VAL_PTR(v{o}); \
                     if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                         kiln_array *arr = (kiln_array *)ptr; \
                         kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                a0 = a0,
                o = o
            ),
            "} }",
        )
    };
    let relies = &c.relies;
    format!(
        "{header} \
             if (KILN_LIKELY({ARRAY_METHOD_GUARD} && arr->properties.tag == KILN_VALUE_UNDEFINED && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH && ({this_ok}))) {{ \
                 kiln_val vrecv = plain->arrow ? plain->captured_this : {this_arg}; \
                 kiln_c99_normalize_closure_receiver(&vrecv, plain); \
                 if (arr->is_numeric && plain->body == (kiln_body_fn){symbol}_fn_{target}{relies}) {{ \
                     size_t count0 = arr->count; \
                     kiln_value built_val; \
                     int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                     if (status != KILN_C99_OK) return status; \
                     kiln_array *built_arr = kiln_c99_array_of(built_val); \
                     {elements} \
                     kiln_val * restrict out_elems = (kiln_val * restrict)built_arr->elements; \
                     for (size_t idx = 0; idx < count0; ++idx) {{ \
                         {step} \
                         double d_elem; memcpy(&d_elem, &{element}, sizeof(d_elem)); \
                         {result} \
                     }} \
                     built_arr->count = count0; \
                     v{v} = kiln_val_from_boundary(built_val); \
                     handled = 1; \
                 }} else if (!plain->generator) {{ \
                     /* The length is read once: a callback that grows the array is not visited \
                      * past it (and would otherwise write past the result's capacity), and one \
                      * that shrinks it leaves holes, as `map` does. */ \
                     size_t count0 = arr->count; \
                     kiln_value built_val; \
                     int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                     if (status != KILN_C99_OK) return status; \
                     kiln_array *built_arr = kiln_c99_array_of(built_val); \
                     for (size_t idx = 0; idx < count0; ++idx) {{ \
                         kiln_val elem = idx < arr->count ? arr->elements[idx] : KILN_VAL_HOLE; \
                         {resolve} \
                         if (elem == KILN_VAL_HOLE) {{ \
                             kiln_c99_heap_store_val(built_arr, &built_arr->elements[built_arr->count++], KILN_VAL_HOLE); \
                             built_arr->is_numeric = 0; \
                             continue; \
                         }} \
                         kiln_val vargs[3] = {{ elem, kiln_c99_val_from_double((double)idx), v{o} }}; \
                         kiln_val prod = KILN_VAL_UNDEFINED; \
                         int status; \
                         if (!plain->publishes_frame) {{ kiln_host call_host; kiln_call_context ctx_call = {{ kiln_c99_closure_host(host, plain, &call_host), thrown, NULL, depth + 1 }}; status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)3); }} \
                         else status = kiln_c99_invoke_closure(host, &prod, plain, vrecv, vargs, (size_t)3, depth, thrown, NULL, 0); \
                         if (KILN_UNLIKELY(status != KILN_C99_OK)) {{ \
                             {check} \
                         }} \
                         if (built_arr->is_numeric && !KILN_VAL_IS_NUMBER(prod)) {{ \
                             built_arr->is_numeric = 0; \
                         }} \
                         kiln_c99_heap_store_val(built_arr, &built_arr->elements[built_arr->count++], prod); \
                     }} \
                     v{v} = kiln_val_from_boundary(built_val); \
                     handled = 1; \
                 }} \
             }} \
         {footer}"
    )
}

/// `filter`'s typed loop: the same proof and the same once-ahead-of-the-loop guards as `map`'s,
/// `count0` captured for the same reason (the interpreter's `generic_array_callback` captures
/// `filter`'s length once too). The element is snapshotted before the call -- `elem`, read once
/// -- and appended after it, exactly as the generic loop below already does, so a callback that
/// overwrites the very slot it is being called with still contributes the value it was called
/// with, not whatever is there afterward.
fn emit_filter_typed(
    symbol: &str,
    o: u32,
    a0: u32,
    v: u32,
    check: &str,
    typed_failed: &str,
    c: &TypedCallback,
    known_arr: bool,
    (this_arg, this_ok): (&str, &str),
) -> String {
    let target = c.target;
    let mut params = vec!["d_elem".to_string()];
    if c.arity == 2 {
        params.push("(double)idx".to_string());
    }
    let param_refs: Vec<&str> = params.iter().map(String::as_str).collect();
    let call_args = typed_call_args(c.needs, &param_refs);
    let resolve = inherited_element(o, check);
    let (call, truthy) = if c.returns_double {
        (
            format!(
                "double r_elem = {symbol}_fn_{target}_t({call_args}); \
                 if (KILN_UNLIKELY(kiln_c99_typed_failed(r_elem))) {typed_failed}"
            ),
            "(r_elem == r_elem && r_elem != 0.0)".to_string(),
        )
    } else {
        (
            format!(
                "kiln_val r_elem = {symbol}_fn_{target}_t({call_args}); \
                 if (KILN_UNLIKELY(r_elem == KILN_VAL_TYPED_FAIL)) {typed_failed}"
            ),
            "kiln_c99_val_truthy(r_elem)".to_string(),
        )
    };
    let (header, footer) = if known_arr {
        (
            format!(
                "if (!handled && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                     kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                     kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                a0 = a0,
                o = o
            ),
            "}",
        )
    } else {
        (
            format!(
                "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                     void *ptr = KILN_VAL_PTR(v{o}); \
                     if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                         kiln_array *arr = (kiln_array *)ptr; \
                         kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                a0 = a0,
                o = o
            ),
            "} }",
        )
    };
    let relies = &c.relies;
    format!(
        "{header} \
             if (KILN_LIKELY({ARRAY_METHOD_GUARD} && arr->properties.tag == KILN_VALUE_UNDEFINED && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH && ({this_ok}))) {{ \
                 kiln_val vrecv = plain->arrow ? plain->captured_this : {this_arg}; \
                 kiln_c99_normalize_closure_receiver(&vrecv, plain); \
                 if (arr->is_numeric && plain->body == (kiln_body_fn){symbol}_fn_{target}{relies}) {{ \
                     size_t count0 = arr->count; \
                     kiln_value built_val; \
                     int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                     if (status != KILN_C99_OK) return status; \
                     kiln_array *built_arr = kiln_c99_array_of(built_val); \
                     for (size_t idx = 0; idx < count0; ++idx) {{ \
                         kiln_val elem = arr->elements[idx]; \
                         double d_elem; memcpy(&d_elem, &elem, sizeof(d_elem)); \
                         {call} \
                         if ({truthy}) {{ kiln_c99_heap_store_val(built_arr, &built_arr->elements[built_arr->count++], elem); }} \
                     }} \
                     v{v} = kiln_val_from_boundary(built_val); \
                     handled = 1; \
                 }} else if (!plain->generator) {{ \
                     size_t count0 = arr->count; \
                     kiln_value built_val; \
                     int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                     if (status != KILN_C99_OK) return status; \
                     kiln_array *built_arr = kiln_c99_array_of(built_val); \
                     for (size_t idx = 0; idx < count0; ++idx) {{ \
                         kiln_val elem = idx < arr->count ? arr->elements[idx] : KILN_VAL_HOLE; \
                         {resolve} \
                         if (elem == KILN_VAL_HOLE) continue; \
                         kiln_val vargs[3] = {{ elem, kiln_c99_val_from_double((double)idx), v{o} }}; \
                         kiln_val prod = KILN_VAL_UNDEFINED; \
                         int status; \
                         if (!plain->publishes_frame) {{ kiln_host call_host; kiln_call_context ctx_call = {{ kiln_c99_closure_host(host, plain, &call_host), thrown, NULL, depth + 1 }}; status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)3); }} \
                         else status = kiln_c99_invoke_closure(host, &prod, plain, vrecv, vargs, (size_t)3, depth, thrown, NULL, 0); \
                         if (KILN_UNLIKELY(status != KILN_C99_OK)) {{ \
                             {check} \
                         }} \
                         if (kiln_c99_val_truthy(prod)) {{ \
                             if (built_arr->is_numeric && !KILN_VAL_IS_NUMBER(elem)) built_arr->is_numeric = 0; \
                             kiln_c99_heap_store_val(built_arr, &built_arr->elements[built_arr->count++], elem); \
                         }} \
                     }} \
                     v{v} = kiln_val_from_boundary(built_val); \
                     handled = 1; \
                 }} \
             }} \
         {footer}"
    )
}

/// `reduce`'s typed loop: reached only when the typed entry is also proven to return a number
/// (the accumulator must stay a `double` for the whole loop) and the initial value is proven a
/// number too, both checked in [`emit_call_method`] before this is called. `arr->count` is *not*
/// captured once here: the interpreter's own dense-array `reduce` re-reads the live length each
/// step (its own comment: "a callback that shortens the array stops the walk rather than
/// reading past the end"), and the generic `reduce` loop below does the same, so a callback that
/// grows the receiver during a typed `reduce` is visited the same way it is in both of those --
/// consistent with each other, not with the specification.
fn emit_reduce_typed(
    symbol: &str,
    site: (u32, u32, u32, u32),
    check: &str,
    typed_failed: &str,
    c: &TypedCallback,
    known_arr: bool,
    init_dacc: &str,
) -> String {
    let (o, a0, a1, v) = site;
    let target = c.target;
    let call_args = typed_call_args(c.needs, &["dacc", "d_elem"]);
    let resolve = inherited_element(o, check);
    let (header, footer) = if known_arr {
        (
            format!(
                "if (!handled && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                     kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                     kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                a0 = a0,
                o = o
            ),
            "}",
        )
    } else {
        (
            format!(
                "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                     void *ptr = KILN_VAL_PTR(v{o}); \
                     if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                         kiln_array *arr = (kiln_array *)ptr; \
                         kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                a0 = a0,
                o = o
            ),
            "} }",
        )
    };
    let relies = &c.relies;
    format!(
        "{header} \
             if (KILN_LIKELY({ARRAY_METHOD_GUARD} && arr->properties.tag == KILN_VALUE_UNDEFINED && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH)) {{ \
                 kiln_val vrecv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; \
                 kiln_c99_normalize_closure_receiver(&vrecv, plain); \
                 if (arr->is_numeric && plain->body == (kiln_body_fn){symbol}_fn_{target}{relies}) {{ \
                     double dacc = {init_dacc}; \
                     for (size_t idx = 0; idx < arr->count; ++idx) {{ \
                         double d_elem; memcpy(&d_elem, &arr->elements[idx], sizeof(d_elem)); \
                         double r_acc = {symbol}_fn_{target}_t({call_args}); \
                         if (KILN_UNLIKELY(kiln_c99_typed_failed(r_acc))) {typed_failed} \
                         dacc = r_acc; \
                     }} \
                     v{v} = kiln_c99_val_from_double(dacc); \
                     handled = 1; \
                 }} else if (!plain->generator) {{ \
                     kiln_val vacc = v{a1}; \
                     size_t count0 = arr->count; \
                     for (size_t idx = 0; idx < count0; ++idx) {{ \
                         kiln_val elem = idx < arr->count ? arr->elements[idx] : KILN_VAL_HOLE; \
                         {resolve} \
                         if (elem == KILN_VAL_HOLE) continue; \
                         kiln_val vargs[4] = {{ vacc, elem, kiln_c99_val_from_double((double)idx), v{o} }}; \
                         kiln_val prod = KILN_VAL_UNDEFINED; \
                         int status; \
                         if (!plain->publishes_frame) {{ kiln_host call_host; kiln_call_context ctx_call = {{ kiln_c99_closure_host(host, plain, &call_host), thrown, NULL, depth + 1 }}; status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)4); }} \
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
         {footer}"
    )
}

/// Whether `object` is the result of an array literal, so a loop can read it as a `kiln_array`
/// without testing its kind. Only a literal counts: the result of `map`, `filter`, `slice` or
/// `concat` is whatever the method returns, and with `Array.prototype.<m>` patched (or a
/// `Symbol.species` constructor) that is not an array, so those results get the kind test.
fn is_proven_array(
    object: ValueId,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    _program: &kiln_ir::Program,
) -> bool {
    let mut curr = object;
    for _ in 0..8 {
        match defs.get(&curr) {
            Some(GraphOp::NewArray { .. }) => return true,
            Some(GraphOp::StoreLocal { value, .. })
            | Some(GraphOp::StoreCell { value, .. })
            | Some(GraphOp::CheckTdz { value, .. }) => curr = *value,
            _ => return false,
        }
    }
    false
}

fn is_proven_string(
    object: ValueId,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    program: &kiln_ir::Program,
) -> bool {
    let mut curr = object;
    for _ in 0..8 {
        match defs.get(&curr) {
            Some(GraphOp::Const {
                constant: Constant::String(_),
                ..
            })
            | Some(GraphOp::Unary {
                op: kiln_ir::UnaryOp::TypeOf,
                ..
            }) => return true,
            Some(GraphOp::StoreLocal { value, .. })
            | Some(GraphOp::StoreCell { value, .. })
            | Some(GraphOp::CheckTdz { value, .. }) => curr = *value,
            Some(GraphOp::CallMethod {
                name,
                prepared_callee: None,
                ..
            }) => {
                let is_str_method = program
                    .constants
                    .get(*name as usize)
                    .and_then(|c| match c {
                        Constant::String(s) => s.to_scalar_string().ok(),
                        _ => None,
                    })
                    .is_some_and(|m| {
                        matches!(
                            m.as_str(),
                            "substring"
                                | "substr"
                                | "trim"
                                | "toLowerCase"
                                | "toUpperCase"
                                | "join"
                                | "charAt"
                        )
                    });
                return is_str_method;
            }
            _ => return false,
        }
    }
    false
}

/// `GraphOp::CallMethod { object, name, args, .. }`, moved out of `emit_graph_op`'s match
/// verbatim: a receiver-shape inline cache, this unit's own candidates by name, and a typed
/// entry for a candidate whose plan proved it -- lifted out only because the arm was the
/// largest thing in the match, not because it does anything the boxed comment above did not
/// already say.
pub(super) fn emit_call_method(
    output: &mut String,
    vid: ValueId,
    object: &ValueId,
    name: &u32,
    args: &[ValueId],
    line: u32,
    column: u32,
    prepared_callee: Option<ValueId>,
    ctx: &OpContext,
) {
    if let Some(callee) = prepared_callee {
        // A prepared call (the method was read before its arguments) runs the array loops below
        // too, behind an identity test on the saved callee (`prepared_original_member`). Units
        // that cannot use them, and the parallel family, keep the plain prepared call.
        let key = match ctx.program.constants.get(*name as usize) {
            Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
            _ => String::new(),
        };
        if ctx.plan.generic_only || ctx.parallel || key.starts_with("parallel") {
            emit_call_prepared_method(
                output, vid, object, name, &callee, args, line, column, ctx, None,
            );
            return;
        }
    }
    let v = vid.0;
    let program = ctx.program;
    let symbol = ctx.symbol;
    let diagnostic_site = ctx.diagnostic_site;
    let numbers = ctx.numbers;
    let ints = ctx.ints;
    let known_arr = is_proven_array(*object, ctx.defs, program);
    let known_str = is_proven_string(*object, ctx.defs, program);
    // In the parallel clone (ADR 0058, application 2) a `Math.<name>` call on the builtin
    // itself, with every argument proven a number, is the bare libm call and nothing else: no
    // generic fallback, because the clone runs on a thread with no realm to fall back to. The
    // site that enters the clone has already checked `kiln_c99_builtins_unmodified()` on the
    // realm thread, so `Math.<name>` is the table's. `parallel::parallel_safe` admits only these
    // calls into a clone, so any other `CallMethod` here would be a bug in that predicate; the
    // generic lowering below then mentions `host`, and the clone is rejected by the scan.
    if ctx.parallel
        && matches!(
            ctx.defs.get(object),
            Some(GraphOp::LoadBuiltin(kiln_ir::Builtin::Math))
        )
        && args.iter().all(|a| numbers.contains(a))
        && let Some(Constant::String(key)) = program.constants.get(*name as usize)
        && crate::parallel::PARALLEL_MATH
            .iter()
            .any(|(k, n)| key == *k && *n == args.len())
    {
        // The same expressions the guarded intrinsics below compute, so the clone and the
        // sequential lowering agree bit for bit on every edge (`-0`, NaN, the sign of a zero).
        let a = if ctx.live_numbers.contains(&args[0]) {
            format!("d{}", args[0].0)
        } else if ctx.live_ints.contains(&args[0]) {
            format!("(double)i{}", args[0].0)
        } else {
            format!("d{}", args[0].0)
        };
        let b = args
            .get(1)
            .map(|x| {
                if ctx.live_numbers.contains(x) {
                    format!("d{}", x.0)
                } else if ctx.live_ints.contains(x) {
                    format!("(double)i{}", x.0)
                } else {
                    format!("d{}", x.0)
                }
            })
            .unwrap_or_default();
        let call = match (key.to_scalar_string().ok().as_deref(), args.len()) {
            (Some("sqrt"), 1) => format!("sqrt({a})"),
            (Some("abs"), 1) => format!("fabs({a})"),
            (Some("floor"), 1) => format!("floor({a})"),
            (Some("ceil"), 1) => format!("ceil({a})"),
            (Some("trunc"), 1) => format!("trunc({a})"),
            (Some("round"), 1) => format!(
                "({{ double r = isfinite({a}) ? floor({a} + 0.5) : {a}; if (r == 0.0 && signbit({a})) r = -0.0; r; }})"
            ),
            (Some("sign"), 1) => {
                format!("(isnan({a}) || {a} == 0.0 ? {a} : ({a} < 0.0 ? -1.0 : 1.0))")
            }
            (Some("pow"), 2) => format!("pow({a}, {b})"),
            (Some("min"), 2) => format!(
                "((isnan({a}) || isnan({b})) ? NAN : (({a} < {b} || ({a} == 0.0 && {b} == 0.0 && signbit({a}))) ? {a} : {b}))"
            ),
            (Some("max"), 2) => format!(
                "((isnan({a}) || isnan({b})) ? NAN : (({a} > {b} || ({a} == 0.0 && {b} == 0.0 && !signbit({a}))) ? {a} : {b}))"
            ),
            (Some("sin"), 1) => format!("sin({a})"),
            (Some("cos"), 1) => format!("cos({a})"),
            (Some("tan"), 1) => format!("tan({a})"),
            (Some("asin"), 1) => format!("asin({a})"),
            (Some("acos"), 1) => format!("acos({a})"),
            (Some("atan"), 1) => format!("atan({a})"),
            (Some("atan2"), 2) => format!("atan2({a}, {b})"),
            (Some("sinh"), 1) => format!("sinh({a})"),
            (Some("cosh"), 1) => format!("cosh({a})"),
            (Some("tanh"), 1) => format!("tanh({a})"),
            (Some("asinh"), 1) => format!("asinh({a})"),
            (Some("acosh"), 1) => format!("acosh({a})"),
            (Some("atanh"), 1) => format!("atanh({a})"),
            (Some("exp"), 1) => format!("exp({a})"),
            (Some("expm1"), 1) => format!("expm1({a})"),
            (Some("log"), 1) => format!("log({a})"),
            (Some("log2"), 1) => format!("log2({a})"),
            (Some("log10"), 1) => format!("log10({a})"),
            (Some("log1p"), 1) => format!("log1p({a})"),
            (Some("cbrt"), 1) => format!("cbrt({a})"),
            (Some("hypot"), 2) => format!("hypot({a}, {b})"),
            _ => unreachable!("PARALLEL_MATH names only these"),
        };
        let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
        let live_typed = ctx.live_numbers.contains(&vid);
        let live_typed_int = ctx.live_ints.contains(&vid);
        if live_typed {
            let int_stmt = if live_typed_int {
                format!(" i{v} = (int64_t)d{v};")
            } else {
                String::new()
            };
            let box_stmt = if need_box {
                format!(" v{v} = kiln_c99_val_from_double(d{v});")
            } else {
                String::new()
            };
            writeln!(output, "    d{v} = {call};{int_stmt}{box_stmt}").unwrap();
        } else if live_typed_int {
            let box_stmt = if need_box {
                format!(" v{v} = kiln_c99_val_from_double((double)i{v});")
            } else {
                String::new()
            };
            writeln!(output, "    i{v} = (int64_t)({call});{box_stmt}").unwrap();
        } else {
            writeln!(output, "    v{v} = kiln_c99_val_from_double({call});").unwrap();
        }
        return;
    }
    let unwind = ctx.unwind;
    let plan = ctx.plan;
    let spec = ctx.spec;
    let typed = ctx.live_numbers.contains(&vid);
    let typed_int = ctx.live_ints.contains(&vid);
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|o| (o.pc, o.label.as_str()))
        .unwrap_or((0, "Op"));
    let diagnostic_source = diagnostic_source(diagnostic_site);
    writeln!(
        output,
        "    kiln_c99_js_frame_note_instruction({diagnostic_source}, (size_t){pc}, (size_t){line}, (size_t){column});"
    )
    .unwrap();
    let argc = args.len();
    let key_str = match program.constants.get(*name as usize) {
        Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
        _ => String::new(),
    };
    // Every parallel site (`parallelMap` and the family) says, in the C, what was decided for it
    // and why; see `parallel::SiteRecord`.
    let parallel_site = crate::parallel::ParallelMethod::from_name(&key_str).map(|method| {
        let (record, site) =
            parallel_site(plan, program, ctx.self_index, ctx.defs, method, args, vid);
        writeln!(output, "    {}", record.marker()).unwrap();
        site
    });
    if plan.generic_only {
        let n = argc.max(1);
        let init = if args.is_empty() {
            "KILN_VAL_UNDEFINED".to_owned()
        } else {
            args.iter()
                .map(|argument| format!("v{}", argument.0))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let check = status_check(diagnostic_site, pc, label, unwind);
        let sync_number = if typed {
            format!(" memcpy(&d{v}, &v{v}, sizeof(d{v}));")
        } else {
            String::new()
        };
        let sync_int = if typed_int {
            if typed {
                format!(" i{v} = (int64_t)d{v};")
            } else {
                format!(
                    " {{ double tmp_{v}; memcpy(&tmp_{v}, &v{v}, sizeof(tmp_{v})); i{v} = (int64_t)tmp_{v}; }}"
                )
            }
        } else {
            String::new()
        };
        let sync_number = format!("{sync_number}{sync_int}");
        // A qualifying `parallelMap` site in a generic-only unit (issue 13): the pooled
        // lowering first, and the generic call below only when it did not answer.
        let pool = parallel_site
            .as_ref()
            .and_then(Option::as_ref)
            .map(|site| parallel_lowering(plan, symbol, site, object.0, args, v));
        if let Some(pool) = &pool {
            writeln!(
                output,
                "    {{ int handled = 0; {pool} if (handled) {{{sync_number} }} else {{"
            )
            .unwrap();
        }
        let close = if pool.is_some() { " } }" } else { "" };
        if plan.compact {
            let key = format!("{symbol}_key_{name}");
            let o = object.0;
            if argc == 0 {
                writeln!(
                    output,
                    "    {{ static kiln_c99_site site = {{0}}; KU_CALL_METHOD(v{v}, v{o}, NULL, 0, {key}, &site, {pc}, \"{label}\", {unwind});{sync_number} }}{close}"
                )
                .unwrap();
            } else {
                for (i, arg) in args.iter().enumerate() {
                    writeln!(output, "    ku_args[{i}] = v{};", arg.0).unwrap();
                }
                writeln!(
                    output,
                    "    {{ static kiln_c99_site site = {{0}}; KU_CALL_METHOD(v{v}, v{o}, ku_args, {argc}, {key}, &site, {pc}, \"{label}\", {unwind});{sync_number} }}{close}"
                )
                .unwrap();
            }
            return;
        }
        writeln!(
            output,
            "    {{ static kiln_c99_site site = {{0}}; kiln_val call_args[{n}] = {{ {init} }}; int status = kiln_c99_val_call_method_key(host, v{o}, call_args, (size_t){argc}, {symbol}_key_{name}, depth, thrown, &site, {diagnostic_source}, (size_t){pc}, \"{label}\", &v{v}); {check}{sync_number} }}{close}",
            o = object.0,
            diagnostic_source = diagnostic_source,
        )
        .unwrap();
        return;
    }
    // The unit's own methods of this name: a cached callee whose body is one of them is
    // called by name, and the C compiler may inline it into this site.
    let candidates = method_candidates(
        program,
        program
            .constants
            .get(*name as usize)
            .and_then(|c| match c {
                Constant::String(s) => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| unreachable!("method name is a string constant")),
        &plan.publishes_frame,
    )
    .into_iter()
    .filter(|&index| plan.available.get(index as usize).copied().unwrap_or(true))
    .collect::<Vec<_>>();
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
        "if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) {{ if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(v{o}).tag; if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"CallMethod invocation refused\"; kiln_c99_note_refusal({diagnostic_source}, (size_t){pc}, \"{label}\", (size_t)kiln_c99_diagnostic_active_depth); }} else if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {{ if (kiln_c99_diagnostic_detail == NULL) {{ kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(v{o}).tag; kiln_c99_diagnostic_detail = \"CallMethod invocation threw\"; }} kiln_c99_note_refusal({diagnostic_source}, (size_t){pc}, \"{label}\", (size_t)kiln_c99_diagnostic_active_depth); }}",
        o = object.0
    );
    let typed_check = status_check(diagnostic_site, pc, label, unwind);
    let fn_id = ctx
        .self_index
        .map_or("entry".to_string(), |i| i.to_string());
    // Each arm is the candidate's typed call, entered when the callee in hand -- `plain` -- has
    // the candidate's code. The arms run twice over: first behind the candidate's method path
    // (receiver shape -> prototype shape -> slot, read live), then behind the site's cache.
    let typed_arms: Vec<(String, String)> = typed_candidates
                .iter()
                .map(|&i| {
                    let pfx = format!("{symbol}_fn_{fn_id}_v{v}_t_{i}");

                    let needs = plan.typed_needs[i as usize];
                    let mut call_args = String::from("depth + 1");
                    if needs.upvalues {
                        call_args.push_str(", plain->upvalues");
                    }
                    if needs.receiver {
                        call_args.push_str(&format!(", v{}", object.0));
                    }
                    // The callee's row for this receiver's shape, looked up (or claimed) here in
                    // the callee's own table (write-once rows, ADR 0071) by `_t_row`, so the
                    // callee does no lookup.
                    if needs.row {
                        call_args.push_str(", {ROW}");
                    }
                    if needs.runtime {
                        call_args.push_str(", host, thrown");
                    }
                    for a in args {
                        if ctx.live_numbers.contains(a) {
                            call_args.push_str(&format!(", d{}", a.0));
                        } else if ctx.live_ints.contains(a) {
                            call_args.push_str(&format!(", (double)i{}", a.0));
                        } else {
                            call_args.push_str(&format!(", d{}", a.0));
                        }
                    }
                    let failed = format!(
                        "{{ int status = kiln_c99_typed_status; if (status == KILN_C99_THROWN) *thrown = kiln_c99_typed_thrown; {diag} {typed_check} }}"
                    );
                    // ADR 0071: the candidate is recognised by its code, never by a closure
                    // object -- the site just read the callee live from where the method lives,
                    // and its body pointer says which function it is; the upvalues are that
                    // closure's own.
                    // Issue 20: the built-ins the entry relies on are asked about before it is
                    // entered; a no takes the boxed call below, as a callee without a typed
                    // entry would.
                    let relies = typed_entry_guard(plan, i);
                    let matched = format!(
                        "plain->body == (kiln_body_fn){symbol}_fn_{i} && kiln_typed_ok{relies}"
                    );
                    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
                    let box_stmt = if need_box { format!(" v{v} = kiln_c99_val_from_double(kiln_typed_d);") } else { String::new() };
                    let invoke = |row: &str, learn: &str| -> String {
                        let call_args = call_args.replace("{ROW}", row);
                        if plan.typed_returns[i as usize] {
                            let twin = if typed { format!(" d{v} = kiln_typed_d; kiln_twin = 1;") } else { String::new() };
                            let int_twin = if typed_int { format!(" i{v} = (int64_t)kiln_typed_d; kiln_twin = 1;") } else { String::new() };
                            format!(
                                "double kiln_typed_d = {symbol}_fn_{i}_t({call_args}); if (KILN_UNLIKELY(kiln_c99_typed_failed(kiln_typed_d))) {failed} else {{{box_stmt}{twin}{int_twin}{learn} }}"
                            )
                        } else {
                            format!(
                                "kiln_val kiln_typed_r = {symbol}_fn_{i}_t({call_args}); if (KILN_UNLIKELY(kiln_typed_r == KILN_VAL_TYPED_FAIL)) {failed} else {{ v{v} = kiln_typed_r;{learn} }}"
                            )
                        }
                    };
                    // ADR 0071, issue 04 step 3b: a call answered through the site's cache
                    // teaches the candidate's method path, when the entry that answered is a
                    // shaped prototype one step up and both shapes are shared.
                    // Issue 04 step 3c: only a prototype shape that records the candidate's code
                    // for the slot is learned.
                    let path_learn = if needs.row {
                        format!(
                            " kiln_c99_method_path_learn(&{pfx}_path, &site, obj, (kiln_body_fn){symbol}_fn_{i}, __row, 1);"
                        )
                    } else {
                        format!(
                            " kiln_c99_method_path_learn(&{pfx}_path, &site, obj, (kiln_body_fn){symbol}_fn_{i}, NULL, 0);"
                        )
                    };
                    let arm = |learn_path: &str, matched: &str| -> String {
                        if needs.row {
                            // The row for the receiver's shape is found -- or claimed -- here,
                            // before the entry is entered, so the callee's own lookup folds
                            // away. Issue 21: a receiver whose shape cannot take a row (its
                            // `this` fields are not the doubles the entry reads) is not handed
                            // to an entry that could only refuse at its door; the condition
                            // fails before anything has run and the boxed call below answers,
                            // as it would for a callee with no typed entry.
                            let known = invoke("__row", learn_path);
                            format!(
                                "if ({matched} && (__row = {symbol}_fn_{i}_t_row(obj)) != NULL) {{ {known} handled = 1; }}"
                            )
                        } else {
                            let plain_call = invoke("", learn_path);
                            format!("if ({matched}) {{ {plain_call} handled = 1; }}")
                        }
                    };
                    // The method path's hit (step 3c): the receiver's shape and its prototype's
                    // shape, whose record says the slot holds a closure of the candidate's code
                    // -- so the code is called without loading the closure, and the closure is
                    // read only when the call takes its upvalues. No walk of the site's entries.
                    // The row, when the candidate takes one, was learned with the path.
                    let front_closure = if needs.upvalues {
                        format!(
                            "kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(kiln_path_level->slots[{pfx}_path.slot]); "
                        )
                    } else {
                        String::new()
                    };
                    let front_call = if needs.row {
                        format!(
                            "const uint32_t *__row = {pfx}_path.row; if (__row == NULL) __builtin_unreachable(); {}",
                            invoke("__row", "")
                        )
                    } else {
                        invoke("", "")
                    };
                    let front = format!(
                        "if (!handled && obj->shape == kiln_c99_slot_key(&{pfx}_path.recv)) {{ const kiln_object *kiln_path_level; if (kiln_c99_method_path_hit(&{pfx}_path, obj, &kiln_path_level) && kiln_typed_ok{relies}) {{ {front_closure}{front_call} handled = 1; }} }} "
                    );
                    (front, format!("{} else ", arm(&path_learn, &matched)))
                })
                .collect();
    // A path starts with a receiver shape no object has (`KILN_C99_UNLEARNED_METHOD_PATH`) and
    // learns only shared shapes, so a dictionary-mode receiver's NULL shape never matches.
    let typed_front: String = typed_arms.iter().map(|(front, _)| front.as_str()).collect();
    let typed_chain: String = typed_arms.iter().map(|(_, chain)| chain.as_str()).collect();
    // One dispatch guard for every typed arm of the site: the constructor flag and the depth
    // are the same for each, and `depth` is a local only where a typed arm can be entered.
    let typed_guard_decl = if typed_candidates.is_empty() {
        String::new()
    } else {
        "int kiln_typed_ok = !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH; (void)kiln_typed_ok; const uint32_t *__row = NULL; (void)__row; ".to_owned()
    };
    let typed_remembered = typed_guard_decl;
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
    let init_stmts = if args.is_empty() {
        "call_args[0] = KILN_VAL_UNDEFINED;".to_owned()
    } else {
        args.iter()
            .enumerate()
            .map(|(i, a)| {
                if ctx.live_numbers.contains(a) {
                    format!("call_args[{i}] = kiln_c99_val_from_double(d{});", a.0)
                } else if ctx.live_ints.contains(a) {
                    format!(
                        "call_args[{i}] = kiln_c99_val_from_double((double)i{});",
                        a.0
                    )
                } else {
                    format!("call_args[{i}] = v{};", a.0)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let prep_args = format!("if (!call_args_ready) {{ {init_stmts} call_args_ready = 1; }} ");
    let check = status_check(diagnostic_site, pc, label, unwind);
    // `await Promise.resolve(number)` still has to suspend once, but the intermediate fulfilled
    // promise is unobservable when this call result is consumed only by that await. The graph
    // supplies this marker only for an exact builtin receiver and immediate await; the runtime
    // number check and epoch guard cover value type and writes to Promise/prototype stores. Any failed guard falls
    // through to the generic method call; this marked site deliberately does not use the older
    // Promise.resolve intrinsic because its result is immediately consumed by the await path.
    let await_resolve_intrinsic = if ctx.await_resolve_arg.is_some()
        && key_str == "resolve"
        && argc == 1
    {
        Some(format!(
            "if (KILN_LIKELY({guard} && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_PROMISE && KILN_VAL_IS_NUMBER(v{a0}))) {{ v{v} = v{a0}; handled = 1; }}",
            guard = static_member_guard(&[
                ("PROMISE", "resolve"),
                ("PROMISE_PROTOTYPE", "constructor")
            ]),
            o = object.0,
            a0 = args[0].0,
            v = v,
        ))
    } else {
        None
    };

    // 1. Math.* intrinsic
    let math_intrinsic = if await_resolve_intrinsic.is_none() {
        let load_arg = |a: ValueId, name: &str| -> (String, String) {
            if ctx.live_numbers.contains(&a) {
                (String::new(), format!("double {name} = d{};", a.0))
            } else if ctx.live_ints.contains(&a) {
                (String::new(), format!("double {name} = (double)i{};", a.0))
            } else if numbers.contains(&a) {
                (
                    String::new(),
                    format!("double {name}; memcpy(&{name}, &v{}, sizeof({name}));", a.0),
                )
            } else {
                (
                    format!("if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{}))) ", a.0),
                    format!("double {name}; memcpy(&{name}, &v{}, sizeof({name}));", a.0),
                )
            }
        };
        let (guard0, load0) = if argc >= 1 {
            load_arg(args[0], "d")
        } else {
            (String::new(), String::new())
        };
        let (guard2, load2) = if argc >= 2 {
            let (g0, l0) = load_arg(args[0], "ma0");
            let (g1, l1) = load_arg(args[1], "ma1");
            let guard = match (g0.is_empty(), g1.is_empty()) {
                (true, true) => String::new(),
                (false, true) => g0,
                (true, false) => g1,
                (false, false) => format!(
                    "if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}) && KILN_VAL_IS_NUMBER(v{a1}))) ",
                    a0 = args[0].0,
                    a1 = args[1].0
                ),
            };
            (guard, format!("{l0} {l1}"))
        } else {
            (String::new(), String::new())
        };

        match (key_str.as_str(), argc) {
            ("resolve", 1) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_PROMISE)) {{ \
                             kiln_val kiln_resolved; \
                             if (kiln_c99_val_promise_resolve(v{a0}, &kiln_resolved) == KILN_C99_OK) {{ \
                                 v{v} = kiln_resolved; \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                a0 = args[0].0,
                v = v
            )),
            ("sqrt", 1) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard0}{{ \
                                 {load0} \
                                 v{v} = kiln_c99_val_from_double(sqrt(d)); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard0 = guard0,
                load0 = load0,
                v = v
            )),
            ("floor", 1) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard0}{{ \
                                 {load0} \
                                 v{v} = kiln_c99_val_from_double(floor(d)); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard0 = guard0,
                load0 = load0,
                v = v
            )),
            ("ceil", 1) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard0}{{ \
                                 {load0} \
                                 v{v} = kiln_c99_val_from_double(ceil(d)); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard0 = guard0,
                load0 = load0,
                v = v
            )),
            ("round", 1) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard0}{{ \
                                 {load0} \
                                 double r = isfinite(d) ? floor(d + 0.5) : d; \
                                 if (r == 0.0 && signbit(d)) r = -0.0; \
                                 v{v} = kiln_c99_val_from_double(r); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard0 = guard0,
                load0 = load0,
                v = v
            )),
            ("trunc", 1) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard0}{{ \
                                 {load0} \
                                 v{v} = kiln_c99_val_from_double(trunc(d)); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard0 = guard0,
                load0 = load0,
                v = v
            )),
            ("abs", 1) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard0}{{ \
                                 {load0} \
                                 v{v} = kiln_c99_val_from_double(fabs(d)); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard0 = guard0,
                load0 = load0,
                v = v
            )),
            ("sign", 1) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard0}{{ \
                                 {load0} \
                                 v{v} = kiln_c99_val_from_double(isnan(d) || d == 0.0 ? d : (d < 0.0 ? -1.0 : 1.0)); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard0 = guard0,
                load0 = load0,
                v = v
            )),
            // `ToUint32` of a number: truncate, wrap modulo 2^32, and 0 for NaN and the
            // infinities -- the `int64_t` cast is exact below 2^63, and `fmod` takes the rest. An argument that is not a number leaves for the generic call, which
            // runs its `valueOf` -- converting it here with the pure `kiln_c99_as_number` answered
            // `Math.clz32({ valueOf() { return 3; } })` as 32 without calling it, and the
            // `(int64_t)` of a NaN or an infinity was undefined behaviour.
            ("clz32", 1) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard0}{{ \
                                 {load0} \
                                 uint32_t u0 = ((d > -9.2e18 && d < 9.2e18) ? (uint32_t)(int64_t)d : (isfinite(d) ? (uint32_t)(int64_t)fmod(trunc(d), 4294967296.0) : 0u)); \
                                 int32_t res = u0 ? (int32_t)__builtin_clz(u0) : 32; \
                                 v{v} = kiln_c99_val_from_double((double)res); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard0 = guard0,
                load0 = load0,
                v = v
            )),
            ("fround", 1) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard0}{{ \
                                 {load0} \
                                 v{v} = kiln_c99_val_from_double((double)(float)d); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard0 = guard0,
                load0 = load0,
                v = v
            )),
            // `ToInt32` of each argument, as `clz32` above: numbers only, wrapped rather than cast.
            ("imul", 2) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard2}{{ \
                                 {load2} \
                                 uint32_t i0 = ((ma0 > -9.2e18 && ma0 < 9.2e18) ? (uint32_t)(int64_t)ma0 : (isfinite(ma0) ? (uint32_t)(int64_t)fmod(trunc(ma0), 4294967296.0) : 0u)); \
                                 uint32_t i1 = ((ma1 > -9.2e18 && ma1 < 9.2e18) ? (uint32_t)(int64_t)ma1 : (isfinite(ma1) ? (uint32_t)(int64_t)fmod(trunc(ma1), 4294967296.0) : 0u)); \
                                 int32_t res = (int32_t)(i0 * i1); \
                                 v{v} = kiln_c99_val_from_double((double)res); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard2 = guard2,
                load2 = load2,
                v = v
            )),
            ("pow", 2) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard2}{{ \
                                 {load2} \
                                 v{v} = kiln_c99_val_from_double(pow(ma0, ma1)); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard2 = guard2,
                load2 = load2,
                v = v
            )),
            ("min", 2) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard2}{{ \
                                 {load2} \
                                 double res = (isnan(ma0) || isnan(ma1)) ? NAN : ((ma0 < ma1 || (ma0 == 0.0 && ma1 == 0.0 && signbit(ma0))) ? ma0 : ma1); \
                                 v{v} = kiln_c99_val_from_double(res); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard2 = guard2,
                load2 = load2,
                v = v
            )),
            ("max", 2) => Some(format!(
                "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                             {guard2}{{ \
                                 {load2} \
                                 double res = (isnan(ma0) || isnan(ma1)) ? NAN : ((ma0 > ma1 || (ma0 == 0.0 && ma1 == 0.0 && !signbit(ma0))) ? ma0 : ma1); \
                                 v{v} = kiln_c99_val_from_double(res); \
                                 handled = 1; \
                             }} \
                         }}",
                o = object.0,
                guard2 = guard2,
                load2 = load2,
                v = v
            )),
            ("sin", 1)
            | ("cos", 1)
            | ("tan", 1)
            | ("asin", 1)
            | ("acos", 1)
            | ("atan", 1)
            | ("sinh", 1)
            | ("cosh", 1)
            | ("tanh", 1)
            | ("asinh", 1)
            | ("acosh", 1)
            | ("atanh", 1)
            | ("exp", 1)
            | ("expm1", 1)
            | ("log", 1)
            | ("log2", 1)
            | ("log10", 1)
            | ("log1p", 1)
            | ("cbrt", 1) => {
                let func_name = &key_str;
                Some(format!(
                    "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                                 {guard0}{{ \
                                     {load0} \
                                     v{v} = kiln_c99_val_from_double({func_name}(d)); \
                                     handled = 1; \
                                 }} \
                             }}",
                    o = object.0,
                    guard0 = guard0,
                    load0 = load0,
                    v = v
                ))
            }
            ("atan2", 2) | ("hypot", 2) => {
                let func_name = &key_str;
                Some(format!(
                    "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                                 {guard2}{{ \
                                     {load2} \
                                     v{v} = kiln_c99_val_from_double({func_name}(ma0, ma1)); \
                                     handled = 1; \
                                 }} \
                             }}",
                    o = object.0,
                    guard2 = guard2,
                    load2 = load2,
                    v = v
                ))
            }
            _ => None,
        }
    } else {
        None
    };
    // A builtin's static method is what the table says only while the realm's program has not
    // written that member: `Math.max = f` and `Promise.resolve = f` are stores onto the member,
    // and the intrinsic above then takes the general call, which finds the override. A store onto
    // another member or another built-in (`Math.foo = 1`) leaves it alone (issue 20).
    let math_guard = if key_str == "resolve" {
        // `Promise.resolve(x)` is the member; the promise it makes answers `constructor` from
        // Promise.prototype when it is awaited or resolved with.
        static_member_guard(&[("PROMISE", "resolve"), ("PROMISE_PROTOTYPE", "constructor")])
    } else {
        static_member_guard(&[("MATH", key_str.as_str())])
    };
    let math_guarded = format!("if (KILN_LIKELY({math_guard})) {{ ");
    let math_intrinsic = math_intrinsic.map(|text| format!("{math_guarded}{text} }}"));

    // 1b. Array.isArray static predicate
    let array_is_array_intrinsic = if key_str == "isArray" {
        let arg_check = if argc == 0 {
            format!("v{v} = KILN_VAL_FALSE; handled = 1;")
        } else {
            let a0 = args[0].0;
            if numbers.contains(&args[0]) {
                format!("v{v} = KILN_VAL_FALSE; handled = 1;")
            } else {
                format!(
                    "if ((v{a0} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_LIKELY(kiln_c99_gc_kind_of_ptr(KILN_VAL_PTR(v{a0})) == KILN_GC_ARRAY)) {{ \
                         v{v} = KILN_VAL_TRUE; \
                         handled = 1; \
                     }} else if (KILN_VAL_IS_NUMBER(v{a0}) || KILN_VAL_IS_BOOLEAN(v{a0}) || KILN_VAL_IS_NULLISH(v{a0}) || KILN_VAL_IS_STRING(v{a0}) || ((v{a0} & KILN_TAG_MASK) == KILN_TAG_BIGINT_PREFIX) || ((v{a0} & KILN_TAG_MASK) == KILN_TAG_SYMBOL_PREFIX) || KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                         v{v} = KILN_VAL_FALSE; \
                         handled = 1; \
                     }}"
                )
            }
        };
        Some(format!(
            "if (KILN_LIKELY(kiln_c99_builtin_member_unmodified(KILN_C99_BUILTIN_ARRAY, \"isArray\") && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_ARRAY)) {{ \
                 {arg_check} \
             }}",
            o = object.0,
            arg_check = arg_check
        ))
    } else {
        None
    };

    // 1c. Number.* static predicates (isFinite, isNaN, isInteger, isSafeInteger)
    let nguard = static_member_guard(&[("NUMBER", key_str.as_str())]);
    let number_predicate_intrinsic = match (key_str.as_str(), argc) {
        ("isFinite" | "isNaN" | "isInteger" | "isSafeInteger", 0) => Some(format!(
            "if (KILN_LIKELY({nguard} && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_NUMBER)) {{ \
                 v{v} = KILN_VAL_FALSE; \
                 handled = 1; \
             }}",
            o = object.0,
            v = v
        )),
        ("isFinite", _) => {
            let a0 = args[0].0;
            let check = if ints.contains(&args[0]) {
                format!("v{v} = KILN_VAL_TRUE; handled = 1;")
            } else if ctx.live_numbers.contains(&args[0]) {
                format!("v{v} = isfinite(d{a0}) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1;")
            } else if numbers.contains(&args[0]) {
                format!(
                    "{{ double d; memcpy(&d, &v{a0}, sizeof(d)); v{v} = isfinite(d) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1; }}"
                )
            } else {
                format!(
                    "if (!KILN_VAL_IS_NUMBER(v{a0})) {{ v{v} = KILN_VAL_FALSE; handled = 1; }} \
                     else {{ double d; memcpy(&d, &v{a0}, sizeof(d)); v{v} = isfinite(d) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1; }}"
                )
            };
            Some(format!(
                "if (KILN_LIKELY({nguard} && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_NUMBER)) {{ \
                     {check} \
                 }}",
                o = object.0,
                check = check
            ))
        }
        ("isNaN", _) => {
            let a0 = args[0].0;
            let check = if ints.contains(&args[0]) {
                format!("v{v} = KILN_VAL_FALSE; handled = 1;")
            } else if ctx.live_numbers.contains(&args[0]) {
                format!("v{v} = isnan(d{a0}) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1;")
            } else if numbers.contains(&args[0]) {
                format!(
                    "{{ double d; memcpy(&d, &v{a0}, sizeof(d)); v{v} = isnan(d) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1; }}"
                )
            } else {
                format!(
                    "if (!KILN_VAL_IS_NUMBER(v{a0})) {{ v{v} = KILN_VAL_FALSE; handled = 1; }} \
                     else {{ double d; memcpy(&d, &v{a0}, sizeof(d)); v{v} = isnan(d) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1; }}"
                )
            };
            Some(format!(
                "if (KILN_LIKELY({nguard} && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_NUMBER)) {{ \
                     {check} \
                 }}",
                o = object.0,
                check = check
            ))
        }
        ("isInteger", _) => {
            let a0 = args[0].0;
            let check = if ints.contains(&args[0]) {
                format!("v{v} = KILN_VAL_TRUE; handled = 1;")
            } else if ctx.live_numbers.contains(&args[0]) {
                format!(
                    "v{v} = (isfinite(d{a0}) && floor(d{a0}) == d{a0}) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1;"
                )
            } else if numbers.contains(&args[0]) {
                format!(
                    "{{ double d; memcpy(&d, &v{a0}, sizeof(d)); v{v} = (isfinite(d) && floor(d) == d) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1; }}"
                )
            } else {
                format!(
                    "if (!KILN_VAL_IS_NUMBER(v{a0})) {{ v{v} = KILN_VAL_FALSE; handled = 1; }} \
                     else {{ double d; memcpy(&d, &v{a0}, sizeof(d)); v{v} = (isfinite(d) && floor(d) == d) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1; }}"
                )
            };
            Some(format!(
                "if (KILN_LIKELY({nguard} && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_NUMBER)) {{ \
                     {check} \
                 }}",
                o = object.0,
                check = check
            ))
        }
        ("isSafeInteger", _) => {
            let a0 = args[0].0;
            let check = if ints.contains(&args[0]) {
                format!("v{v} = KILN_VAL_TRUE; handled = 1;")
            } else if ctx.live_numbers.contains(&args[0]) {
                format!(
                    "v{v} = (isfinite(d{a0}) && floor(d{a0}) == d{a0} && fabs(d{a0}) <= 9007199254740991.0) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1;"
                )
            } else if numbers.contains(&args[0]) {
                format!(
                    "{{ double d; memcpy(&d, &v{a0}, sizeof(d)); v{v} = (isfinite(d) && floor(d) == d && fabs(d) <= 9007199254740991.0) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1; }}"
                )
            } else {
                format!(
                    "if (!KILN_VAL_IS_NUMBER(v{a0})) {{ v{v} = KILN_VAL_FALSE; handled = 1; }} \
                     else {{ double d; memcpy(&d, &v{a0}, sizeof(d)); v{v} = (isfinite(d) && floor(d) == d && fabs(d) <= 9007199254740991.0) ? KILN_VAL_TRUE : KILN_VAL_FALSE; handled = 1; }}"
                )
            };
            Some(format!(
                "if (KILN_LIKELY({nguard} && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_NUMBER)) {{ \
                     {check} \
                 }}",
                o = object.0,
                check = check
            ))
        }
        _ => None,
    };

    // 2. Array push intrinsic
    let push_intrinsic = if key_str == "push" && argc == 1 {
        let a0 = args[0].0;
        let is_num = numbers.contains(&args[0]);
        let numeric_check = if is_num {
            String::new()
        } else {
            format!("if (arr->is_numeric && !KILN_VAL_IS_NUMBER(v{a0})) arr->is_numeric = 0; ")
        };
        let push_arg = if is_num && !ctx.boxed.map_or(true, |b| b.contains(&args[0])) {
            if ctx.live_numbers.contains(&args[0]) {
                format!("kiln_c99_val_from_double(d{a0})")
            } else if ctx.live_ints.contains(&args[0]) {
                format!("kiln_c99_val_from_double((double)i{a0})")
            } else {
                format!("v{a0}")
            }
        } else {
            format!("v{a0}")
        };
        let (typed_ret, typed_fallback) = if typed {
            let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
            let box_stmt = if need_box {
                format!(" v{v} = kiln_c99_val_from_double(d{v});")
            } else {
                String::new()
            };
            let int_stmt = if typed_int {
                format!(" i{v} = (int64_t)d{v};")
            } else {
                String::new()
            };
            (
                format!("d{v} = (double)arr->count;{int_stmt}{box_stmt} kiln_twin = 1;"),
                format!("memcpy(&d{v}, &v{v}, sizeof(d{v}));{int_stmt}"),
            )
        } else if typed_int {
            let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
            let box_stmt = if need_box {
                format!(" v{v} = kiln_c99_val_from_double((double)i{v});")
            } else {
                String::new()
            };
            (
                format!("i{v} = (int64_t)arr->count;{box_stmt} kiln_twin = 1;"),
                format!(
                    "{{ double tmp_{v}; memcpy(&tmp_{v}, &v{v}, sizeof(tmp_{v})); i{v} = (int64_t)tmp_{v}; }}"
                ),
            )
        } else {
            (
                format!("v{v} = kiln_c99_val_from_double((double)arr->count);"),
                String::new(),
            )
        };
        if known_arr {
            Some(format!(
                "if (!handled) {{ \
                     kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                     if (KILN_LIKELY(!arr->frozen && !arr->not_extensible && arr->count < arr->capacity)) {{ \
                         {numeric_check}\
                         kiln_c99_heap_store_val(arr, &arr->elements[arr->count++], {push_arg}); \
                         {typed_ret} \
                         handled = 1; \
                     }} \
                 }} \
                 if (!handled && kiln_c99_val_array_push_fast(v{o}, {push_arg}, &v{v}) == KILN_C99_OK) {{ \
                     {typed_fallback} \
                     handled = 1; \
                 }}",
                o = object.0,
                push_arg = push_arg,
                numeric_check = numeric_check,
                typed_ret = typed_ret,
                typed_fallback = typed_fallback,
                v = v
            ))
        } else {
            Some(format!(
                "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {{ \
                     void *ptr = KILN_VAL_PTR(v{o}); \
                     if (KILN_LIKELY(kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY)) {{ \
                         kiln_array *arr = (kiln_array *)ptr; \
                         if (KILN_LIKELY(!arr->frozen && !arr->not_extensible && arr->count < arr->capacity)) {{ \
                             {numeric_check}\
                             kiln_c99_heap_store_val(arr, &arr->elements[arr->count++], {push_arg}); \
                             {typed_ret} \
                             handled = 1; \
                         }} \
                     }} \
                 }} \
                 if (!handled && kiln_c99_val_array_push_fast(v{o}, {push_arg}, &v{v}) == KILN_C99_OK) {{ \
                     {typed_fallback} \
                     handled = 1; \
                 }}",
                o = object.0,
                push_arg = push_arg,
                numeric_check = numeric_check,
                typed_ret = typed_ret,
                typed_fallback = typed_fallback,
                v = v
            ))
        }
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

    // The sequential method this site runs: `key_str` itself, or -- at a site of the parallel
    // family -- the method the family member means (`parallelMap` is `map`, ADR 0068 rule 3).
    // A family site lowers exactly as a call of that method would at this site, behind the pooled
    // lowering and behind `kiln_c99_kiln_extensions_on()` (a realm without kiln's members has no
    // `parallelMap` to call), so a call the pool declines at run time runs `map`'s own fastest
    // path here, never the generic call into the TypeScript definition (parallel issue 15). With
    // the built-ins untouched (`kiln_c99_builtins_unmodified()`, which the guard checks for the
    // `sort` loop, the one loop that does not check it itself) the member is the bootstrap's,
    // which calls that method. `parallelReduce` has no sequential method; its tree is
    // `callback_loops::reduce_tree`. `parallelFlatMap` has no lowering of its own: `flatMap` has
    // none either, and both call the same built-in.
    let family = crate::parallel::ParallelMethod::from_name(&key_str);
    let seq_key: &str = match family {
        Some(crate::parallel::ParallelMethod::Map) => "map",
        Some(crate::parallel::ParallelMethod::Filter) => "filter",
        Some(crate::parallel::ParallelMethod::Sort) => "sort",
        _ => key_str.as_str(),
    };
    let family_guard = |text: String| {
        if family.is_some() {
            format!(
                "if (kiln_c99_kiln_extensions_on() && kiln_c99_builtins_unmodified()) {{ {text} }}"
            )
        } else {
            text
        }
    };
    // `map` and `filter` take a thisArg: the callback's `this`, else undefined -- which a sloppy
    // callback sees as the global object. A sloppy callback given a primitive thisArg would see it
    // boxed; that case is left to the generic call.
    let callback_loop = (seq_key == "map" || seq_key == "filter") && (argc == 1 || argc == 2);
    let (this_arg, this_ok) = if argc == 2 {
        let t = args[1].0;
        (
            format!("v{t}"),
            format!("plain->arrow || plain->strict || KILN_VAL_IS_NULLISH(v{t})"),
        )
    } else {
        ("KILN_VAL_UNDEFINED".to_owned(), "1".to_owned())
    };
    let loop_site = callback_loops::LoopSite {
        symbol,
        object: object.0,
        callback: args.first().map_or(0, |a| a.0),
        v,
        check: &check,
        typed_failed: "",
        this_arg: &this_arg,
        this_ok: &this_ok,
    };

    // A callback whose proto this compile proves the identity of -- `plan.known`, not a cache --
    // and whose own declared arity the loop can call typed, is entered through its typed entry
    // directly from the loop, ahead of the generic (boxed) one. `DirectPlan::plan`'s
    // `callback_targets` references such a proto from the same call shapes read here, so
    // its typed entry exists exactly when one of these finds it.
    let map_typed = (seq_key == "map" && callback_loop)
        .then(|| typed_callback(plan, program, args[0], &[1, 2]))
        .flatten();
    let filter_typed = (seq_key == "filter" && callback_loop)
        .then(|| typed_callback(plan, program, args[0], &[1, 2]))
        .flatten();
    // `reduce`'s accumulator has to stay a `double` for the whole loop, so the typed loop is
    // reached only when the typed entry is proven to return one and the initial value is
    // already proven a number -- both checked here, not inside the loop.
    let reduce_typed = (key_str == "reduce" && argc == 2)
        .then(|| typed_callback(plan, program, args[0], &[2]))
        .flatten()
        .filter(|c| c.returns_double && numbers.contains(&args[1]));
    // On failure `kiln_c99_typed_status` holds the status and `kiln_c99_typed_thrown` the thrown
    // value when it is `KILN_C99_THROWN` -- the same failure handling the by-name typed
    // candidates above use, verbatim.
    let typed_failed = format!(
        "{{ int status = kiln_c99_typed_status; if (status == KILN_C99_THROWN) *thrown = kiln_c99_typed_thrown; {diag} {typed_check} }}"
    );
    let loop_site = callback_loops::LoopSite {
        typed_failed: &typed_failed,
        ..loop_site
    };
    // The receiver checks every array loop below opens with: a proven array needs none, anything
    // else must be an array block. Each loop's `{footer}` closes what its header opened.
    let array_header = |a0: u32| -> (String, &'static str) {
        if known_arr {
            (
                format!(
                    "if (!handled && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                         kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                         kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                    o = object.0
                ),
                "}",
            )
        } else {
            (
                format!(
                    "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                    o = object.0
                ),
                "} }",
            )
        }
    };
    // A typed array's loop, beside the array one wherever the receiver is not proven an array.
    let typed_array_loop = |text: String| if known_arr { String::new() } else { text };

    let map_intrinsic = callback_loop.then(|| {
        if seq_key != "map" {
            return String::new();
        }
        let array = match &map_typed {
            Some(c) => emit_map_typed(
                symbol,
                object.0,
                args[0].0,
                v,
                &check,
                &typed_failed,
                c,
                known_arr,
                (&this_arg, &this_ok),
            ),
            None => {
                let (header, footer) = array_header(args[0].0);
                format!(
                    "{header} \
                         if (KILN_LIKELY({ARRAY_METHOD_GUARD} && arr->properties.tag == KILN_VALUE_UNDEFINED && !plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH && ({this_ok}))) {{ \
                             /* `map`, element for element: the length read once, each element \
                              * read when it is visited (a callback may grow, shrink or \
                              * reallocate the array), and the result's length kept current so \
                              * the collector sees every value stored so far. The result is \
                              * numeric until a value that is not a number lands in it. */ \
                             size_t count0 = arr->count; \
                             kiln_value built_val; \
                             int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                             if (status != KILN_C99_OK) return status; \
                             kiln_array *built_arr = kiln_c99_array_of(built_val); \
                             kiln_val vrecv = plain->arrow ? plain->captured_this : {this_arg}; \
                             kiln_c99_normalize_closure_receiver(&vrecv, plain); \
                             for (size_t idx = 0; idx < count0; ++idx) {{ \
                                 kiln_val elem = idx < arr->count ? arr->elements[idx] : KILN_VAL_HOLE; \
                                 {resolve} \
                                 if (elem == KILN_VAL_HOLE) {{ \
                                     kiln_c99_heap_store_val(built_arr, &built_arr->elements[built_arr->count++], KILN_VAL_HOLE); \
                                     built_arr->is_numeric = 0; \
                                     continue; \
                                 }} \
                                 kiln_val prod = KILN_VAL_UNDEFINED; \
                                 {call} \
                                 if (built_arr->is_numeric && !KILN_VAL_IS_NUMBER(prod)) {{ \
                                     built_arr->is_numeric = 0; \
                                 }} \
                                 kiln_c99_heap_store_val(built_arr, &built_arr->elements[built_arr->count++], prod); \
                             }} \
                             v{v} = kiln_val_from_boundary(built_val); \
                             handled = 1; \
                         }} \
                     {footer}",
                    call = callback_loops::closure_call(
                        "plain",
                        "vrecv",
                        &[
                            "elem",
                            "kiln_c99_val_from_double((double)idx)",
                            &format!("v{}", object.0)
                        ],
                        "prod",
                        &check
                    ),
                    resolve = inherited_element(object.0, &check),
                )
            }
        };
        let typed_array =
            typed_array_loop(callback_loops::typed_array_map(&loop_site, map_typed.as_ref()));
        family_guard(format!("{array} {typed_array}"))
    });

    // The pooled `parallelMap` (ADR 0058, application 2) and the family's other pooled
    // lowerings (issue 10): the callback is this site's known proto, its body passes
    // `parallel::parallel_safe`, its typed entry returns a double, and its parallel clone (`_p`)
    // was emitted and scanned clean (`lib.rs`), so it can run on a provider pool thread that owns
    // no realm. `kiln_c99_builtins_unmodified()` is checked on the realm thread, because the
    // clone calls `Math.<name>` as bare libm. Tried before the sequential lowering and only when
    // the receiver qualifies; a typed entry that bails out, or a provider that could not run the
    // pool, leaves `handled` clear and the sequential lowering answers -- the same answer,
    // recomputed. Nothing is refused here. Every pooled form stands down when the realm withholds
    // kiln's extensions (ADR 0068): the call then goes through the ordinary lookup, which misses,
    // and throws the TypeError Node throws for a method that is not there.
    let parallel_pool_intrinsic = parallel_site
        .as_ref()
        .and_then(Option::as_ref)
        .map(|site| parallel_lowering(plan, symbol, site, object.0, args, v));

    let filter_intrinsic = callback_loop.then(|| {
        if seq_key != "filter" {
            return String::new();
        }
        let array = match &filter_typed {
            Some(c) => emit_filter_typed(
                symbol,
                object.0,
                args[0].0,
                v,
                &check,
                &typed_failed,
                c,
                known_arr,
                (&this_arg, &this_ok),
            ),
            None => {
                let (header, footer) = array_header(args[0].0);
                format!(
                    "{header} \
                         if (KILN_LIKELY({ARRAY_METHOD_GUARD} && arr->properties.tag == KILN_VALUE_UNDEFINED && !plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH && ({this_ok}))) {{ \
                             size_t count0 = arr->count; \
                             kiln_value built_val; \
                             int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                             if (status != KILN_C99_OK) return status; \
                             kiln_array *built_arr = kiln_c99_array_of(built_val); \
                             kiln_val vrecv = plain->arrow ? plain->captured_this : {this_arg}; \
                             kiln_c99_normalize_closure_receiver(&vrecv, plain); \
                             for (size_t idx = 0; idx < count0; ++idx) {{ \
                                 kiln_val elem = idx < arr->count ? arr->elements[idx] : KILN_VAL_HOLE; \
                                 {resolve} \
                                 if (elem == KILN_VAL_HOLE) continue; \
                                 kiln_val prod = KILN_VAL_UNDEFINED; \
                                 {call} \
                                 if (kiln_c99_val_truthy(prod)) {{ \
                                     if (built_arr->is_numeric && !KILN_VAL_IS_NUMBER(elem)) built_arr->is_numeric = 0; \
                                     kiln_c99_heap_store_val(built_arr, &built_arr->elements[built_arr->count++], elem); \
                                 }} \
                             }} \
                             v{v} = kiln_val_from_boundary(built_val); \
                             handled = 1; \
                         }} \
                     {footer}",
                    call = callback_loops::closure_call(
                        "plain",
                        "vrecv",
                        &[
                            "elem",
                            "kiln_c99_val_from_double((double)idx)",
                            &format!("v{}", object.0)
                        ],
                        "prod",
                        &check
                    ),
                    resolve = inherited_element(object.0, &check),
                )
            }
        };
        let typed_array = typed_array_loop(callback_loops::typed_array_filter(
            &loop_site,
            filter_typed.as_ref(),
        ));
        family_guard(format!("{array} {typed_array}"))
    });

    let reduce_init_dacc = |initial: ValueId| {
        if ctx.live_ints.contains(&initial) && !ctx.live_numbers.contains(&initial) {
            format!("(double)i{}", initial.0)
        } else {
            format!("d{}", initial.0)
        }
    };
    let reduce_intrinsic = if key_str == "reduce" && argc == 2 {
        let array = match &reduce_typed {
            Some(c) => emit_reduce_typed(
                symbol,
                (object.0, args[0].0, args[1].0, v),
                &check,
                &typed_failed,
                c,
                known_arr,
                &reduce_init_dacc(args[1]),
            ),
            None => {
                let (header, footer) = array_header(args[0].0);
                format!(
                    "{header} \
                         if (KILN_LIKELY({ARRAY_METHOD_GUARD} && arr->properties.tag == KILN_VALUE_UNDEFINED && !plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH)) {{ \
                             kiln_val vacc = v{a1}; \
                             kiln_val vrecv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; \
                             kiln_c99_normalize_closure_receiver(&vrecv, plain); \
                             size_t count0 = arr->count; \
                             for (size_t idx = 0; idx < count0; ++idx) {{ \
                                 kiln_val elem = idx < arr->count ? arr->elements[idx] : KILN_VAL_HOLE; \
                                 {resolve} \
                                 if (elem == KILN_VAL_HOLE) continue; \
                                 kiln_val prod = KILN_VAL_UNDEFINED; \
                                 {call} \
                                 vacc = prod; \
                             }} \
                             v{v} = vacc; \
                             handled = 1; \
                         }} \
                     {footer}",
                    a1 = args[1].0,
                    call = callback_loops::closure_call(
                        "plain",
                        "vrecv",
                        &[
                            "vacc",
                            "elem",
                            "kiln_c99_val_from_double((double)idx)",
                            &format!("v{}", object.0)
                        ],
                        "prod",
                        &check
                    ),
                    resolve = inherited_element(object.0, &check),
                )
            }
        };
        // `reduce` calls its callback with `this` undefined, whatever the site passes.
        let reduce_site = callback_loops::LoopSite {
            this_arg: "KILN_VAL_UNDEFINED",
            this_ok: "1",
            ..loop_site
        };
        let init = reduce_init_dacc(args[1]);
        let typed_array = typed_array_loop(callback_loops::typed_array_reduce(
            &reduce_site,
            args[1].0,
            reduce_typed.as_ref().map(|c| (c, init.as_str())),
        ));
        Some(format!("{array} {typed_array}"))
    } else if family == Some(crate::parallel::ParallelMethod::Reduce) && argc == 3 {
        // `parallelReduce` when the pool declines: its tree, sequentially, at `reduce`'s speed.
        // The typed tree needs both callbacks' typed entries (the fold's declared as `reduce`'s,
        // with the index or without) and an initial value that is a number at run time.
        let fold = typed_callback(plan, program, args[0], &[2, 3]);
        let combine = typed_callback(plan, program, args[1], &[2]);
        let reduce_site = callback_loops::LoopSite {
            this_arg: "KILN_VAL_UNDEFINED",
            this_ok: "1",
            ..loop_site
        };
        Some(callback_loops::reduce_tree(
            &reduce_site,
            args[1].0,
            args[2].0,
            fold.as_ref().zip(combine.as_ref()),
        ))
    } else {
        None
    };

    let slice_intrinsic = if key_str == "slice" && argc == 1 {
        let a0 = args[0].0;
        let (array_body, string_body, guard) = if ctx.live_ints.contains(&args[0]) {
            (
                format!(
                    "if (i{a0} >= 0) {{ \
                         size_t start = (size_t)i{a0}; \
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
                     }}",
                    a0 = a0,
                    v = v
                ),
                format!(
                    "if (i{a0} >= 0) {{ \
                         size_t start = (size_t)i{a0}; \
                         size_t slen = start < s_recv->unit_len ? s_recv->unit_len - start : 0; \
                         v{v} = kiln_c99_val_slice_string(s_recv, start, slen); \
                         handled = 1; \
                     }}",
                    a0 = a0,
                    v = v
                ),
                String::new(),
            )
        } else {
            (
                format!(
                    "double d0; memcpy(&d0, &v{a0}, sizeof(d0)); \
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
                     }}",
                    a0 = a0,
                    v = v
                ),
                format!(
                    "double d0; memcpy(&d0, &v{a0}, sizeof(d0)); \
                     if (d0 >= 0.0 && (double)(size_t)d0 == d0) {{ \
                         size_t start = (size_t)d0; \
                         size_t slen = start < s_recv->unit_len ? s_recv->unit_len - start : 0; \
                         v{v} = kiln_c99_val_slice_string(s_recv, start, slen); \
                         handled = 1; \
                     }}",
                    a0 = a0,
                    v = v
                ),
                format!(" && KILN_VAL_IS_NUMBER(v{a0})"),
            )
        };
        if known_arr {
            Some(format!(
                "if (!handled{guard}) {{ \
                     kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                     if (KILN_LIKELY({ARRAY_METHOD_GUARD} && arr->properties.tag == KILN_VALUE_UNDEFINED)) {{ \
                         {array_body} \
                     }} \
                 }}",
                o = object.0,
                guard = guard,
                array_body = array_body,
            ))
        } else if known_str {
            Some(format!(
                "if (!handled{guard}) {{ \
                     if (KILN_LIKELY({STRING_METHOD_GUARD})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                             {string_body} \
                         }} \
                     }} \
                 }}",
                o = object.0,
                guard = guard,
                string_body = string_body,
            ))
        } else {
            Some(format!(
                "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX{guard}) {{ \
                     void *ptr = KILN_VAL_PTR(v{o}); \
                     if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                         kiln_array *arr = (kiln_array *)ptr; \
                         if (KILN_LIKELY({ARRAY_METHOD_GUARD} && arr->properties.tag == KILN_VALUE_UNDEFINED)) {{ \
                             {array_body} \
                         }} \
                     }} \
                 }} else if (!handled && KILN_VAL_IS_STRING(v{o}){guard}) {{ \
                     if (KILN_LIKELY({STRING_METHOD_GUARD})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                             {string_body} \
                         }} \
                     }} \
                 }}",
                o = object.0,
                guard = guard,
                array_body = array_body,
                string_body = string_body
            ))
        }
    } else {
        None
    };

    let join_intrinsic = if key_str == "join" && argc == 1 {
        let (header, footer) = if known_arr {
            (
                format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{a0})) {{ \
                         kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o});",
                    a0 = args[0].0,
                    o = object.0
                ),
                "}",
            )
        } else {
            (
                format!(
                    "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_STRING(v{a0})) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr;",
                    a0 = args[0].0,
                    o = object.0
                ),
                "} }",
            )
        };
        Some(format!(
            "{header} \
                 if (KILN_LIKELY({ARRAY_METHOD_GUARD} && arr->properties.tag == KILN_VALUE_UNDEFINED)) {{ \
                     const kiln_string *sep = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                     size_t total_bytes = 0; \
                     int can_fast = (sep != NULL && !kiln_string_is_utf16(sep)); \
                     for (size_t idx = 0; idx < arr->count && can_fast; ++idx) {{ \
                         kiln_val elem = arr->elements[idx]; \
                         if (KILN_VAL_IS_STRING(elem)) {{ \
                             const kiln_string *es = (const kiln_string *)KILN_VAL_PTR(elem); \
                             if (es != NULL && !kiln_string_is_utf16(es)) total_bytes += es->unit_len; \
                             else can_fast = 0; \
                         }} else {{ can_fast = 0; }} \
                     }} \
                     if (can_fast) {{ \
                         if (arr->count > 1) total_bytes += (arr->count - 1) * sep->unit_len; \
                         char *buf = kiln_c99_gc_alloc_text(total_bytes); \
                         if (buf != NULL) {{ \
                             char *dst = buf; \
                             for (size_t idx = 0; idx < arr->count; ++idx) {{ \
                                 if (idx > 0 && sep->unit_len > 0) {{ \
                                     if (sep->unit_len == 1) {{ *dst = (char)kiln_string_unit_at(sep, 0); }} \
                                     else {{ kiln_c99_copy_short(dst, (const char *)kiln_string_latin1(sep), sep->unit_len); }} \
                                     dst += sep->unit_len; \
                                 }} \
                                 const kiln_string *es = (const kiln_string *)KILN_VAL_PTR(arr->elements[idx]); \
                                 if (es && es->unit_len > 0) {{ \
                                     kiln_c99_copy_short(dst, (const char *)kiln_string_latin1(es), es->unit_len); \
                                     dst += es->unit_len; \
                                 }} \
                             }} \
                             kiln_string *s = ((kiln_string *)buf) - 1; \
                             v{v} = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, s); \
                             handled = 1; \
                         }} \
                     }} \
                 }} \
             {footer}",
            header = header,
            a0 = args[0].0,
            v = v,
            footer = footer
        ))
    } else {
        None
    };

    let split_intrinsic = if key_str == "split" && argc == 1 {
        let str_check = if known_str {
            String::new()
        } else {
            format!("KILN_VAL_IS_STRING(v{o}) && ", o = object.0)
        };
        Some(format!(
            "if (!handled && KILN_LIKELY({STRING_METHOD_GUARD}) && {str_check}KILN_VAL_IS_STRING(v{a0})) {{ \
                         const kiln_string *src = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         const kiln_string *sep = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                         if (src != NULL && sep != NULL && !kiln_string_is_utf16(src) && !kiln_string_is_utf16(sep) && sep->unit_len == 1) {{ \
                             kiln_value built_val; \
                             int status = kiln_c99_split_string_ascii_byte(&built_val, src, (char)kiln_string_unit_at(sep, 0), (size_t)0xFFFFFFFFu); \
                             if (status == KILN_C99_OK) {{ \
                                 v{v} = KILN_VAL_TAG_PTR(KILN_TAG_BUFFER_PREFIX, (const void *)(uintptr_t)built_val.bits); \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
            str_check = str_check,
            o = object.0,
            a0 = args[0].0,
            v = v
        ))
    } else {
        None
    };

    let index_of_intrinsic = if key_str == "indexOf" && argc == 1 {
        let (int_stmt, double_stmt, box_stmt) = if typed {
            let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
            let box_stmt = if need_box {
                format!(" v{v} = kiln_c99_val_from_double(d{v});")
            } else {
                String::new()
            };
            let int_stmt = if typed_int {
                format!(
                    " i{v} = found ? (int64_t)(found - (const char *)kiln_string_latin1(s_recv)) : -1LL;"
                )
            } else {
                String::new()
            };
            (
                int_stmt,
                format!(
                    " d{v} = found ? (double)(found - (const char *)kiln_string_latin1(s_recv)) : -1.0;"
                ),
                format!("{box_stmt} kiln_twin = 1;"),
            )
        } else {
            (
                String::new(),
                String::new(),
                format!(
                    " v{v} = kiln_c99_val_from_double(found ? (double)(found - (const char *)kiln_string_latin1(s_recv)) : -1.0);"
                ),
            )
        };
        let str_check = if known_str {
            String::new()
        } else {
            format!("KILN_VAL_IS_STRING(v{o}) && ", o = object.0)
        };
        Some(format!(
            "if (!handled && KILN_LIKELY({STRING_METHOD_GUARD}) && {str_check}KILN_VAL_IS_STRING(v{a0})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         const kiln_string *s_ndl = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                         if (s_recv != NULL && s_ndl != NULL && !kiln_string_is_utf16(s_recv) && !kiln_string_is_utf16(s_ndl)) {{ \
                             const char *found = kiln_c99_memmem((const char *)kiln_string_latin1(s_recv), s_recv->unit_len, (const char *)kiln_string_latin1(s_ndl), s_ndl->unit_len); \
                             {int_stmt}{double_stmt}{box_stmt} \
                             handled = 1; \
                         }} \
                     }}",
            str_check = str_check,
            o = object.0,
            a0 = args[0].0,
            int_stmt = int_stmt,
            double_stmt = double_stmt,
            box_stmt = box_stmt
        ))
    } else {
        None
    };

    let substring_intrinsic = if key_str == "substring" && argc == 2 {
        let a0 = args[0].0;
        let a1 = args[1].0;
        let (guard, check_and_extract) = if ctx.live_ints.contains(&args[0])
            && ctx.live_ints.contains(&args[1])
        {
            (
                String::new(),
                format!(
                    "if (i{a0} >= 0 && i{a1} >= i{a0} && (size_t)i{a1} <= s_recv->unit_len) {{ \
                         size_t start = (size_t)i{a0}; \
                         size_t len = (size_t)i{a1} - start; \
                         v{v} = kiln_c99_val_slice_string(s_recv, start, len); \
                         handled = 1; \
                     }}",
                    a0 = a0,
                    a1 = a1,
                    v = v
                ),
            )
        } else {
            (
                format!(" && KILN_VAL_IS_NUMBER(v{a0}) && KILN_VAL_IS_NUMBER(v{a1})"),
                format!(
                    "double d0, d1; memcpy(&d0, &v{a0}, sizeof(d0)); memcpy(&d1, &v{a1}, sizeof(d1)); \
                     if (d0 >= 0.0 && (double)(size_t)d0 == d0 && d1 >= d0 && (double)(size_t)d1 == d1 && (size_t)d1 <= s_recv->unit_len) {{ \
                         size_t start = (size_t)d0; \
                         size_t len = (size_t)d1 - start; \
                         v{v} = kiln_c99_val_slice_string(s_recv, start, len); \
                         handled = 1; \
                     }}",
                    a0 = a0,
                    a1 = a1,
                    v = v
                ),
            )
        };
        let str_check = if known_str {
            String::new()
        } else {
            format!(" && KILN_VAL_IS_STRING(v{o})", o = object.0)
        };
        Some(format!(
            "if (!handled && KILN_LIKELY({STRING_METHOD_GUARD}){str_check}{guard}) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                             {check_and_extract} \
                         }} \
                     }}",
            str_check = str_check,
            o = object.0,
            guard = guard,
            check_and_extract = check_and_extract
        ))
    } else {
        None
    };

    let collection_intrinsic = match (key_str.as_str(), argc) {
        ("set", 2) => Some(format!(
            "if (!handled && ((v{o} & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX || (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {{ \
                             kiln_collection *col = (kiln_collection *)ptr; \
                             if (kiln_c99_collection_intrinsic_method_allowed(col, \"set\", sizeof(\"set\") - 1)) {{ \
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
                             if (kiln_c99_collection_intrinsic_method_allowed(col, \"get\", sizeof(\"get\") - 1)) {{ \
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
                             if (kiln_c99_collection_intrinsic_method_allowed(col, \"has\", sizeof(\"has\") - 1)) {{ \
                                 v{v} = kiln_c99_collection_has_val_fast(col, v{a0}); \
                                 handled = 1; \
                             }} \
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
                             if (kiln_c99_collection_intrinsic_method_allowed(col, \"add\", sizeof(\"add\") - 1)) {{ \
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
                             if (kiln_c99_collection_intrinsic_method_allowed(col, \"delete\", sizeof(\"delete\") - 1)) {{ \
                                 v{v} = kiln_c99_collection_delete_val_fast(col, v{a0}); \
                                 handled = 1; \
                             }} \
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
                             if (kiln_c99_collection_intrinsic_method_allowed(col, \"clear\", sizeof(\"clear\") - 1)) {{ \
                                 col->count = 0; \
                                 col->live = 0; \
                                 if (col->hash_table != NULL && col->hash_capacity > 0) {{ \
                                     memset(col->hash_table, 0, col->hash_capacity * sizeof(uint32_t)); \
                                 }} \
                                 v{v} = KILN_VAL_UNDEFINED; \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
            o = object.0,
            v = v
        )),
        _ => None,
    };

    let json_intrinsic = match (key_str.as_str(), argc) {
        ("stringify", 1) => Some(format!(
            "if (!handled && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_JSON && kiln_c99_builtin_member_unmodified(KILN_C99_BUILTIN_JSON, \"stringify\")) {{ \
                         if (kiln_c99_val_json_stringify_fast(host, v{a0}, &v{v}, depth, thrown) == KILN_C99_OK) {{ \
                             handled = 1; \
                         }} \
                     }}",
            o = object.0,
            a0 = args[0].0,
            v = v
        )),
        ("parse", 1) => Some(format!(
            "if (!handled && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_JSON && kiln_c99_builtin_member_unmodified(KILN_C99_BUILTIN_JSON, \"parse\")) {{ \
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

    let sort_intrinsic = if seq_key == "sort" && argc <= 1 {
        let cmp_expr = if argc == 1 {
            format!("v{}", args[0].0)
        } else {
            "KILN_VAL_UNDEFINED".to_string()
        };
        let text = if known_arr {
            format!(
                "if (!handled) {{ \
                     kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                     if (kiln_c99_val_array_sort_fast(host, arr, {cmp}, depth, thrown) == KILN_C99_OK) {{ \
                         v{v} = v{o}; \
                         handled = 1; \
                     }} \
                 }}",
                o = object.0,
                cmp = cmp_expr,
                v = v
            )
        } else {
            format!(
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
            )
        };
        Some(family_guard(text))
    } else {
        None
    };

    let array_extra_intrinsic = match (key_str.as_str(), argc) {
        ("pop", 0) => {
            if known_arr {
                Some(format!(
                    "if (!handled) {{ \
                         kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                         if (!arr->frozen && !arr->not_extensible) {{ \
                             v{v} = arr->count > 0 ? arr->elements[--arr->count] : KILN_VAL_UNDEFINED; \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    v = v
                ))
            } else {
                Some(format!(
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
                ))
            }
        }
        ("shift", 0) => {
            if known_arr {
                Some(format!(
                    "if (!handled) {{ \
                         kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
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
                     }}",
                    o = object.0,
                    v = v
                ))
            } else {
                Some(format!(
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
                ))
            }
        }
        ("includes", 1) => {
            if known_arr {
                Some(format!(
                    "if (!handled) {{ \
                         kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
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
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                ))
            } else if known_str {
                Some(format!(
                    "if (!handled && KILN_VAL_IS_STRING(v{a0})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         const kiln_string *s_ndl = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                         if (s_recv != NULL && s_ndl != NULL && !kiln_string_is_utf16(s_recv) && !kiln_string_is_utf16(s_ndl)) {{ \
                             const char *found = kiln_c99_memmem((const char *)kiln_string_latin1(s_recv), s_recv->unit_len, (const char *)kiln_string_latin1(s_ndl), s_ndl->unit_len); \
                             v{v} = found ? KILN_VAL_TRUE : KILN_VAL_FALSE; \
                             handled = 1; \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                ))
            } else {
                Some(format!(
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
                             if (s_recv != NULL && s_ndl != NULL && !kiln_string_is_utf16(s_recv) && !kiln_string_is_utf16(s_ndl)) {{ \
                                 const char *found = kiln_c99_memmem((const char *)kiln_string_latin1(s_recv), s_recv->unit_len, (const char *)kiln_string_latin1(s_ndl), s_ndl->unit_len); \
                                 v{v} = found ? KILN_VAL_TRUE : KILN_VAL_FALSE; \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
                    o = object.0,
                    a0 = args[0].0,
                    v = v
                ))
            }
        }
        _ => None,
    };

    let string_extra_intrinsic = match (key_str.as_str(), argc) {
        ("startsWith", 1) => {
            let str_check = if known_str {
                String::new()
            } else {
                format!("KILN_VAL_IS_STRING(v{o}) && ", o = object.0)
            };
            Some(format!(
                "if (!handled && {str_check}KILN_VAL_IS_STRING(v{a0})) {{ \
                             const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                             const kiln_string *s_ndl = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                             if (s_recv != NULL && s_ndl != NULL) {{ \
                                 int sw = s_recv->unit_len >= s_ndl->unit_len; \
                                 if (sw && !kiln_string_is_utf16(s_recv) && !kiln_string_is_utf16(s_ndl)) {{ \
                                     sw = memcmp(kiln_string_latin1(s_recv), kiln_string_latin1(s_ndl), s_ndl->unit_len) == 0; \
                                 }} else if (sw) {{ \
                                     for (size_t index = 0; index < s_ndl->unit_len; ++index) {{ \
                                         if (kiln_string_unit_at(s_recv, index) != kiln_string_unit_at(s_ndl, index)) {{ sw = 0; break; }} \
                                     }} \
                                 }} \
                                 v{v} = sw ? KILN_VAL_TRUE : KILN_VAL_FALSE; \
                                 handled = 1; \
                             }} \
                         }}",
                str_check = str_check,
                o = object.0,
                a0 = args[0].0,
                v = v
            ))
        }
        ("endsWith", 1) => {
            let str_check = if known_str {
                String::new()
            } else {
                format!("KILN_VAL_IS_STRING(v{o}) && ", o = object.0)
            };
            Some(format!(
                "if (!handled && {str_check}KILN_VAL_IS_STRING(v{a0})) {{ \
                             const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                             const kiln_string *s_ndl = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                             if (s_recv != NULL && s_ndl != NULL) {{ \
                                 int ew = s_recv->unit_len >= s_ndl->unit_len; \
                                 size_t start = ew ? s_recv->unit_len - s_ndl->unit_len : 0; \
                                 if (ew && !kiln_string_is_utf16(s_recv) && !kiln_string_is_utf16(s_ndl)) {{ \
                                     ew = memcmp(kiln_string_latin1(s_recv) + start, kiln_string_latin1(s_ndl), s_ndl->unit_len) == 0; \
                                 }} else if (ew) {{ \
                                     for (size_t index = 0; index < s_ndl->unit_len; ++index) {{ \
                                         if (kiln_string_unit_at(s_recv, start + index) != kiln_string_unit_at(s_ndl, index)) {{ ew = 0; break; }} \
                                     }} \
                                 }} \
                                 v{v} = ew ? KILN_VAL_TRUE : KILN_VAL_FALSE; \
                                 handled = 1; \
                             }} \
                         }}",
                str_check = str_check,
                o = object.0,
                a0 = args[0].0,
                v = v
            ))
        }
        ("charCodeAt", 1) => {
            let a0 = args[0].0;
            let a0_is_int = ctx.live_ints.contains(&args[0]);
            let a0_non_neg = ctx.ivs.get(&args[0]).map_or(false, |iv| iv.non_negative);
            let idx_bound = if a0_non_neg {
                format!("(uint64_t)i{a0} < s_recv->unit_len")
            } else {
                format!("i{a0} >= 0 && (uint64_t)i{a0} < s_recv->unit_len")
            };
            let (body, number_guard) = if a0_is_int {
                let code_read = if typed {
                    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
                    let box_stmt = if need_box {
                        format!(" v{v} = kiln_c99_val_from_double(d{v});")
                    } else {
                        String::new()
                    };
                    let int_stmt = if typed_int {
                        format!(" i{v} = (int64_t)code;")
                    } else {
                        String::new()
                    };
                    format!(
                        "uint16_t code = kiln_string_unit_at(s_recv, (size_t)i{a0}); d{v} = (double)code;{int_stmt}{box_stmt} kiln_twin = 1;"
                    )
                } else if typed_int {
                    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
                    let box_stmt = if need_box {
                        format!(" v{v} = kiln_c99_val_from_double((double)code);")
                    } else {
                        String::new()
                    };
                    format!(
                        "uint16_t code = kiln_string_unit_at(s_recv, (size_t)i{a0}); i{v} = (int64_t)code;{box_stmt} kiln_twin = 1;"
                    )
                } else {
                    format!(
                        "v{v} = kiln_c99_val_from_double((double)kiln_string_unit_at(s_recv, (size_t)i{a0}));"
                    )
                };
                let nan_ret = if typed {
                    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
                    let box_stmt = if need_box {
                        format!(" v{v} = KILN_QNAN_BASE;")
                    } else {
                        String::new()
                    };
                    let int_stmt = if typed_int {
                        format!(" i{v} = 0;")
                    } else {
                        String::new()
                    };
                    format!("d{v} = NAN;{int_stmt}{box_stmt} kiln_twin = 1;")
                } else if typed_int {
                    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
                    let box_stmt = if need_box {
                        format!(" v{v} = KILN_QNAN_BASE;")
                    } else {
                        String::new()
                    };
                    format!("i{v} = 0;{box_stmt} kiln_twin = 1;")
                } else {
                    format!("v{v} = KILN_QNAN_BASE;")
                };
                (
                    format!(
                        "if ({idx_bound}) {{ {code_read} handled = 1; }} else {{ {nan_ret} handled = 1; }}"
                    ),
                    String::new(),
                )
            } else {
                (
                    format!(
                        "double d0; memcpy(&d0, &v{a0}, sizeof(d0)); if (isnan(d0)) d0 = 0.0; d0 = trunc(d0); if (d0 >= 0.0 && (size_t)d0 < s_recv->unit_len) {{ v{v} = kiln_c99_val_from_double((double)kiln_string_unit_at(s_recv, (size_t)d0)); handled = 1; }} else {{ v{v} = KILN_QNAN_BASE; handled = 1; }}"
                    ),
                    format!(" && KILN_VAL_IS_NUMBER(v{a0})"),
                )
            };
            let str_check = if known_str {
                String::new()
            } else {
                format!(" && KILN_VAL_IS_STRING(v{o})", o = object.0)
            };
            Some(format!(
                "if (!handled{str_check}{number_guard}) {{ \
                             const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                             if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                                 {body} \
                             }} \
                         }}",
                str_check = str_check,
                o = object.0,
                number_guard = number_guard,
                body = body
            ))
        }
        ("charAt", 1) => {
            let a0 = args[0].0;
            let a0_is_int = ctx.live_ints.contains(&args[0]);
            let a0_non_neg = ctx.ivs.get(&args[0]).map_or(false, |iv| iv.non_negative);
            let idx_bound = if a0_non_neg {
                format!("(uint64_t)i{a0} < s_recv->unit_len")
            } else {
                format!("i{a0} >= 0 && (uint64_t)i{a0} < s_recv->unit_len")
            };
            let (body, number_guard) = if a0_is_int {
                (
                    format!(
                        "if ({idx_bound}) {{ v{v} = kiln_c99_val_from_unit(kiln_string_unit_at(s_recv, (size_t)i{a0})); handled = 1; }} else {{ v{v} = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string); handled = 1; }}"
                    ),
                    String::new(),
                )
            } else {
                (
                    format!(
                        "double d0; memcpy(&d0, &v{a0}, sizeof(d0)); if (isnan(d0)) d0 = 0.0; d0 = trunc(d0); if (d0 >= 0.0 && (size_t)d0 < s_recv->unit_len) {{ v{v} = kiln_c99_val_from_unit(kiln_string_unit_at(s_recv, (size_t)d0)); handled = 1; }} else {{ v{v} = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string); handled = 1; }}"
                    ),
                    format!(" && KILN_VAL_IS_NUMBER(v{a0})"),
                )
            };
            let str_check = if known_str {
                String::new()
            } else {
                format!(" && KILN_VAL_IS_STRING(v{o})", o = object.0)
            };
            Some(format!(
                "if (!handled{str_check}{number_guard}) {{ \
                             const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                             if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                                 {body} \
                             }} \
                         }}",
                str_check = str_check,
                o = object.0,
                number_guard = number_guard,
                body = body
            ))
        }
        ("trim", 0) => {
            let str_check = if known_str {
                String::new()
            } else {
                format!(" && KILN_VAL_IS_STRING(v{o})", o = object.0)
            };
            Some(format!(
                "if (!handled{str_check}) {{ \
                             const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                             if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                                 size_t start = 0; \
                                 while (start < s_recv->unit_len && ((unsigned char)kiln_string_latin1(s_recv)[start] <= ' ' || (unsigned char)kiln_string_latin1(s_recv)[start] == 0xa0)) start++; \
                                 size_t end = s_recv->unit_len; \
                                 while (end > start && ((unsigned char)kiln_string_latin1(s_recv)[end - 1] <= ' ' || (unsigned char)kiln_string_latin1(s_recv)[end - 1] == 0xa0)) end--; \
                                 v{v} = kiln_c99_val_slice_string(s_recv, start, end - start); \
                                 handled = 1; \
                             }} \
                         }}",
                str_check = str_check,
                o = object.0,
                v = v
            ))
        }
        ("slice", 2) => {
            let a0 = args[0].0;
            let a1 = args[1].0;
            let (guard, check_and_extract) = if ctx.live_ints.contains(&args[0])
                && ctx.live_ints.contains(&args[1])
            {
                (
                    String::new(),
                    format!(
                        "if (i{a0} >= 0 && i{a1} >= i{a0}) {{ \
                             size_t start = (size_t)i{a0}; \
                             size_t end = (size_t)i{a1} <= s_recv->unit_len ? (size_t)i{a1} : s_recv->unit_len; \
                             size_t slen = start < end ? end - start : 0; \
                             v{v} = kiln_c99_val_slice_string(s_recv, start, slen); \
                             handled = 1; \
                         }}",
                        a0 = a0,
                        a1 = a1,
                        v = v
                    ),
                )
            } else {
                (
                    format!(" && KILN_VAL_IS_NUMBER(v{a0}) && KILN_VAL_IS_NUMBER(v{a1})"),
                    format!(
                        "double d0, d1; memcpy(&d0, &v{a0}, sizeof(d0)); memcpy(&d1, &v{a1}, sizeof(d1)); \
                         if (d0 >= 0.0 && (double)(size_t)d0 == d0 && d1 >= d0 && (double)(size_t)d1 == d1) {{ \
                             size_t start = (size_t)d0; \
                             size_t end = (size_t)d1 <= s_recv->unit_len ? (size_t)d1 : s_recv->unit_len; \
                             size_t slen = start < end ? end - start : 0; \
                             v{v} = kiln_c99_val_slice_string(s_recv, start, slen); \
                             handled = 1; \
                         }}",
                        a0 = a0,
                        a1 = a1,
                        v = v
                    ),
                )
            };
            let str_check = if known_str {
                String::new()
            } else {
                format!(" && KILN_VAL_IS_STRING(v{o})", o = object.0)
            };
            Some(format!(
                "if (!handled{str_check}{guard}) {{ \
                             const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                             if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                                 {check_and_extract} \
                             }} \
                         }}",
                str_check = str_check,
                o = object.0,
                guard = guard,
                check_and_extract = check_and_extract
            ))
        }
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
        args.iter().fold(text, |text, a| {
            if ctx.live_numbers.contains(a) {
                text.replace(
                    &format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a}))) {{ double d; memcpy(&d, &v{a}, sizeof(d));",
                        a = a.0
                    ),
                    &format!("{{ double d = d{};", a.0),
                )
            } else if ctx.live_ints.contains(a) {
                text.replace(
                    &format!(
                        "if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a}))) {{ double d; memcpy(&d, &v{a}, sizeof(d));",
                        a = a.0
                    ),
                    &format!("{{ double d = (double)i{};", a.0),
                )
            } else {
                text
            }
        })
    });
    // A result proven a number is computed as its twin and boxed from it, and the
    // box, when nothing reads it, is not computed; the statement's tail then leaves
    // the twin alone where the intrinsic answered.
    let math_intrinsic = match math_intrinsic {
        Some(text) if typed => {
            let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
            let box_stmt = if need_box {
                format!(" v{v} = kiln_c99_val_from_double(d{v});")
            } else {
                String::new()
            };
            let int_stmt = if typed_int {
                format!(" i{v} = (int64_t)d{v};")
            } else {
                String::new()
            };
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
                    "d{v} = {expr};{int_stmt}{box_stmt} kiln_twin = 1;"
                ));
                rest = &rest[expr_start + end + 2..];
            }
            out.push_str(rest);
            Some(out)
        }
        Some(text) if typed_int => {
            let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
            let box_stmt = if need_box {
                format!(" v{v} = kiln_c99_val_from_double((double)i{v});")
            } else {
                String::new()
            };
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
                    "i{v} = (int64_t)({expr});{box_stmt} kiln_twin = 1;"
                ));
                rest = &rest[expr_start + end + 2..];
            }
            out.push_str(rest);
            Some(out)
        }
        other => other,
    };
    let math_s = math_intrinsic
        .as_ref()
        .map(|s| format!(" {s}"))
        .unwrap_or_default();
    let array_is_array_s = array_is_array_intrinsic
        .map(|s| format!(" {s}"))
        .unwrap_or_default();
    let number_predicate_s = number_predicate_intrinsic
        .map(|s| format!(" {s}"))
        .unwrap_or_default();
    let await_resolve_s = await_resolve_intrinsic
        .map(|s| format!(" {s}"))
        .unwrap_or_default();
    let push_s = push_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
    let regex_s = regex_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
    let map_s = map_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
    let parallel_pool_s = parallel_pool_intrinsic
        .map(|s| format!(" {s}"))
        .unwrap_or_default();
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
    // Issue 30: every prototype-method fast path above answers by name for the kind the receiver
    // turns out to be (`s.trim()`, `a.join(",")`, `m.get(k)`), so all of them rely on that name
    // being the table's on every kind prototype. (An iterator's `next` is not a member of any of
    // them, and stays outside.) One guard for the lot -- one compare while no
    // realm has written onto a built-in; after that, whether this realm's program wrote this name
    // onto one of them, remembered per name. A written member falls through to the cached and
    // named paths below, which find it.
    let kind_intrinsics = format!(
        "{push_s}{regex_s}{map_s}{filter_s}{reduce_s}{slice_s}{join_s}{split_s}{index_of_s}{substring_s}{collection_s}{sort_s}{array_extra_s}{string_extra_s}"
    );
    let kind_intrinsics = if kind_intrinsics.trim().is_empty() {
        String::new()
    } else if key_str
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
    {
        format!(" if (kiln_c99_kind_member_unmodified(\"{key_str}\")) {{{kind_intrinsics} }}")
    } else {
        String::new()
    };
    if let Some(callee) = prepared_callee {
        // Everything above is the by-name call's own lowering; here the callee is already in
        // hand, so only the intrinsic text is wanted and the prepared emitter places it.
        let loops = if kind_intrinsics.trim().is_empty() {
            None
        } else {
            Some(format!("{typed_remembered}{kind_intrinsics}"))
        };
        emit_call_prepared_method(
            output,
            vid,
            object,
            name,
            &callee,
            args,
            line,
            column,
            ctx,
            loops.as_deref(),
        );
        return;
    }
    let n = argc.max(1);

    let typed_sync = if typed && spec.exits && spec.calls.contains(&vid) {
        let invariant = lookup_speculation_invariant("method_call_number");
        format!(
            "/* Speculation exit [{}]: {} */ if (!kiln_twin) {{ if (KILN_UNLIKELY(!KILN_VAL_IS_NUMBER(v{v}))) {{ {} goto kiln_generic_resume_{v}; }} memcpy(&d{v}, &v{v}, sizeof(d{v})); }}",
            invariant.name,
            invariant.language_rule,
            super::materialize_all_sunk(ctx.graph, symbol, ctx.defs, ctx.array_slots)
        )
    } else if typed {
        format!("if (!kiln_twin) memcpy(&d{v}, &v{v}, sizeof(d{v}));")
    } else {
        String::new()
    };
    let typed_int_sync = if typed_int {
        if typed {
            format!(" i{v} = (int64_t)d{v};")
        } else {
            format!(
                " if (!kiln_twin) {{ double tmp_{v}; memcpy(&tmp_{v}, &v{v}, sizeof(tmp_{v})); i{v} = (int64_t)tmp_{v}; }}"
            )
        }
    } else {
        String::new()
    };
    let typed_sync = format!("{typed_sync}{typed_int_sync}");

    let in_typed = matches!(ctx.kind, crate::emit_graph::EntryKind::Typed { .. });
    if in_typed && math_intrinsic.is_some() {
        // A typed entry's `Math.*`. Its guard's members are what the entry's callers check before
        // entering it (`CallPlan::typed_guards`, read back from the mark by `typed_relies`), so
        // past that check the guard can fail only if the call itself -- this body or one it
        // called -- modified the member (issue 20). Re-entering boxed cannot undo that: effects
        // have happened.
        //
        // Modifying a built-in is a store through the runtime, so a body in which that can
        // happen reaches the runtime already and has `host` and `thrown`: it makes the general
        // call from here, which finds the override, and goes on with its answer -- an answer
        // that is not a number breaks what the entry was compiled for and refuses by name. A
        // body that reaches nothing cannot have modified anything since its caller checked, so
        // its failure edge stays a refusal that does not happen; giving it the runtime for that
        // would cost every such entry its inlining and every typed loop its fast form.
        let body_reaches_runtime = ctx.self_index.is_some_and(|index| {
            plan.typed_needs
                .get(index as usize)
                .is_some_and(|needs| needs.runtime)
        });
        if body_reaches_runtime {
            let not_number = format!(
                "if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"a built-in this typed entry relies on was modified during the call, and it answered a non-number\"; kiln_c99_note_refusal({diagnostic_source}, (size_t){pc}, \"{label}\", (size_t)kiln_c99_diagnostic_active_depth); goto kiln_refusal;"
            );
            writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; kiln_val call_args[{n}]; int call_args_ready = 0; (void)call_args_ready; int handled = 0; int kiln_twin = 0; (void)kiln_twin; \
                {TYPED_RELIES_MARK} {math_s} \
                if (KILN_UNLIKELY(!handled)) {{ \
                    {prep_args}int status = kiln_c99_val_call_method_key(host, v{o}, call_args, (size_t){argc}, {symbol}_key_{name}, depth, thrown, &site, {diagnostic_source}, (size_t){pc}, \"{label}\", &v{v}); \
                    {check} \
                    if (KILN_UNLIKELY(!KILN_VAL_IS_NUMBER(v{v}))) {{ {not_number} }} \
                }} {typed_sync} }}",
                o = object.0,
            )
            .unwrap();
        } else {
            // Nothing in this body can have modified the member since its caller checked, so the
            // guard is that caller's and is not asked again here: in a realm that has stored
            // onto some other built-in, asking would be a call per entry. A build with
            // `KILN_IC_CHECK` asks anyway and traps if a call site entered without checking.
            // What is left of the failure edge -- an argument that is not a number -- refuses.
            let unguarded = math_s.replacen(
                &math_guarded,
                &format!("KILN_C99_TYPED_RELIES_CHECK({math_guard}) {{ "),
                1,
            );
            writeln!(
                output,
                "    {{ int handled = 0; int kiln_twin = 0; (void)kiln_twin; \
                {TYPED_RELIES_MARK} {unguarded} \
                if (KILN_UNLIKELY(!handled)) {{ goto kiln_refusal; }} {typed_sync} }}"
            )
            .unwrap();
        }
        return;
    }

    writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; \
                kiln_val call_args[{n}]; int call_args_ready = 0; (void)call_args_ready; \
                int handled = 0; int kiln_twin = 0; (void)kiln_twin; {typed_remembered}\
                {await_resolve_s}{math_s}{array_is_array_s}{number_predicate_s}{parallel_pool_s}{json_s}{kind_intrinsics}{iterator_s} \
                if (!handled && KILN_VAL_IS_OBJECT(v{o})) {{ \
                    const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(v{o}); if (obj == NULL) __builtin_unreachable(); \
                    {typed_front}\
                    if (!handled && kiln_c99_site_count(&site) > 0) {{ \
                        kiln_val cached_fn = kiln_c99_site_cached_callee(&site, obj); \
                        if (cached_fn != 0 && KILN_VAL_IS_CLOSURE(cached_fn)) {{ \
                            kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(cached_fn); \
                            {typed_chain}if (!plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                                kiln_val vreceiver = plain->arrow ? plain->captured_this : v{o}; \
                                kiln_c99_normalize_closure_receiver(&vreceiver, plain); \
                                v{v} = KILN_VAL_UNDEFINED; \
                                int status; \
                                if (!plain->publishes_frame) {{ kiln_host call_host; kiln_call_context ctx_call = {{ kiln_c99_closure_host(host, plain, &call_host), thrown, NULL, depth + 1 }}; {prep_args}{by_name} {{ status = plain->body(&ctx_call, &v{v}, plain->upvalues, &vreceiver, call_args, (size_t){argc}); }} }} \
                                else {{ {prep_args}status = kiln_c99_invoke_closure(host, &v{v}, plain, vreceiver, call_args, (size_t){argc}, depth, thrown, NULL, 0); }} \
                                if (KILN_UNLIKELY(status == KILN_C99_CONTROL_ERROR)) {{ \
                                    if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(v{o}).tag; \
                                    if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = \"CallMethod invocation refused\"; \
                                    kiln_c99_note_refusal({diagnostic_source}, (size_t){pc}, \"{label}\", (size_t)kiln_c99_diagnostic_active_depth); \
                                }} else if (KILN_UNLIKELY(status == KILN_C99_THROWN)) {{ \
                                    if (kiln_c99_diagnostic_detail == NULL) {{ \
                                        kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(v{o}).tag; \
                                        kiln_c99_diagnostic_detail = \"CallMethod invocation threw\"; \
                                    }} \
                                    kiln_c99_note_refusal({diagnostic_source}, (size_t){pc}, \"{label}\", (size_t)kiln_c99_diagnostic_active_depth); \
                                }} \
                                {check} \
                                handled = 1; \
                            }} \
                        }} else if (cached_fn != 0 && KILN_VAL_IS_METHOD(cached_fn) && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                            {prep_args}int status = kiln_c99_val_invoke_cached_method(host, KILN_VAL_METHOD_ID(cached_fn), v{o}, call_args, (size_t){argc}, depth, thrown, &v{v}); \
                            {check} \
                            handled = 1; \
                        }} \
                    }} \
                }} \
                if (!handled && kiln_c99_cached_method_matches(&site, v{o}, {symbol}_key_{name}) && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                    {prep_args}int status = kiln_c99_val_invoke_cached_method(host, site.cached_method, v{o}, call_args, (size_t){argc}, depth, thrown, &v{v}); \
                    {check} \
                    handled = 1; \
                }} \
                if (!handled) {{ \
                    {prep_args}int status = kiln_c99_val_call_method_key(host, v{o}, call_args, (size_t){argc}, {symbol}_key_{name}, depth, thrown, &site, {diagnostic_source}, (size_t){pc}, \"{label}\", &v{v}); \
                    {check} \
                }} {typed_sync} }}",
                o = object.0,
            )
            .unwrap();
}

/// Emit a named call with its property value already resolved before argument evaluation.
/// Static candidates still enter their generated body directly, but the saved value is always
/// the authority: arguments may have replaced the property's current value by this point.
/// C statements declaring `int prepared_original`: whether the saved callee `method` (a `kiln_val`
/// read before the call's arguments) is the original `member` of the built-in `builtin` (a
/// `KILN_C99_BUILTIN_` suffix such as `ARRAY_PROTOTYPE`, `STRING_PROTOTYPE`, `MATH`). It holds
/// only while the program has written no member of that name onto the built-in
/// (`kiln_c99_builtin_member_unmodified`, so the read below can reach no accessor) and the
/// value the built-in answers for the name is the saved one -- the identity compare that
/// proves an intrinsic standing in for the member is standing in for what the program called.
/// `check` is the site's status check, run on `status` if the read itself fails. Every
/// prepared-aware intrinsic (arrays, strings, `Math`) uses this one test.
pub(crate) fn prepared_original_member(
    builtin: &str,
    member: &str,
    method: &str,
    check: &str,
) -> String {
    format!(
        "int prepared_original = 0; if (kiln_c99_builtin_member_unmodified(KILN_C99_BUILTIN_{builtin}, \"{member}\")) {{ kiln_value canonical = kiln_c99_undefined(); int canonical_status = kiln_c99_get(host, kiln_c99_builtin(KILN_C99_BUILTIN_{builtin}), \"{member}\", {len}, &canonical, depth, thrown); if (canonical_status != KILN_C99_OK) {{ status = canonical_status; {check} }} else {{ prepared_original = kiln_c99_strict_equal(canonical, kiln_val_to_boundary({method})); }} }} ",
        len = member.len(),
    )
}

fn emit_call_prepared_method(
    output: &mut String,
    vid: ValueId,
    object: &ValueId,
    name: &u32,
    callee: &ValueId,
    args: &[ValueId],
    line: u32,
    column: u32,
    ctx: &OpContext,
    array_intrinsics: Option<&str>,
) {
    let v = vid.0;
    let method = callee.0;
    let receiver = object.0;
    let argc = args.len();
    let program = ctx.program;
    let symbol = ctx.symbol;
    let diagnostic_site = ctx.diagnostic_site;
    let plan = ctx.plan;
    let key_str = program
        .constants
        .get(*name as usize)
        .and_then(|constant| match constant {
            Constant::String(key) => key.to_scalar_string().ok(),
            _ => None,
        })
        .unwrap_or_default();
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|origin| (origin.pc, origin.label.as_str()))
        .unwrap_or((0, "CallMethodPrepared"));
    let mut candidates = program
        .constants
        .get(*name as usize)
        .and_then(|constant| match constant {
            Constant::String(key) => {
                Some(method_candidates(program, key, &ctx.plan.publishes_frame))
            }
            _ => None,
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|&index| {
            ctx.plan
                .available
                .get(index as usize)
                .copied()
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable();

    let typed_candidates = candidates
        .iter()
        .copied()
        .filter(|&index| {
            let callee = &program.functions[index as usize];
            plan.typed_entries
                .get(index as usize)
                .copied()
                .unwrap_or(false)
                && !callee.arrow
                && callee.parameter_count as usize == argc
                && args.iter().all(|arg| ctx.live_numbers.contains(arg))
        })
        .collect::<Vec<_>>();
    let typed_check = status_check(diagnostic_site, pc, label, ctx.unwind);
    let mut typed_direct = String::new();
    for &index in &typed_candidates {
        let needs = plan.typed_needs[index as usize];
        let mut call_args = String::from("depth + 1");
        if needs.upvalues {
            call_args.push_str(", plain->upvalues");
        }
        if needs.receiver {
            call_args.push_str(", receiver_for_call");
        }
        if needs.row {
            call_args.push_str(&format!(", row_{index}"));
        }
        if needs.runtime {
            call_args.push_str(", host, thrown");
        }
        for arg in args {
            if ctx.live_numbers.contains(arg) {
                call_args.push_str(&format!(", d{}", arg.0));
            } else if ctx.ints.contains(arg) && ctx.live_ints.contains(arg) {
                call_args.push_str(&format!(", (double)i{}", arg.0));
            } else {
                call_args.push_str(&format!(", d{}", arg.0));
            }
        }

        let guard = typed_entry_guard(plan, index);
        let row_ready = if needs.row {
            format!(
                "const uint32_t *row_{index} = KILN_VAL_IS_OBJECT(receiver_for_call) ? {symbol}_fn_{index}_t_row((const kiln_object *)KILN_VAL_PTR(receiver_for_call)) : NULL; if (row_{index} != NULL && kiln_typed_ok{guard}) {{"
            )
        } else {
            format!("if (kiln_typed_ok{guard}) {{")
        };
        let typed_result = if plan.typed_returns[index as usize] {
            let boxed = ctx
                .boxed
                .map_or(true, |boxed| boxed.contains(&vid))
                .then(|| format!("v{v} = kiln_c99_val_from_double(result_{index});"))
                .unwrap_or_default();
            let twin = if ctx.live_numbers.contains(&vid) {
                format!("d{v} = result_{index};")
            } else {
                String::new()
            };
            let int_twin = if ctx.live_ints.contains(&vid) {
                format!("i{v} = (int64_t)result_{index};")
            } else {
                String::new()
            };
            format!(
                "double result_{index} = {symbol}_fn_{index}_t({call_args}); if (KILN_UNLIKELY(kiln_c99_typed_failed(result_{index}))) {{ status = kiln_c99_typed_status; if (status == KILN_C99_THROWN) *thrown = kiln_c99_typed_thrown; {typed_check} }} else {{ {boxed}{twin}{int_twin} kiln_twin = 1; handled = 1; }}"
            )
        } else {
            format!(
                "kiln_val result_{index} = {symbol}_fn_{index}_t({call_args}); if (KILN_UNLIKELY(result_{index} == KILN_VAL_TYPED_FAIL)) {{ status = kiln_c99_typed_status; if (status == KILN_C99_THROWN) *thrown = kiln_c99_typed_thrown; {typed_check} }} else {{ v{v} = result_{index}; handled = 1; }}"
            )
        };
        typed_direct.push_str(&format!(
            "if (!handled && plain->body == (kiln_body_fn){symbol}_fn_{index} && !plain->arrow && !plain->generator && !plain->publishes_frame) {{ {row_ready} {typed_result} }} }} "
        ));
    }

    let mut direct = String::new();
    if candidates.len() <= 32 {
        for index in candidates {
            // The selected body is proven by pointer identity, but typed entries also need
            // their argument and builtin guards. Use the ordinary generated entry here; it
            // still avoids the host boundary without treating a same-name function as proof
            // that the saved closure has typed arguments.
            if typed_candidates.contains(&index) {
                continue;
            }
            let entry = format!(
                "{symbol}_fn_{index}{}",
                if plan.fast.get(index as usize).copied().unwrap_or(false) {
                    "_f"
                } else {
                    ""
                }
            );
            let keyword = if direct.is_empty() { "if" } else { "else if" };
            direct.push_str(&format!(
                "{keyword} (plain->body == (kiln_body_fn){symbol}_fn_{index}) {{ status = {entry}(&ctx_call, &call_result, plain->upvalues, &receiver_for_call, call_args, (size_t){argc}); }} "
            ));
        }
    }
    if direct.is_empty() {
        direct = format!(
            "status = plain->body(&ctx_call, &call_result, plain->upvalues, &receiver_for_call, call_args, (size_t){argc});"
        );
    } else {
        direct.push_str(&format!(
            "else {{ status = plain->body(&ctx_call, &call_result, plain->upvalues, &receiver_for_call, call_args, (size_t){argc}); }}"
        ));
    }

    let mut arguments = format!("kiln_val call_args[{}] = {{", argc.max(1));
    for (position, arg) in args.iter().enumerate() {
        if position != 0 {
            arguments.push_str(", ");
        }
        if ctx.live_numbers.contains(arg) {
            arguments.push_str(&format!("kiln_c99_val_from_double(d{})", arg.0));
        } else if ctx.ints.contains(arg) && ctx.live_ints.contains(arg) {
            arguments.push_str(&format!("kiln_c99_val_from_double((double)i{})", arg.0));
        } else {
            arguments.push_str(&format!("v{}", arg.0));
        }
    }
    if argc == 0 {
        arguments.push_str(" KILN_VAL_UNDEFINED");
    }
    arguments.push_str("};");
    let args_ptr = if argc == 0 {
        "NULL".to_string()
    } else {
        "call_args".to_string()
    };
    let method_key = format!("{symbol}_key_{name}");
    let diagnostic_source = diagnostic_source(diagnostic_site);
    let check = status_check(diagnostic_site, pc, label, ctx.unwind);

    let mut prepared_intrinsics = String::new();
    if key_str == "resolve" && argc == 1 && ctx.await_resolve_arg == args.first().copied() {
        let argument = args[0].0;
        let guard =
            static_member_guard(&[("PROMISE", "resolve"), ("PROMISE_PROTOTYPE", "constructor")]);
        let store = if ctx.live_numbers.contains(&vid) && ctx.live_numbers.contains(&args[0]) {
            format!("v{v} = v{argument}; d{v} = d{argument}; kiln_twin = 1;")
        } else if ctx.live_numbers.contains(&vid) && ctx.live_ints.contains(&args[0]) {
            format!(
                "v{v} = kiln_c99_val_from_double((double)i{argument}); d{v} = (double)i{argument}; kiln_twin = 1;"
            )
        } else {
            format!("v{v} = v{argument};")
        };
        prepared_intrinsics.push_str(&format!(
            "if (KILN_LIKELY({guard} && KILN_VAL_IS_BUILTIN(recv) && KILN_VAL_BUILTIN_ID(recv) == KILN_C99_BUILTIN_PROMISE && KILN_VAL_IS_METHOD(method) && KILN_VAL_METHOD_ID(method) == KILN_C99_METHOD_PROMISE_RESOLVE && KILN_VAL_IS_NUMBER(v{argument}))) {{ {store} handled = 1; }} "
        ));
    }
    if key_str == "sqrt" && argc == 1 {
        let argument = args[0].0;
        let guard = static_member_guard(&[("MATH", "sqrt")]);
        let store = if ctx.live_numbers.contains(&vid) {
            let box_value = ctx
                .boxed
                .map_or(true, |boxed| boxed.contains(&vid))
                .then(|| format!(" v{v} = kiln_c99_val_from_double(d{v});"))
                .unwrap_or_default();
            format!("d{v} = sqrt(prepared_arg_{v});{box_value} kiln_twin = 1;")
        } else {
            format!("v{v} = kiln_c99_val_from_double(sqrt(prepared_arg_{v}));")
        };
        prepared_intrinsics.push_str(&format!(
            "{TYPED_RELIES_MARK} if (KILN_LIKELY({guard})) {{ if (KILN_VAL_IS_BUILTIN(recv) && KILN_VAL_BUILTIN_ID(recv) == KILN_C99_BUILTIN_MATH && KILN_VAL_IS_METHOD(method) && KILN_VAL_METHOD_ID(method) == KILN_C99_METHOD_MATH_SQRT && KILN_VAL_IS_NUMBER(v{argument})) {{ double prepared_arg_{v}; memcpy(&prepared_arg_{v}, &v{argument}, sizeof(prepared_arg_{v})); {store} handled = 1; }} }} "
        ));
    }

    let parallel_site =
        crate::parallel::ParallelMethod::from_name(&key_str).map(|parallel_method| {
            let (record, site) = parallel_site(
                plan,
                program,
                ctx.self_index,
                ctx.defs,
                parallel_method,
                args,
                vid,
            );
            writeln!(output, "    {}", record.marker()).unwrap();
            site
        });
    // The property read precedes argument evaluation for this graph operation.  The pool is
    // equivalent only while the saved callee is still the installed Array extension; if user
    // code changed it while computing an argument, the prepared call below invokes the saved
    // value.  The member guard ensures the read below sees only the bootstrap data property and
    // never an accessor installed by the program.
    let parallel_pool = parallel_site
        .as_ref()
        .and_then(Option::as_ref)
        .filter(|_| key_str.starts_with("parallel"))
        .map(|site| {
            let pool = parallel_lowering(plan, symbol, site, receiver, args, v);
            format!(
                "if (!handled && kiln_c99_kiln_extensions_on() && KILN_VAL_IS_CLOSURE(method) && (recv & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && kiln_c99_gc_kind_of_ptr(KILN_VAL_PTR(recv)) == KILN_GC_ARRAY && kiln_c99_builtin_pair_unmodified(KILN_C99_BUILTIN_ARRAY, KILN_C99_BUILTIN_ARRAY_PROTOTYPE) && kiln_c99_builtin_member_unmodified(KILN_C99_BUILTIN_ARRAY_PROTOTYPE, \"{key_str}\")) {{ kiln_value canonical = kiln_c99_undefined(); int canonical_status = kiln_c99_get(host, kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_PROTOTYPE), \"{key_str}\", {key_len}, &canonical, depth, thrown); if (canonical_status != KILN_C99_OK) {{ status = canonical_status; {check} }} else if (kiln_c99_strict_equal(canonical, kiln_val_to_boundary(method))) {{ {pool} }} }}",
                key_len = key_str.len(),
            )
        })
        .unwrap_or_default();

    // The array loops lowered for the by-name call, for a receiver that is an array whose saved
    // callee is `Array.prototype.<name>` itself. They leave `handled` clear whenever one of their
    // own guards fails, and the saved callee is invoked below as before.
    let array_loops = array_intrinsics
        .map(|loops| {
            let identity =
                prepared_original_member("ARRAY_PROTOTYPE", &key_str, "method", &check);
            format!(
                "if (!handled && (recv & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && kiln_c99_gc_kind_of_ptr(KILN_VAL_PTR(recv)) == KILN_GC_ARRAY) {{ {identity} if (prepared_original) {{ {loops} }} }} "
            )
        })
        .unwrap_or_default();

    let sync = if ctx.live_numbers.contains(&vid) {
        if ctx.spec.exits && ctx.spec.calls.contains(&vid) {
            let invariant = lookup_speculation_invariant("method_call_number");
            format!(
                " if (!kiln_twin) {{ /* Speculation exit [{}]: {} */ if (KILN_UNLIKELY(!KILN_VAL_IS_NUMBER(v{v}))) {{ {} goto kiln_generic_resume_{v}; }} memcpy(&d{v}, &v{v}, sizeof(d{v})); }}",
                invariant.name,
                invariant.language_rule,
                super::materialize_all_sunk(ctx.graph, symbol, ctx.defs, ctx.array_slots)
            )
        } else {
            format!(" if (!kiln_twin) memcpy(&d{v}, &v{v}, sizeof(d{v}));")
        }
    } else {
        String::new()
    };
    let sync_int = if ctx.live_ints.contains(&vid) {
        if ctx.live_numbers.contains(&vid) {
            format!(" i{v} = (int64_t)d{v};")
        } else {
            format!(
                " {{ double tmp_{v}; memcpy(&tmp_{v}, &v{v}, sizeof(tmp_{v})); i{v} = (int64_t)tmp_{v}; }}"
            )
        }
    } else {
        String::new()
    };
    writeln!(
        output,

        "    kiln_c99_js_frame_note_instruction({diagnostic_source}, (size_t){pc}, (size_t){line}, (size_t){column});\n    {{ {arguments} kiln_val method = v{method}; kiln_val recv = v{receiver}; int status; int kiln_twin = 0; int handled = 0; {prepared_intrinsics} {array_loops} {parallel_pool} if (!handled && KILN_VAL_IS_CLOSURE(method)) {{ kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(method); if (!plain->publishes_frame && !plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ kiln_val call_result = KILN_VAL_UNDEFINED; kiln_val receiver_for_call = plain->arrow ? plain->captured_this : recv; kiln_c99_normalize_closure_receiver(&receiver_for_call, plain); int kiln_typed_ok = !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH; (void)kiln_typed_ok; {typed_direct} if (!handled) {{ kiln_host call_host; kiln_call_context ctx_call = {{ kiln_c99_closure_host(host, plain, &call_host), thrown, NULL, depth + 1 }}; kiln_js_frame js_frame; kiln_c99_js_frame_set_method_name({method_key}); kiln_c99_js_frame_enter(&js_frame, plain); {direct} kiln_c99_js_frame_leave(&js_frame); kiln_c99_js_frame_set_method_name(NULL); {check} v{v} = call_result; }} }} else if (!handled) {{ kiln_value bargs[{argcap}]; for (size_t i = 0; i < (size_t){argc}; i++) bargs[i] = kiln_val_to_boundary(call_args[i]); kiln_value result = kiln_c99_undefined(); kiln_c99_js_frame_set_method_name({method_key}); status = kiln_c99_call(host, &result, kiln_val_to_boundary(method), kiln_val_to_boundary(recv), {boundary_args}, (size_t){argc}, depth, thrown); kiln_c99_js_frame_set_method_name(NULL); {check} v{v} = kiln_val_from_boundary(result); }} }} else if (!handled && KILN_VAL_IS_METHOD(method) && depth < KILN_C99_MAX_CALL_DEPTH) {{ kiln_c99_js_frame_set_method_name({method_key}); status = kiln_c99_val_invoke_cached_method(host, KILN_VAL_METHOD_ID(method), recv, {args_ptr}, (size_t){argc}, depth, thrown, &v{v}); kiln_c99_js_frame_set_method_name(NULL); {check} }} else if (!handled) {{ kiln_value bargs[{argcap}]; for (size_t i = 0; i < (size_t){argc}; i++) bargs[i] = kiln_val_to_boundary(call_args[i]); kiln_value result = kiln_c99_undefined(); kiln_c99_js_frame_set_method_name({method_key}); status = kiln_c99_call(host, &result, kiln_val_to_boundary(method), kiln_val_to_boundary(recv), {boundary_args}, (size_t){argc}, depth, thrown); kiln_c99_js_frame_set_method_name(NULL); {check} v{v} = kiln_val_from_boundary(result); }} {sync}{sync_int} }}",

        argcap = argc.max(1),
        boundary_args = if argc == 0 { "NULL" } else { "bargs" },
        typed_direct = typed_direct,

        prepared_intrinsics = prepared_intrinsics,

        parallel_pool = parallel_pool,

    )
    .unwrap();
}
