use super::OpContext;
use super::{method_candidates, status_check};
use crate::diagnostic_source;
use crate::direct::TypedNeeds;
use crate::graph::{GraphOp, ValueId};
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

/// The `parallelMap` decision at one call site: the proto whose parallel chunk the site may
/// run, or the first rule that kept it sequential, with the site's position -- one function,
/// so the lowering below and the record `--explain-parallel` reads are the same answer.
///
/// The order is the order the lowering tests: the unit's plan, the arguments, the callback's
/// identity, its typed entry, then the proto's own verdict (`parallel::parallel_verdict`,
/// narrowed by the clone check in `lib.rs`).
fn parallel_site(
    plan: &crate::direct::CallPlan,
    program: &kiln_ir::Program,
    enclosing: Option<u32>,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    args: &[ValueId],
) -> (crate::parallel::SiteRecord, Option<u32>) {
    use crate::parallel::{Reason, Refusal, SiteRecord};
    let target = args.first().and_then(|a| plan.known.get(a)).copied();
    // Where the site is, even where the plan knows no callee (a generic unit knows none): the
    // closure written at the call.
    let written = args.first().and_then(|a| match defs.get(a) {
        Some(GraphOp::MakeFunction { index, .. }) => Some(*index),
        _ => None,
    });
    let (line, column) = SiteRecord::locate(program, enclosing, target.or(written));
    let mut record = SiteRecord {
        line,
        column,
        refusal: None,
        captures: 0,
        at: None,
    };
    let refuse = |record: &mut SiteRecord, refusal: Refusal| {
        if let (Some(cb), Some(name)) = (target, refusal.detail.as_deref())
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
        refuse(&mut record, Refusal::new(Reason::DevProfile));
        return (record, None);
    }
    if args.len() > 1 {
        refuse(&mut record, Refusal::new(Reason::ExtraArguments));
        return (record, None);
    }
    let Some(target) = target else {
        refuse(&mut record, Refusal::new(Reason::CallbackUnknown));
        return (record, None);
    };
    let verdict = plan.parallel.get(target as usize).cloned();
    if let Some(Err(refusal)) = verdict {
        refuse(&mut record, refusal);
        return (record, None);
    }
    // The typed entry the sequential lowering and the clone's references hang off.
    if typed_callback(plan, program, args[0], &[1, 2]).is_none() {
        refuse(&mut record, Refusal::new(Reason::NotTyped));
        return (record, None);
    }
    match verdict {
        Some(Ok(shape))
            if plan
                .parallel_safe
                .get(target as usize)
                .copied()
                .unwrap_or(false) =>
        {
            record.captures = shape.captures.len();
            (record, Some(target))
        }
        _ => {
            refuse(&mut record, Refusal::new(Reason::CloneRejected));
            (record, None)
        }
    }
}

