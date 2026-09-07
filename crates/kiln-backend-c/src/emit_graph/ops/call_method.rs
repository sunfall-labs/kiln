use super::OpContext;
use super::{method_candidates, status_check};
use crate::direct::TypedNeeds;
use crate::graph::ValueId;
use crate::prelude::lookup_speculation_invariant;
use kiln_ir::Constant;
use std::fmt::Write;

/// A callback argument of an intrinsic array loop (`map`, `filter`, `reduce`) whose proto this
/// compile *proves* -- `plan.known`, not a cache -- and whose declared arity the loop can call
/// typed. `arity` is the proto's own declared parameter count (1 or 2 for map/filter's element
/// and optional index; always 2 for reduce's accumulator and element), already checked against
/// the loop's own expectation by [`typed_callback`].
struct TypedCallback {
    target: u32,
    needs: TypedNeeds,
    returns_double: bool,
    arity: u32,
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
    })
}

/// The typed entry's own argument list, in its declared order: depth, then whatever
/// `TypedNeeds` says it reaches for beyond its parameters (upvalues, the receiver `vrecv`, a
/// `NULL` receiver-shape row -- this call site keeps none, exactly as the direct `Call` arm
/// does -- and the runtime), then one `double` per declared parameter, named by `params`.
fn typed_call_args(needs: TypedNeeds, params: &[&str]) -> String {
    let mut args = String::from("depth + 1");
    if needs.upvalues {
        args.push_str(", plain->upvalues");
    }
    if needs.receiver {
        args.push_str(", vrecv");
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
fn emit_map_typed(
    symbol: &str,
    o: u32,
    a0: u32,
    v: u32,
    check: &str,
    typed_failed: &str,
    c: &TypedCallback,
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
             built_arr->elements[idx] = kiln_c99_val_from_double(r_elem);"
        )
    } else {
        format!(
            "kiln_val r_elem = {symbol}_fn_{target}_t({call_args}); \
             if (KILN_UNLIKELY(r_elem == KILN_VAL_TYPED_FAIL)) {typed_failed} \
             if (built_arr->is_numeric && !KILN_VAL_IS_NUMBER(r_elem)) built_arr->is_numeric = 0; \
             built_arr->elements[idx] = r_elem;"
        )
    };
    format!(
        "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
             void *ptr = KILN_VAL_PTR(v{o}); \
             if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                 kiln_array *arr = (kiln_array *)ptr; \
                 kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                 if (!kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                     kiln_val vrecv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; \
                     if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                     if (arr->is_numeric && plain->body == (kiln_body_fn){symbol}_fn_{target}) {{ \
                         size_t count0 = arr->count; \
                         kiln_value built_val; \
                         int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                         if (status != KILN_C99_OK) return status; \
                         kiln_array *built_arr = kiln_c99_array_of(built_val); \
                         for (size_t idx = 0; idx < count0; ++idx) {{ \
                             double d_elem; memcpy(&d_elem, &arr->elements[idx], sizeof(d_elem)); \
                             {result} \
                         }} \
                         built_arr->count = count0; \
                         v{v} = kiln_val_from_boundary(built_val); \
                         handled = 1; \
                     }} else if (!plain->generator) {{ \
                         kiln_value built_val; \
                         int status = kiln_c99_new_array_with_capacity(&built_val, arr->count, arr->is_numeric); \
                         if (status != KILN_C99_OK) return status; \
                         kiln_array *built_arr = kiln_c99_array_of(built_val); \
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
             }} \
         }}"
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
) -> String {
    let target = c.target;
    let mut params = vec!["d_elem".to_string()];
    if c.arity == 2 {
        params.push("(double)idx".to_string());
    }
    let param_refs: Vec<&str> = params.iter().map(String::as_str).collect();
    let call_args = typed_call_args(c.needs, &param_refs);
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
    format!(
        "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
             void *ptr = KILN_VAL_PTR(v{o}); \
             if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                 kiln_array *arr = (kiln_array *)ptr; \
                 kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                 if (!kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                     kiln_val vrecv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; \
                     if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                     if (arr->is_numeric && plain->body == (kiln_body_fn){symbol}_fn_{target}) {{ \
                         size_t count0 = arr->count; \
                         kiln_value built_val; \
                         int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                         if (status != KILN_C99_OK) return status; \
                         kiln_array *built_arr = kiln_c99_array_of(built_val); \
                         for (size_t idx = 0; idx < count0; ++idx) {{ \
                             kiln_val elem = arr->elements[idx]; \
                             double d_elem; memcpy(&d_elem, &elem, sizeof(d_elem)); \
                             {call} \
                             if ({truthy}) {{ built_arr->elements[built_arr->count++] = elem; }} \
                         }} \
                         v{v} = kiln_val_from_boundary(built_val); \
                         handled = 1; \
                     }} else if (!plain->generator) {{ \
                         kiln_value built_val; \
                         int status = kiln_c99_new_array_with_capacity(&built_val, arr->count, arr->is_numeric); \
                         if (status != KILN_C99_OK) return status; \
                         kiln_array *built_arr = kiln_c99_array_of(built_val); \
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
             }} \
         }}"
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
) -> String {
    let (o, a0, a1, v) = site;
    let target = c.target;
    let call_args = typed_call_args(c.needs, &["dacc", "d_elem"]);
    format!(
        "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
             void *ptr = KILN_VAL_PTR(v{o}); \
             if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                 kiln_array *arr = (kiln_array *)ptr; \
                 kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                 if (!kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                     kiln_val vrecv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; \
                     if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                     if (arr->is_numeric && plain->body == (kiln_body_fn){symbol}_fn_{target}) {{ \
                         double dacc = d{a1}; \
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
             }} \
         }}"
    )
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
    ctx: &OpContext,
) {
    let v = vid.0;
    let program = ctx.program;
    let symbol = ctx.symbol;
    let diagnostic_site = ctx.diagnostic_site;
    let numbers = ctx.numbers;
    let unwind = ctx.unwind;
    let plan = ctx.plan;
    let spec = ctx.spec;
    let typed = numbers.contains(&vid);
    let (pc, label) = ctx
        .origins
        .get(&vid)
        .map(|o| (o.pc, o.label.as_str()))
        .unwrap_or((0, "Op"));
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
    // A builtin's static method is what the table says only while no own property has been
    // written onto any builtin: `Math.max = f` and `Promise.resolve = f` move the store epoch,
    // and every intrinsic above then takes the general call, which finds the override.
    let math_intrinsic = math_intrinsic
        .map(|text| format!("if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0)) {{ {text} }}"));

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

    // A callback whose proto this compile proves the identity of -- `plan.known`, not a cache --
    // and whose own declared arity the loop can call typed, is entered through its typed entry
    // directly from the loop, ahead of the generic (boxed) one. `DirectPlan::plan`'s
    // `callback_targets` references such a proto from the same three call shapes read here, so
    // its typed entry exists exactly when one of these finds it.
    let map_typed = (closed_world && key_str == "map" && argc == 1)
        .then(|| typed_callback(plan, program, args[0], &[1, 2]))
        .flatten();
    let filter_typed = (closed_world && key_str == "filter" && argc == 1)
        .then(|| typed_callback(plan, program, args[0], &[1, 2]))
        .flatten();
    // `reduce`'s accumulator has to stay a `double` for the whole loop, so the typed loop is
    // reached only when the typed entry is proven to return one and the initial value is
    // already proven a number -- both checked here, not inside the loop.
    let reduce_typed = (closed_world && key_str == "reduce" && argc == 2)
        .then(|| typed_callback(plan, program, args[0], &[2]))
        .flatten()
        .filter(|c| c.returns_double && numbers.contains(&args[1]));
    // On failure `kiln_c99_typed_status` holds the status and `kiln_c99_typed_thrown` the thrown
    // value when it is `KILN_C99_THROWN` -- the same failure handling the by-name typed
    // candidates above use, verbatim.
    let typed_failed = format!(
        "{{ int status = kiln_c99_typed_status; if (status == KILN_C99_THROWN) *thrown = kiln_c99_typed_thrown; {diag} {typed_check} }}"
    );

    let map_intrinsic = if closed_world && key_str == "map" && argc == 1 {
        Some(match &map_typed {
            Some(c) => emit_map_typed(symbol, object.0, args[0].0, v, &check, &typed_failed, c),
            None => format!(
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
                                 kiln_val vrecv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; \
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
            ),
        })
    } else {
        None
    };

    let parallel_map_intrinsic = if key_str == "parallelMap" && argc >= 1 {
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
                                 built_arr->count = arr->count; \
                                 kiln_val vrecv = plain->arrow ? plain->captured_this : v{o}; \
                                 if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                                 for (size_t idx = 0; idx < arr->count; ++idx) {{ \
                                     if (arr->elements[idx] == KILN_VAL_HOLE) {{ \
                                         built_arr->elements[idx] = KILN_VAL_HOLE; \
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
                                     built_arr->elements[idx] = prod; \
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
        Some(match &filter_typed {
            Some(c) => emit_filter_typed(symbol, object.0, args[0].0, v, &check, &typed_failed, c),
            None => format!(
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
                                 kiln_val vrecv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; \
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
            ),
        })
    } else {
        None
    };

    let reduce_intrinsic = if closed_world && key_str == "reduce" && argc == 2 {
        Some(match &reduce_typed {
            Some(c) => emit_reduce_typed(
                symbol,
                (object.0, args[0].0, args[1].0, v),
                &check,
                &typed_failed,
                c,
            ),
            None => format!(
                "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                             if (!plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                                 kiln_val vacc = v{a1}; \
                                 kiln_val vrecv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; \
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
            ),
        })
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
    let parallel_map_s = parallel_map_intrinsic
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
    let n = argc.max(1);

    writeln!(
                output,
                "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; \
                kiln_val call_args[{n}] = {{ {init} }}; \
                int handled = 0; int kiln_twin = 0; (void)kiln_twin; {typed_remembered}\
                {math_s}{push_s}{regex_s}{map_s}{parallel_map_s}{filter_s}{reduce_s}{slice_s}{join_s}{split_s}{index_of_s}{substring_s}{collection_s}{json_s}{sort_s}{array_extra_s}{string_extra_s}{iterator_s} \
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
