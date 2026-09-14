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
                 if (!kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
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
                 if (!kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
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
                 if (!kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
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
        let a = format!("d{}", args[0].0);
        let b = args.get(1).map(|x| format!("d{}", x.0)).unwrap_or_default();
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
            _ => unreachable!("PARALLEL_MATH names only these"),
        };
        writeln!(
            output,
            "    d{v} = {call}; v{v} = kiln_c99_val_from_double(d{v});"
        )
        .unwrap();
        return;
    }
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
        Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
        _ => String::new(),
    };
    let diagnostic_source = diagnostic_source(diagnostic_site);
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
        let sync_number = typed
            .then(|| format!(" memcpy(&d{v}, &v{v}, sizeof(d{v}));"))
            .unwrap_or_default();
        writeln!(
            output,
            "    {{ static kiln_c99_site site = {{KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}}; kiln_val call_args[{n}] = {{ {init} }}; int status = kiln_c99_val_call_method_key(host, v{o}, call_args, (size_t){argc}, {symbol}_key_{name}, depth, thrown, &site, {diagnostic_source}, (size_t){pc}, \"{label}\", &v{v}); {check}{sync_number} }}",
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
    let typed_chain: String = typed_candidates
                .iter()
                .map(|&i| {
                    let pfx = format!("{symbol}_fn_{fn_id}_v{v}_t_{i}");
                    let shape_var = format!("{pfx}_shape");
                    let row_var = format!("{pfx}_row");
                    let proto_var = format!("{pfx}_proto");
                    let epoch_var = format!("{pfx}_epoch");
                    let upvalues_var = format!("{pfx}_upvalues");
                    let fn_var = format!("{pfx}_fn");

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
                        call_args.push_str(&format!(", obj->shape == {shape_var} ? {row_var} : NULL"));
                        (
                            String::new(),
                            format!(
                                " if (obj->shape != {shape_var}) {{ const uint32_t *__r = kiln_c99_recv_row({symbol}_fn_{i}_typed_recv_shape, &{symbol}_fn_{i}_typed_recv_slot[0][0], {columns}, obj->shape); if (__r != NULL) {{ {shape_var} = obj->shape; {row_var} = __r; }} }} {proto_var} = obj->prototype.tag == KILN_VALUE_OBJECT ? obj->prototype.bits : 0; {epoch_var} = kiln_c99_site_callee_is_inherited(&site, obj) ? kiln_c99_proto_epoch : UINT64_MAX; {upvalues_var} = plain->upvalues;"
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
                        "(cached_fn == {fn_var} || (plain->body == (kiln_body_fn){symbol}_fn_{i} && ({fn_var} = cached_fn) != 0)) && kiln_typed_ok"
                    );
                    let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
                    let box_stmt = if need_box { format!(" v{v} = kiln_c99_val_from_double(kiln_typed_d);") } else { String::new() };
                    if plan.typed_returns[i as usize] {
                        let twin = if typed { format!(" d{v} = kiln_typed_d; kiln_twin = 1;") } else { String::new() };
                        format!(
                            "if ({matched}) {{ double kiln_typed_d = {symbol}_fn_{i}_t({call_args}); if (KILN_UNLIKELY(kiln_c99_typed_failed(kiln_typed_d))) {failed} else {{{box_stmt}{twin}{row_learn} }} handled = 1; }} else "
                        )
                    } else {
                        format!(
                            "if ({matched}) {{ kiln_val kiln_typed_r = {symbol}_fn_{i}_t({call_args}); if (KILN_UNLIKELY(kiln_typed_r == KILN_VAL_TYPED_FAIL)) {failed} else {{ v{v} = kiln_typed_r;{row_learn} }} handled = 1; }} else "
                        )
                    }
                })
                .collect();
    // One dispatch guard for every typed arm of the site: the constructor flag and the depth
    // are the same for each, and `depth` is a local only where a typed arm can be entered.
    let typed_guard_decl = if typed_candidates.is_empty() {
        String::new()
    } else {
        "int kiln_typed_ok = !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH; (void)kiln_typed_ok; ".to_owned()
    };
    let typed_remembered = typed_guard_decl;
    // The shape-first entry. Once the generic arm below has entered a typed candidate for a
    // receiver shape -- through the site's validated cache -- the shape, the receiver's own
    // prototype link and the prototype epoch of that moment name the callee outright: the same
    // three facts the cache's own hit checks, without the entry walk, the closure checks or the
    // callee compare. A mutation anywhere a cache reads through moves the epoch and sends the
    // site back through the generic arm, which restamps.
    let typed_fast: String = typed_candidates
        .iter()
        .copied()
        .filter(|&i| plan.typed_needs[i as usize].row)
        .enumerate()
        .map(|(k, i)| {
            let pfx = format!("{symbol}_fn_{fn_id}_v{v}_t_{i}");
            let shape_var = format!("{pfx}_shape");
            let row_var = format!("{pfx}_row");
            let upvalues_var = format!("{pfx}_upvalues");

            let needs = plan.typed_needs[i as usize];
            let mut call_args = String::from("depth + 1");
            if needs.upvalues {
                call_args.push_str(&format!(", {upvalues_var}"));
            }
            if needs.receiver {
                call_args.push_str(&format!(", v{}", object.0));
            }
            call_args.push_str(", __row");
            if needs.runtime {
                call_args.push_str(", host, thrown");
            }
            for a in args {
                call_args.push_str(&format!(", d{}", a.0));
            }
            let failed = format!(
                "{{ int status = kiln_c99_typed_status; if (status == KILN_C99_THROWN) *thrown = kiln_c99_typed_thrown; {diag} {typed_check} }}"
            );
            let guard = if k == 0 {
                format!("if (obj->shape == {shape_var})")
            } else {
                format!("else if (obj->shape == {shape_var})")
            };
            let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
            let box_stmt = if need_box { format!(" v{v} = kiln_c99_val_from_double(kiln_typed_d);") } else { String::new() };
            if plan.typed_returns[i as usize] {
                let twin = if typed { format!(" d{v} = kiln_typed_d; kiln_twin = 1;") } else { String::new() };
                format!(
                    "{guard} {{ const uint32_t *__row = {row_var}; if (__row == NULL) __builtin_unreachable(); double kiln_typed_d = {symbol}_fn_{i}_t({call_args}); if (KILN_UNLIKELY(kiln_c99_typed_failed(kiln_typed_d))) {failed} else {{{box_stmt}{twin} }} handled = 1; }} "
                )
            } else {
                format!(
                    "{guard} {{ const uint32_t *__row = {row_var}; if (__row == NULL) __builtin_unreachable(); kiln_val kiln_typed_r = {symbol}_fn_{i}_t({call_args}); if (KILN_UNLIKELY(kiln_typed_r == KILN_VAL_TYPED_FAIL)) {failed} else {{ v{v} = kiln_typed_r; }} handled = 1; }} "
                )
            }
        })
        .collect();
    let typed_fast_block = if typed_fast.is_empty() {
        String::new()
    } else {
        format!("{typed_fast} ")
    };
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
                if numbers.contains(a) {
                    format!("call_args[{i}] = kiln_c99_val_from_double(d{});", a.0)
                } else {
                    format!("call_args[{i}] = v{};", a.0)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let prep_args = format!("if (!call_args_ready) {{ {init_stmts} call_args_ready = 1; }} ");
    let check = status_check(diagnostic_site, pc, label, unwind);
    let closed_world = !program.has_direct_eval() && !program.has_dynamic_import();

    // `await Promise.resolve(number)` still has to suspend once, but the intermediate fulfilled
    // promise is unobservable when this call result is consumed only by that await. The graph
    // supplies this marker only for an exact builtin receiver and immediate await; the runtime
    // number check and epoch guard cover value type and writes to Promise/prototype stores. Any failed guard falls
    // through to the generic method call; this marked site deliberately does not use the older
    // Promise.resolve intrinsic because its result is immediately consumed by the await path.
    let await_resolve_intrinsic = if ctx.await_resolve_arg.is_some()
        && closed_world
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

    // 1. Math.* intrinsic if closed_world
    let math_intrinsic = if closed_world && await_resolve_intrinsic.is_none() {
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
                             if (!plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
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

    // The pooled `parallelMap` (ADR 0058, application 2): the callback is this site's known proto,
    // its body passes `parallel::parallel_safe`, its typed entry returns a double, and its
    // parallel clone (`_p`) was emitted and scanned clean (`lib.rs`), so it can run on a provider
    // pool thread that owns no realm. `kiln_c99_builtin_store_epoch == 0` is checked here, on the
    // realm thread, because the clone calls `Math.<name>` as bare libm.
    // Tried before the sequential lowering below and only when the receiver is a numeric array
    // whose closure really is that proto; a typed entry that bails out, or a provider that could
    // not run the pool, leaves `handled` clear and the sequential lowering answers -- the same
    // answer, recomputed. Nothing is refused here.
    let parallel_pool_intrinsic = (closed_world && key_str == "parallelMap" && argc == 1)
        .then(|| typed_callback(plan, program, args[0], &[1, 2]))
        .flatten()
        .filter(|c| {
            c.returns_double
                && plan
                    .parallel_safe
                    .get(c.target as usize)
                    .copied()
                    .unwrap_or(false)
        })
        .map(|c| {
            let target = c.target;
            format!(
                "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                     void *ptr = KILN_VAL_PTR(v{o}); \
                     if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                         kiln_array *arr = (kiln_array *)ptr; \
                         kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                         if (arr->is_numeric && kiln_c99_builtin_store_epoch == 0 && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH && plain->body == (kiln_body_fn){symbol}_fn_{target}) {{ \
                             size_t count0 = arr->count; \
                             kiln_value built_val; \
                             int status = kiln_c99_new_array_with_capacity(&built_val, count0, 1); \
                             if (status != KILN_C99_OK) return status; \
                             kiln_array *built_arr = kiln_c99_array_of(built_val); \
                             int pmap_failed = 0; \
                             if (kiln_parallel_map(arr->elements, count0, built_arr->elements, {symbol}_pmap_{target}, &pmap_failed) == 0 && !pmap_failed) {{ \
                                 built_arr->count = count0; \
                                 v{v} = kiln_val_from_boundary(built_val); \
                                 handled = 1; \
                             }} \
                         }} \
                     }} \
                 }}",
                o = object.0,
                a0 = args[0].0,
                v = v,
            )
        });

    let parallel_map_intrinsic = if key_str == "parallelMap" && argc >= 1 {
        Some(format!(
            "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
                         void *ptr = KILN_VAL_PTR(v{o}); \
                         if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                             kiln_array *arr = (kiln_array *)ptr; \
                             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                             if (!plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
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
                             if (!plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
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
                             if (!plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
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
                         if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                             double d0; memcpy(&d0, &v{a0}, sizeof(d0)); \
                             if (d0 >= 0.0 && d0 == floor(d0)) {{ \
                                 size_t start = (size_t)d0; \
                                 size_t slen = start < s_recv->unit_len ? s_recv->unit_len - start : 0; \
                                 v{v} = kiln_c99_val_from_ascii(kiln_string_latin1(s_recv) + start, slen); \
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
                         if (src != NULL && sep != NULL && !kiln_string_is_utf16(src) && !kiln_string_is_utf16(sep) && sep->unit_len == 1) {{ \
                             kiln_value built_val; \
                             int status = kiln_c99_split_ascii_byte(&built_val, (const char *)kiln_string_latin1(src), src->unit_len, (char)kiln_string_unit_at(sep, 0), (size_t)0xFFFFFFFFu); \
                             if (status == KILN_C99_OK) {{ \
                                 v{v} = KILN_VAL_TAG_PTR(KILN_TAG_BUFFER_PREFIX, (const void *)(uintptr_t)built_val.bits); \
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
                         if (s_recv != NULL && s_ndl != NULL && !kiln_string_is_utf16(s_recv) && !kiln_string_is_utf16(s_ndl)) {{ \
                             const char *found = kiln_c99_memmem((const char *)kiln_string_latin1(s_recv), s_recv->unit_len, (const char *)kiln_string_latin1(s_ndl), s_ndl->unit_len); \
                             v{v} = kiln_c99_val_from_double(found ? (double)(found - (const char *)kiln_string_latin1(s_recv)) : -1.0); \
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
                         if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                             double d0, d1; memcpy(&d0, &v{a0}, sizeof(d0)); memcpy(&d1, &v{a1}, sizeof(d1)); \
                             if (d0 == floor(d0) && d1 == floor(d1) && d0 >= 0.0 && d1 >= d0 && (size_t)d1 <= s_recv->unit_len) {{ \
                                 size_t start = (size_t)d0; \
                                 size_t len = (size_t)d1 - start; \
                                 v{v} = kiln_c99_val_from_ascii((const char *)kiln_string_latin1(s_recv) + start, len); \
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

    let array_extra_intrinsic = match (key_str.as_str(), argc) {
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
        )),
        _ => None,
    };

    let string_extra_intrinsic = match (key_str.as_str(), argc) {
        ("startsWith", 1) => Some(format!(
            "if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_STRING(v{a0})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         const kiln_string *s_ndl = (const kiln_string *)KILN_VAL_PTR(v{a0}); \
                         if (s_recv != NULL && s_ndl != NULL) {{ \
                             int sw = (!kiln_string_is_utf16(s_recv) && !kiln_string_is_utf16(s_ndl) && s_recv->unit_len >= s_ndl->unit_len && memcmp(kiln_string_latin1(s_recv), kiln_string_latin1(s_ndl), s_ndl->unit_len) == 0); \
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
                             int ew = (!kiln_string_is_utf16(s_recv) && !kiln_string_is_utf16(s_ndl) && s_recv->unit_len >= s_ndl->unit_len && memcmp(kiln_string_latin1(s_recv) + (s_recv->unit_len - s_ndl->unit_len), kiln_string_latin1(s_ndl), s_ndl->unit_len) == 0); \
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
                         if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                             double d0; memcpy(&d0, &v{a0}, sizeof(d0)); \
                             if (isnan(d0)) d0 = 0.0; \
                             d0 = trunc(d0); \
                             if (d0 >= 0.0 && (size_t)d0 < s_recv->unit_len) {{ \
                                 v{v} = kiln_c99_val_from_double((double)kiln_string_unit_at(s_recv, (size_t)d0)); \
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
                         if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                             double d0; memcpy(&d0, &v{a0}, sizeof(d0)); \
                             if (isnan(d0)) d0 = 0.0; \
                             d0 = trunc(d0); \
                             if (d0 >= 0.0 && (size_t)d0 < s_recv->unit_len) {{ \
                                 v{v} = kiln_c99_val_from_unit(kiln_string_unit_at(s_recv, (size_t)d0)); \
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
                         if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                             size_t start = 0; \
                             while (start < s_recv->unit_len && ((unsigned char)kiln_string_latin1(s_recv)[start] <= ' ' || (unsigned char)kiln_string_latin1(s_recv)[start] == 0xa0)) start++; \
                             size_t end = s_recv->unit_len; \
                             while (end > start && ((unsigned char)kiln_string_latin1(s_recv)[end - 1] <= ' ' || (unsigned char)kiln_string_latin1(s_recv)[end - 1] == 0xa0)) end--; \
                             v{v} = kiln_c99_val_from_ascii((const char *)kiln_string_latin1(s_recv) + start, end - start); \
                             handled = 1; \
                         }} \
                     }}",
            o = object.0,
            v = v
        )),
        ("slice", 2) => Some(format!(
            "if (!handled && KILN_VAL_IS_STRING(v{o}) && KILN_VAL_IS_NUMBER(v{a0}) && KILN_VAL_IS_NUMBER(v{a1})) {{ \
                         const kiln_string *s_recv = (const kiln_string *)KILN_VAL_PTR(v{o}); \
                         if (s_recv != NULL && !kiln_string_is_utf16(s_recv)) {{ \
                             double d0, d1; memcpy(&d0, &v{a0}, sizeof(d0)); memcpy(&d1, &v{a1}, sizeof(d1)); \
                             if (d0 >= 0.0 && d0 == floor(d0) && d1 >= d0 && d1 == floor(d1)) {{ \
                                 size_t start = (size_t)d0; \
                                 size_t end = (size_t)d1 <= s_recv->unit_len ? (size_t)d1 : s_recv->unit_len; \
                                 size_t slen = start < end ? end - start : 0; \
                                 v{v} = kiln_c99_val_from_ascii((const char *)kiln_string_latin1(s_recv) + start, slen); \
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
            let need_box = ctx.boxed.map_or(true, |b| b.contains(&vid));
            let box_stmt = if need_box {
                format!(" v{v} = kiln_c99_val_from_double(d{v});")
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
                out.push_str(&format!("d{v} = {expr};{box_stmt} kiln_twin = 1;"));
                rest = &rest[expr_start + end + 2..];
            }
            out.push_str(rest);
            Some(out)
        }
        other => other,
    };
    let math_s = math_intrinsic.map(|s| format!(" {s}")).unwrap_or_default();
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

    writeln!(
                output,
                "    {{ static kiln_c99_site site = {{0}}; \
                kiln_val call_args[{n}]; int call_args_ready = 0; (void)call_args_ready; \
                int handled = 0; int kiln_twin = 0; (void)kiln_twin; {typed_remembered}\
                {await_resolve_s}{math_s}{push_s}{regex_s}{map_s}{parallel_pool_s}{parallel_map_s}{filter_s}{reduce_s}{slice_s}{join_s}{split_s}{index_of_s}{substring_s}{collection_s}{json_s}{sort_s}{array_extra_s}{string_extra_s}{iterator_s} \
                if (!handled && KILN_VAL_IS_OBJECT(v{o})) {{ \
                    const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(v{o}); if (obj == NULL) __builtin_unreachable(); \
                    {typed_fast_block}\
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
                            kiln_value bself = kiln_val_to_boundary(v{o}); \
                            kiln_value bargs[{n}]; \
                            {prep_args}for (size_t bi = 0; bi < {argc}; ++bi) bargs[bi] = kiln_val_to_boundary(call_args[bi]); \
                            kiln_value bresult = kiln_c99_undefined(); \
                            int status = kiln_c99_invoke_method(host, &bresult, KILN_VAL_METHOD_ID(cached_fn), bself, {argc} > 0 ? bargs : NULL, (size_t){argc}, depth, thrown); \
                            {check} \
                            v{v} = kiln_val_from_boundary(bresult); \
                            handled = 1; \
                        }} \
                    }} \
                }} \
                if (!handled && kiln_c99_cached_method_matches(&site, v{o}) && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
                    kiln_value bself = kiln_val_to_boundary(v{o}); \
                    kiln_value bargs[{n}]; \
                    {prep_args}for (size_t bi = 0; bi < {argc}; ++bi) bargs[bi] = kiln_val_to_boundary(call_args[bi]); \
                    kiln_value bresult = kiln_c99_undefined(); \
                    int status = kiln_c99_invoke_method(host, &bresult, site.cached_method, bself, {argc} > 0 ? bargs : NULL, (size_t){argc}, depth, thrown); \
                    {check} \
                    v{v} = kiln_val_from_boundary(bresult); \
                    handled = 1; \
                }} \
                if (!handled) {{ \
                    {prep_args}int status = kiln_c99_val_call_method_key(host, v{o}, call_args, (size_t){argc}, {symbol}_key_{name}, depth, thrown, &site, {diagnostic_source}, (size_t){pc}, \"{label}\", &v{v}); \
                    {check} \
                }} {typed_sync} }}",
                o = object.0,
                typed_sync = if typed && spec.exits && spec.calls.contains(&vid) {
                    let invariant = lookup_speculation_invariant("method_call_number");
                    format!("/* Speculation exit [{}]: {} */ if (!kiln_twin) {{ if (KILN_UNLIKELY(!KILN_VAL_IS_NUMBER(v{v}))) {{ {} goto kiln_generic_resume_{v}; }} memcpy(&d{v}, &v{v}, sizeof(d{v})); }}", invariant.name, invariant.language_rule, super::materialize_all_sunk(ctx.graph, symbol, ctx.defs, ctx.array_slots))
                } else if typed {
                    format!("if (!kiln_twin) memcpy(&d{v}, &v{v}, sizeof(d{v}));")
                } else {
                    String::new()
                }
            )
            .unwrap();
}