/// The pooled `parallelMap` lowering of one site whose callback is proto `target` (see
/// `parallel_site`): tried first, leaving `handled` clear -- for the sequential lowering or the
/// generic call after it -- whenever it does not answer. Shared by the specialized emission and
/// the generic-only one (issue 13).
fn parallel_pool_lowering(
    plan: &crate::direct::CallPlan,
    symbol: &str,
    target: u32,
    object: u32,
    callback: u32,
    v: u32,
) -> String {
    // Tier 1 (ADR 0069): the captured bindings the callback reads, each written once in
    // the unit, are read here on the realm thread -- after the closure is known to be
    // the target, so its upvalue table is the one the proof is about -- and must all be
    // numbers; otherwise `handled` stays clear and the sequential lowering answers.
    // Nothing can write them while the pool runs: the callback writes nothing and the
    // realm thread is blocked in the call.
    let captures = crate::emit_graph::parallel_captures(plan, target as usize);
    let (env_decl, env_ptr, env_failed) = if captures.is_empty() {
        (
            "int pmap_failed = 0; int pmap_caps_ok = 1;".to_owned(),
            "&pmap_failed",
            "pmap_failed",
        )
    } else {
        let mut decl = format!(
            "struct {{ int failed; double caps[{}]; }} pmap_env; pmap_env.failed = 0; int pmap_caps_ok = 1;",
            captures.len()
        );
        for (j, k) in captures.iter().enumerate() {
            decl.push_str(&format!(
                    " {{ kiln_val pmap_cap = *(const kiln_val *)KILN_VAL_PTR(plain->upvalues[{k}]); if (KILN_VAL_IS_NUMBER(pmap_cap)) memcpy(&pmap_env.caps[{j}], &pmap_cap, sizeof(double)); else pmap_caps_ok = 0; }}"
                ));
        }
        (decl, "&pmap_env", "pmap_env.failed")
    };
    format!(
        "if (!handled && kiln_c99_kiln_extensions_on() && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                 void *ptr = KILN_VAL_PTR(v{o}); \
                 if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                     kiln_array *arr = (kiln_array *)ptr; \
                     kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                     if (arr->is_numeric && kiln_c99_builtin_store_epoch == 0 && arr->properties.tag == KILN_VALUE_UNDEFINED && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH && plain->body == (kiln_body_fn){symbol}_fn_{target}) {{ \
                         {env_decl} \
                         if (pmap_caps_ok) {{ \
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
                 }} else if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY) {{ \
                     kiln_typed_array *view = (kiln_typed_array *)ptr; \
                     kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                     if (kiln_c99_builtin_store_epoch == 0 && !kiln_c99_inheriting_new_target && !view->buffer->detached && depth < KILN_C99_MAX_CALL_DEPTH && plain->body == (kiln_body_fn){symbol}_fn_{target}) {{ \
                         {env_decl} \
                         if (pmap_caps_ok) {{ \
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
                 }} \
             }}",
        o = object,
        a0 = callback,
        v = v,
    )
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
    known_arr: bool,
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
    let (elements, element, step) = if c.needs.runtime {
        (
            String::new(),
            "arr->elements[idx]".to_owned(),
            format!(
                "if (KILN_UNLIKELY(idx >= arr->count || !KILN_VAL_IS_NUMBER(arr->elements[idx]))) {{ \
                     if (idx >= arr->count || arr->elements[idx] == KILN_VAL_HOLE) {{ \
                         out_elems[idx] = KILN_VAL_HOLE; built_arr->is_numeric = 0; built_arr->count = idx + 1; continue; \
                     }} \
                     kiln_val vargs[3] = {{ arr->elements[idx], kiln_c99_val_from_double((double)idx), v{o} }}; \
                     kiln_val prod = KILN_VAL_UNDEFINED; \
                     int status; \
                     if (!plain->publishes_frame) {{ kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)3); }} \
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
    format!(
        "{header} \
             if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && arr->properties.tag == KILN_VALUE_UNDEFINED && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH)) {{ \
                 kiln_val vrecv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; \
                 if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                 if (arr->is_numeric && plain->body == (kiln_body_fn){symbol}_fn_{target}) {{ \
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
                     int status = kiln_c99_new_array_with_capacity(&built_val, count0, arr->is_numeric); \
                     if (status != KILN_C99_OK) return status; \
                     kiln_array *built_arr = kiln_c99_array_of(built_val); \
                     for (size_t idx = 0; idx < count0; ++idx) {{ \
                         if (idx >= arr->count || arr->elements[idx] == KILN_VAL_HOLE) {{ \
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
    format!(
        "{header} \
             if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && arr->properties.tag == KILN_VALUE_UNDEFINED && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH)) {{ \
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
    format!(
        "{header} \
             if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && arr->properties.tag == KILN_VALUE_UNDEFINED && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH)) {{ \
                 kiln_val vrecv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED; \
                 if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                 if (arr->is_numeric && plain->body == (kiln_body_fn){symbol}_fn_{target}) {{ \
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
         {footer}"
    )
}

fn is_proven_array(
    object: ValueId,
    defs: &std::collections::HashMap<ValueId, &GraphOp>,
    program: &kiln_ir::Program,
) -> bool {
    let mut curr = object;
    for _ in 0..8 {
        match defs.get(&curr) {
            Some(GraphOp::NewArray { .. }) => return true,
            Some(GraphOp::StoreLocal { value, .. })
            | Some(GraphOp::StoreCell { value, .. })
            | Some(GraphOp::CheckTdz { value, .. }) => curr = *value,
            Some(GraphOp::CallMethod { name, .. }) => {
                let is_arr_method = program
                    .constants
                    .get(*name as usize)
                    .and_then(|c| match c {
                        Constant::String(s) => s.to_scalar_string().ok(),
                        _ => None,
                    })
                    .is_some_and(|m| {
                        matches!(m.as_str(), "map" | "filter" | "slice" | "concat" | "split")
                    });
                return is_arr_method;
            }
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
            Some(GraphOp::CallMethod { name, .. }) => {
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
    ctx: &OpContext,
) {
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
    // site that enters the clone has already checked `kiln_c99_builtin_store_epoch == 0` on the
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
    let argc = args.len();
    let key_str = match program.constants.get(*name as usize) {
        Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
        _ => String::new(),
    };
    let diagnostic_source = diagnostic_source(diagnostic_site);
    // Every `parallelMap` site says, in the C, what was decided for it and why; see
    // `parallel::SiteRecord`.
    let parallel_site = (key_str == "parallelMap").then(|| {
        let (record, target) = parallel_site(plan, program, ctx.self_index, ctx.defs, args);
        writeln!(output, "    {}", record.marker()).unwrap();
        target
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
            .flatten()
            .map(|target| parallel_pool_lowering(plan, symbol, target, object.0, args[0].0, v));
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
                    let shape_var = format!("{pfx}_shape");
                    let row_var = format!("{pfx}_row");

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
                        call_args.push_str(", {ROW}");
                        (
                            String::new(),
                            format!(
                                " if (obj->shape != {shape_var}) {{ const uint32_t *__r = kiln_c99_recv_row({symbol}_fn_{i}_typed_recv_shape, &{symbol}_fn_{i}_typed_recv_slot[0][0], {columns}, obj->shape); if (__r != NULL) {{ {shape_var} = obj->shape; {row_var} = __r; }} }}"
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
                    let matched = format!(
                        "plain->body == (kiln_body_fn){symbol}_fn_{i} && kiln_typed_ok"
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
                            " kiln_c99_method_path_learn(&{pfx}_path, &site, obj, (kiln_body_fn){symbol}_fn_{i}, obj->shape == {shape_var} ? {row_var} : NULL, 1);"
                        )
                    } else {
                        format!(
                            " kiln_c99_method_path_learn(&{pfx}_path, &site, obj, (kiln_body_fn){symbol}_fn_{i}, NULL, 0);"
                        )
                    };
                    let arm = |learn_path: &str, matched: &str| -> String {
                        if needs.row {
                            // The receiver's shape is the one whose row this site remembered: the
                            // row goes in known, so the callee's own lookup folds away.
                            let known = invoke("__row", learn_path);
                            let learning = invoke("NULL", &format!("{row_learn}{learn_path}"));
                            format!(
                                "if ({matched}) {{ if (obj->shape != NULL && obj->shape == {shape_var}) {{ const uint32_t *__row = {row_var}; if (__row == NULL) __builtin_unreachable(); {known} }} else {{ {learning} }} handled = 1; }}"
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
                        "if (!handled && obj->shape == {pfx}_path.recv) {{ const kiln_object *kiln_path_level; if (kiln_c99_method_path_hit(&{pfx}_path, obj, &kiln_path_level) && kiln_typed_ok) {{ {front_closure}{front_call} handled = 1; }} }} "
                    );
                    (front, format!("{} else ", arm(&path_learn, &matched)))
                })
                .collect();
    // A path's receiver shape starts NULL, so the arms run only for a shaped receiver.
    let typed_front: String = if typed_arms.is_empty() {
        String::new()
    } else {
        format!(
            "if (obj->shape != NULL) {{ {} }} ",
            typed_arms.iter().map(|(front, _)| front.as_str()).collect::<String>()
        )
    };
    let typed_chain: String = typed_arms.iter().map(|(_, chain)| chain.as_str()).collect();
    // One dispatch guard for every typed arm of the site: the constructor flag and the depth
    // are the same for each, and `depth` is a local only where a typed arm can be entered.
    let typed_guard_decl = if typed_candidates.is_empty() {
        String::new()
    } else {
        "int kiln_typed_ok = !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH; (void)kiln_typed_ok; ".to_owned()
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
            "if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_PROMISE && KILN_VAL_IS_NUMBER(v{a0}))) {{ v{v} = v{a0}; handled = 1; }}",
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
            ("clz32", 1) => {
                let a0 = args[0].0;
                let load_u0 = if ctx.live_ints.contains(&args[0]) {
                    format!("uint32_t u0 = (uint32_t)i{a0};")
                } else if ctx.live_numbers.contains(&args[0]) {
                    format!("uint32_t u0 = (uint32_t)(int64_t)d{a0};")
                } else {
                    format!(
                        "uint32_t u0; {{ double d; if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a0}))) memcpy(&d, &v{a0}, sizeof(d)); else d = kiln_c99_as_number(kiln_val_to_boundary(v{a0})); u0 = (uint32_t)(int64_t)d; }}"
                    )
                };
                Some(format!(
                    "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                         {load_u0} \
                         int32_t res = u0 ? (int32_t)__builtin_clz(u0) : 32; \
                         v{v} = kiln_c99_val_from_double((double)res); \
                         handled = 1; \
                     }}",
                    o = object.0,
                    load_u0 = load_u0,
                    v = v
                ))
            }
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
            ("imul", 2) => {
                let load_imul = |a: ValueId, name: &str| -> String {
                    if ctx.live_ints.contains(&a) {
                        format!("int32_t {name} = (int32_t)i{};", a.0)
                    } else if ctx.live_numbers.contains(&a) {
                        format!("int32_t {name} = (int32_t)(uint32_t)(int64_t)d{};", a.0)
                    } else {
                        format!(
                            "int32_t {name}; {{ double d; if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v{a}))) memcpy(&d, &v{a}, sizeof(d)); else d = kiln_c99_as_number(kiln_val_to_boundary(v{a})); {name} = (int32_t)(uint32_t)(int64_t)d; }}",
                            a = a.0
                        )
                    }
                };
                let load_i0 = load_imul(args[0], "i0");
                let load_i1 = load_imul(args[1], "i1");
                Some(format!(
                    "if (KILN_LIKELY(KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_MATH)) {{ \
                         {load_i0} {load_i1} \
                         int32_t res = (int32_t)((uint32_t)i0 * (uint32_t)i1); \
                         v{v} = kiln_c99_val_from_double((double)res); \
                         handled = 1; \
                     }}",
                    o = object.0,
                    load_i0 = load_i0,
                    load_i1 = load_i1,
                    v = v
                ))
            }
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
    // A builtin's static method is what the table says only while no own property has been
    // written onto any builtin: `Math.max = f` and `Promise.resolve = f` move the store epoch,
    // and every intrinsic above then takes the general call, which finds the override.
    let math_intrinsic = math_intrinsic
        .map(|text| format!("if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0)) {{ {text} }}"));

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
            "if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_ARRAY)) {{ \
                 {arg_check} \
             }}",
            o = object.0,
            arg_check = arg_check
        ))
    } else {
        None
    };

    // 1c. Number.* static predicates (isFinite, isNaN, isInteger, isSafeInteger)
    let number_predicate_intrinsic = match (key_str.as_str(), argc) {
        ("isFinite" | "isNaN" | "isInteger" | "isSafeInteger", 0) => Some(format!(
            "if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_NUMBER)) {{ \
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
                "if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_NUMBER)) {{ \
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
                "if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_NUMBER)) {{ \
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
                "if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_NUMBER)) {{ \
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
                "if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && KILN_VAL_IS_BUILTIN(v{o}) && KILN_VAL_BUILTIN_ID(v{o}) == KILN_C99_BUILTIN_NUMBER)) {{ \
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
                         arr->elements[arr->count++] = {push_arg}; \
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
                             arr->elements[arr->count++] = {push_arg}; \
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

    // A callback whose proto this compile proves the identity of -- `plan.known`, not a cache --
    // and whose own declared arity the loop can call typed, is entered through its typed entry
    // directly from the loop, ahead of the generic (boxed) one. `DirectPlan::plan`'s
    // `callback_targets` references such a proto from the same three call shapes read here, so
    // its typed entry exists exactly when one of these finds it.
    let map_typed = (key_str == "map" && argc == 1)
        .then(|| typed_callback(plan, program, args[0], &[1, 2]))
        .flatten();
    let filter_typed = (key_str == "filter" && argc == 1)
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

    let map_intrinsic = if key_str == "map" && argc == 1 {
        Some(match &map_typed {
            Some(c) => emit_map_typed(
                symbol,
                object.0,
                args[0].0,
                v,
                &check,
                &typed_failed,
                c,
                known_arr,
            ),
            None => {
                let (header, footer) = if known_arr {
                    (
                        format!(
                            "if (!handled && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                                 kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                                 kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                            a0 = args[0].0,
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
                            a0 = args[0].0,
                            o = object.0
                        ),
                        "} }",
                    )
                };
                format!(
                    "{header} \
                         if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && arr->properties.tag == KILN_VALUE_UNDEFINED && !plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH)) {{ \
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
                     {footer}",
                    header = header,
                    o = object.0,
                    check = check,
                    v = v,
                    footer = footer
                )
            }
        })
    } else {
        None
    };

    // The pooled `parallelMap` (ADR 0058, application 2): the callback is this site's known proto,
    // its body passes `parallel::parallel_safe`, its typed entry returns a double, and its
    // parallel clone (`_p`) was emitted and scanned clean (`lib.rs`), so it can run on a provider
    // pool thread that owns no realm. `kiln_c99_builtin_store_epoch == 0` is checked here, on the
    // realm thread, because the clone calls `Math.<name>` as bare libm.
    // Tried before the sequential lowering below and only when the receiver is a numeric array
    // whose closure really is that proto; a typed entry that bails out, or a provider that could
    // not run the pool, leaves `handled` clear and the sequential lowering answers -- the same
    // answer, recomputed. Nothing is refused here.
    // Both lowerings of `parallelMap` stand down when the realm withholds kiln's extensions
    // (ADR 0068): the call then goes through the ordinary lookup, which misses, and throws the
    // TypeError Node throws for a method that is not there.
    let parallel_pool_intrinsic = parallel_site
        .flatten()
        .map(|target| parallel_pool_lowering(plan, symbol, target, object.0, args[0].0, v));

    // `parallelMap`'s `this` is `map`'s: the thisArg, else undefined -- which a sloppy callback
    // sees as the global object. A sloppy callback given a primitive thisArg would see it boxed;
    // that case is left to the generic call below rather than lowered here.
    let (this_arg, this_ok) = if argc >= 2 {
        let t = args[1].0;
        (
            format!("v{t}"),
            format!("plain->arrow || plain->strict || KILN_VAL_IS_NULLISH(v{t})"),
        )
    } else {
        ("KILN_VAL_UNDEFINED".to_owned(), "1".to_owned())
    };
    let parallel_map_intrinsic = if key_str == "parallelMap" && argc >= 1 {
        Some(format!(
            "if (!handled && kiln_c99_kiln_extensions_on() && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                             if (!plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH && ({this_ok})) {{ \
                                 /* `map`, element for element: the length read once, each element \
                                  * read when it is visited (a callback may grow, shrink or \
                                  * reallocate the array), the result's length kept current so the \
                                  * collector sees every value stored so far, and `this` the \
                                  * thisArg, or undefined. */ \
                                 size_t count0 = arr->count; \
                                 kiln_value built_val; \
                                 int status = kiln_c99_new_array_with_capacity(&built_val, count0, arr->is_numeric); \
                                 if (status != KILN_C99_OK) return status; \
                                 kiln_array *built_arr = kiln_c99_array_of(built_val); \
                                 kiln_val vrecv = plain->arrow ? plain->captured_this : {this_arg}; \
                                 if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                                 for (size_t idx = 0; idx < count0; ++idx) {{ \
                                     if (idx >= arr->count || arr->elements[idx] == KILN_VAL_HOLE) {{ \
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
                         }} else if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY) {{ \
                             kiln_typed_array *view = (kiln_typed_array *)ptr; \
                             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                             if (!plain->generator && !kiln_inheriting && !view->buffer->detached && depth < KILN_C99_MAX_CALL_DEPTH && ({this_ok})) {{ \
                                 kiln_value built_val; \
                                 int status = kiln_c99_typed_alloc(&built_val, view->kind, view->length); \
                                 if (status != KILN_C99_OK) return status; \
                                 kiln_typed_array *built_view = kiln_c99_typed_array_of(built_val); \
                                 kiln_val vrecv = plain->arrow ? plain->captured_this : {this_arg}; \
                                 if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vrecv); \
                                 for (size_t idx = 0; idx < view->length; ++idx) {{ \
                                     kiln_val vargs[3] = {{ kiln_c99_val_from_double(kiln_c99_typed_get(view, idx)), kiln_c99_val_from_double((double)idx), v{o} }}; \
                                     kiln_val prod = KILN_VAL_UNDEFINED; \
                                     int status; \
                                     if (!plain->publishes_frame) {{ kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; status = plain->body(&ctx_call, &prod, plain->upvalues, &vrecv, vargs, (size_t)3); }} \
                                     else status = kiln_c99_invoke_closure(host, &prod, plain, vrecv, vargs, (size_t)3, depth, thrown, NULL, 0); \
                                     if (KILN_UNLIKELY(status != KILN_C99_OK)) {{ \
                                         {check} \
                                     }} \
                                     double num_val = 0.0; \
                                     if (KILN_VAL_IS_NUMBER(prod)) {{ memcpy(&num_val, &prod, sizeof(num_val)); }} \
                                     else {{ \
                                         status = kiln_c99_to_number_checked(host, kiln_val_to_boundary(prod), &num_val, depth, thrown); \
                                         if (status != KILN_C99_OK) return status; \
                                     }} \
                                     kiln_c99_typed_set(built_view, idx, num_val); \
                                 }} \
                                 v{v} = kiln_val_from_boundary(built_val); \
                                 handled = 1; \
                             }} \
                         }} \
                     }}",
            o = object.0,
            a0 = args[0].0,
            v = v,
            check = check,
            this_arg = this_arg,
            this_ok = this_ok,
        ))
    } else {
        None
    };

    let filter_intrinsic = if key_str == "filter" && argc == 1 {
        Some(match &filter_typed {
            Some(c) => emit_filter_typed(
                symbol,
                object.0,
                args[0].0,
                v,
                &check,
                &typed_failed,
                c,
                known_arr,
            ),
            None => {
                let (header, footer) = if known_arr {
                    (
                        format!(
                            "if (!handled && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                                 kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                                 kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                            a0 = args[0].0,
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
                            a0 = args[0].0,
                            o = object.0
                        ),
                        "} }",
                    )
                };
                format!(
                    "{header} \
                         if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && arr->properties.tag == KILN_VALUE_UNDEFINED && !plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH)) {{ \
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
                     {footer}",
                    header = header,
                    o = object.0,
                    check = check,
                    v = v,
                    footer = footer
                )
            }
        })
    } else {
        None
    };

    let reduce_intrinsic = if key_str == "reduce" && argc == 2 {
        Some(match &reduce_typed {
            Some(c) => {
                let init_dacc = if ctx.live_numbers.contains(&args[1]) {
                    format!("d{}", args[1].0)
                } else if ctx.live_ints.contains(&args[1]) {
                    format!("(double)i{}", args[1].0)
                } else {
                    format!("d{}", args[1].0)
                };
                emit_reduce_typed(
                    symbol,
                    (object.0, args[0].0, args[1].0, v),
                    &check,
                    &typed_failed,
                    c,
                    known_arr,
                    &init_dacc,
                )
            }
            None => {
                let (header, footer) = if known_arr {
                    (
                        format!(
                            "if (!handled && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                                 kiln_array *arr = (kiln_array *)KILN_VAL_PTR(v{o}); \
                                 kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0});",
                            a0 = args[0].0,
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
                            a0 = args[0].0,
                            o = object.0
                        ),
                        "} }",
                    )
                };
                format!(
                    "{header} \
                         if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && arr->properties.tag == KILN_VALUE_UNDEFINED && !plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH)) {{ \
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
                     {footer}",
                    header = header,
                    o = object.0,
                    a1 = args[1].0,
                    check = check,
                    v = v,
                    footer = footer
                )
            }
        })
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
                     if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && arr->properties.tag == KILN_VALUE_UNDEFINED)) {{ \
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
                     if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0)) {{ \
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
                         if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && arr->properties.tag == KILN_VALUE_UNDEFINED)) {{ \
                             {array_body} \
                         }} \
                     }} \
                 }} else if (!handled && KILN_VAL_IS_STRING(v{o}){guard}) {{ \
                     if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0)) {{ \
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
                 if (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0 && arr->properties.tag == KILN_VALUE_UNDEFINED)) {{ \
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
            "if (!handled && KILN_LIKELY(kiln_c99_builtin_store_epoch == 0) && {str_check}KILN_VAL_IS_STRING(v{a0})) {{ \
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
            "if (!handled && KILN_LIKELY(kiln_c99_builtin_store_epoch == 0) && {str_check}KILN_VAL_IS_STRING(v{a0})) {{ \
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
            "if (!handled && KILN_LIKELY(kiln_c99_builtin_store_epoch == 0){str_check}{guard}) {{ \
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

    let json_intrinsic = match (key_str.as_str(), argc) {
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
        if known_arr {
            Some(format!(
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
            ))
        } else {
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
        }
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
                                 int sw = (!kiln_string_is_utf16(s_recv) && !kiln_string_is_utf16(s_ndl) && s_recv->unit_len >= s_ndl->unit_len && memcmp(kiln_string_latin1(s_recv), kiln_string_latin1(s_ndl), s_ndl->unit_len) == 0); \
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
                                 int ew = (!kiln_string_is_utf16(s_recv) && !kiln_string_is_utf16(s_ndl) && s_recv->unit_len >= s_ndl->unit_len && memcmp(kiln_string_latin1(s_recv) + (s_recv->unit_len - s_ndl->unit_len), kiln_string_latin1(s_ndl), s_ndl->unit_len) == 0); \
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
    let parallel_map_s = parallel_map_intrinsic
        .map(|s| format!(" {s}"))
        .unwrap_or_default();
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
        writeln!(
            output,
            "    {{ int handled = 0; int kiln_twin = 0; (void)kiln_twin; \
            {math_s} \
            if (KILN_UNLIKELY(!handled)) {{ goto kiln_refusal; }} {typed_sync} }}"
        )
        .unwrap();
        return;
    }

    writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; \
                kiln_val call_args[{n}]; int call_args_ready = 0; (void)call_args_ready; \
                int handled = 0; int kiln_twin = 0; (void)kiln_twin; {typed_remembered}\
                {await_resolve_s}{math_s}{array_is_array_s}{number_predicate_s}{push_s}{regex_s}{map_s}{parallel_pool_s}{parallel_map_s}{filter_s}{reduce_s}{slice_s}{join_s}{split_s}{index_of_s}{substring_s}{collection_s}{json_s}{sort_s}{array_extra_s}{string_extra_s}{iterator_s} \
                if (!handled && KILN_VAL_IS_OBJECT(v{o})) {{ \
                    const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(v{o}); if (obj == NULL) __builtin_unreachable(); \
                    {typed_front}\
                    if (!handled && site.pic_count > 0) {{ \
                        kiln_val cached_fn = kiln_c99_site_cached_callee(&site, obj); \
                        if (cached_fn != 0 && KILN_VAL_IS_CLOSURE(cached_fn)) {{ \
                            kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(cached_fn); \
                            {typed_chain}if (!plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                                kiln_val vreceiver = plain->arrow ? plain->captured_this : v{o}; \
                                if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&vreceiver); \
                                v{v} = KILN_VAL_UNDEFINED; \
                                int status; \
                                if (!plain->publishes_frame) {{ kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; {prep_args}{by_name} {{ status = plain->body(&ctx_call, &v{v}, plain->upvalues, &vreceiver, call_args, (size_t){argc}); }} }} \
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
                if (!handled && kiln_c99_cached_method_matches(&site, v{o}) && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
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
