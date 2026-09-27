//! The callback loops a call site runs inline over a typed array (`map`, `filter`, `reduce`), and
//! `parallelReduce`'s tree run sequentially -- the lowerings [`super::call_method`] composes
//! beside its array loops.
//!
//! # A measured fast path over the TypeScript owner (ADR 0060)
//!
//! `%TypedArray%.prototype.map`, `filter` and `reduce` are the shared TypeScript in
//! `kiln-builtins/src/typedarray.ts`, and that stays their owner: every guard below that fails
//! leaves `handled` clear and the ordinary method call runs the TypeScript. The bootstrap is
//! compiled generic-only, so the TypeScript's loop pays a generic element read and a
//! `Function.prototype.call` per element: 250-290 ms per million `Float64Array` elements natively,
//! against node's 9-20 ms (parallel issue 16). Here the site's own specialized code runs the loop,
//! calling the callback's typed entry directly when the site proves which function it is.
//!
//! The equivalence invariant the guards name: the receiver is a view of a Number element type that
//! is not a Node `Buffer`, and no built-in has been written since the bootstrap
//! (`kiln_c99_builtins_unmodified()`). A native view has neither own properties nor a
//! prototype of its own choosing (both refuse), so with the built-ins untouched the method is the
//! bootstrap's, `ValidateTypedArray` passes exactly when the buffer is attached, and the species
//! constructor is the intrinsic one -- a plain allocation of the same kind. Each loop then takes
//! the TypeScript's steps in its order: the length read once, the element read at its visit
//! (`undefined` once a callback has detached the buffer), the callback called with
//! `(element, index, view)` and `this` the thisArg, and a result written through `ToNumber`.
//!
//! `parallelReduce` has no sequential method to fall back to, so its tree -- leaves of
//! [`crate::parallel::REDUCE_LEAF`] folded left to right, then combined pairwise level by level
//! -- is written out here once more, exactly as `kiln-builtins/src/parallel.ts` applies it. The
//! pooled lowering answers first when it can; this is what the site runs when the pool declines,
//! at the speed of `reduce` at the same site (parallel issue 15).

use super::call_method::{TypedCallback, typed_call_args_for};

/// What every loop here reads from its call site.
pub(super) struct LoopSite<'a> {
    pub symbol: &'a str,
    /// The receiver, the callback and the result, as value ids.
    pub object: u32,
    pub callback: u32,
    pub v: u32,
    /// The site's status check (reads `status`) and its typed-entry failure handling.
    pub check: &'a str,
    pub typed_failed: &'a str,
    /// The callback's `this` argument (a C expression) and the guard under which a sloppy
    /// callback may be handed it unboxed.
    pub this_arg: &'a str,
    pub this_ok: &'a str,
}

/// A call of closure `closure` with `this` `recv` and the arguments `args` (C expressions), the
/// answer in `out`, failures through `check`.
pub(super) fn closure_call(
    closure: &str,
    recv: &str,
    args: &[&str],
    out: &str,
    check: &str,
) -> String {
    let n = args.len();
    let args = args.join(", ");
    format!(
        "{{ kiln_val vargs[{n}] = {{ {args} }}; int status; \
             if (!{closure}->publishes_frame) {{ kiln_call_context ctx_call = {{ host, thrown, NULL, depth + 1 }}; status = {closure}->body(&ctx_call, &{out}, {closure}->upvalues, &{recv}, vargs, (size_t){n}); }} \
             else status = kiln_c99_invoke_closure(host, &{out}, {closure}, {recv}, vargs, (size_t){n}, depth, thrown, NULL, 0); \
             if (KILN_UNLIKELY(status != KILN_C99_OK)) {{ {check} }} }}"
    )
}

/// `this` for a closure called with `this_arg`: an arrow's own, else the argument, which a sloppy
/// function sees as the global object when it is nullish.
fn receiver(closure: &str, recv: &str, this_arg: &str) -> String {
    format!(
        "kiln_val {recv} = {closure}->arrow ? {closure}->captured_this : {this_arg}; \
         if (!{closure}->arrow && !{closure}->strict) (void)kiln_c99_global_object_val(&{recv});"
    )
}

/// The element at `idx` as the TypeScript reads it (`this[k]`): `undefined` once the buffer is
/// detached.
const ELEMENT: &str = "((view->buffer->detached || idx >= view->length) ? KILN_VAL_UNDEFINED : kiln_c99_val_from_double(kiln_typed_load(view, idx)))";

/// The shared head of the three typed-array loops: a view of a Number kind, not a `Buffer`, the
/// built-ins untouched, and a callable closure. `body` sees `view`, `plain` and `vrecv`.
fn typed_array_guard(site: &LoopSite, body: &str) -> String {
    let LoopSite {
        object: o,
        callback: a0,
        this_arg,
        this_ok,
        ..
    } = *site;
    let recv = receiver("plain", "vrecv", this_arg);
    format!(
        "if (!handled && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0})) {{ \
             void *ptr = KILN_VAL_PTR(v{o}); \
             if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY) {{ \
                 kiln_typed_array *view = (kiln_typed_array *)ptr; \
                 kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
                 if (KILN_LIKELY(kiln_c99_builtins_unmodified() && !view->is_buffer && !view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64 && !plain->generator && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH && ({this_ok}))) {{ \
                     {recv} \
                     {body} \
                 }} \
             }} \
         }}"
    )
}

/// The typed entry's arguments for a callback of declared `arity` called with `(element, index)`.
fn element_params(c: &TypedCallback) -> Vec<&'static str> {
    if c.arity == 2 {
        vec!["d_elem", "(double)idx"]
    } else {
        vec!["d_elem"]
    }
}

/// `%TypedArray%.prototype.map` over a view: the result is a new view of the same kind, each
/// callback answer written through `ToNumber`. With `typed`, a callback whose closure is the
/// proven proto and that reaches nothing outside itself is entered through its typed entry.
pub(super) fn typed_array_map(site: &LoopSite, typed: Option<&TypedCallback>) -> String {
    let LoopSite {
        symbol,
        object: o,
        v,
        check,
        typed_failed,
        ..
    } = *site;
    let write_double = "if (out_f64) memcpy(out_bytes + idx * 8, &num_val, sizeof num_val); else kiln_c99_typed_set(built_view, idx, num_val);";
    let write_boxed = format!(
        "double num_val; \
         if (KILN_VAL_IS_NUMBER(prod)) memcpy(&num_val, &prod, sizeof num_val); \
         else {{ int status = kiln_c99_to_number_checked(host, kiln_val_to_boundary(prod), &num_val, depth, thrown); if (KILN_UNLIKELY(status != KILN_C99_OK)) {{ {check} }} }} \
         {write_double}"
    );
    let call = closure_call(
        "plain",
        "vrecv",
        &[
            "elem",
            "kiln_c99_val_from_double((double)idx)",
            &format!("v{o}"),
        ],
        "prod",
        check,
    );
    let generic = format!(
        "for (size_t idx = 0; idx < count0; ++idx) {{ \
             kiln_val elem = {ELEMENT}; kiln_val prod = KILN_VAL_UNDEFINED; \
             {call} \
             {write_boxed} \
         }}"
    );
    let loops = match typed.filter(|c| !c.needs.runtime) {
        Some(c) => {
            let target = c.target;
            let call_args = typed_call_args_for(c.needs, "plain", "vrecv", &element_params(c));
            let answer = if c.returns_double {
                format!(
                    "double num_val = {symbol}_fn_{target}_t({call_args}); \
                     if (KILN_UNLIKELY(kiln_c99_typed_failed(num_val))) {typed_failed} \
                     {write_double}"
                )
            } else {
                format!(
                    "kiln_val prod = {symbol}_fn_{target}_t({call_args}); \
                     if (KILN_UNLIKELY(prod == KILN_VAL_TYPED_FAIL)) {typed_failed} \
                     {write_boxed}"
                )
            };
            let relies = &c.relies;
            format!(
                "if (plain->body == (kiln_body_fn){symbol}_fn_{target}{relies}) {{ \
                     for (size_t idx = 0; idx < count0; ++idx) {{ \
                         double d_elem = kiln_typed_load(view, idx); \
                         {answer} \
                     }} \
                 }} else {{ {generic} }}"
            )
        }
        None => generic,
    };
    typed_array_guard(
        site,
        &format!(
            "size_t count0 = view->length; \
             kiln_value built_val; \
             int status = kiln_c99_typed_alloc_ex(&built_val, view->kind, count0, 0); \
             if (status != KILN_C99_OK) return status; \
             kiln_typed_array *built_view = kiln_c99_typed_array_of(built_val); \
             uint8_t *out_bytes = built_view->buffer->bytes + built_view->byte_offset; \
             int out_f64 = built_view->kind == KILN_TYPED_FLOAT64; \
             {loops} \
             v{v} = kiln_val_from_boundary(built_val); \
             handled = 1;"
        ),
    )
}

/// `%TypedArray%.prototype.filter` over a view: each kept element's bytes, as read before its
/// callback ran, go to a scratch view of the same kind; the result is a new view of the kept
/// count, allocated after the loop as the TypeScript allocates it. An element read as `undefined`
/// (the buffer detached by a callback) and kept is written as `ToNumber(undefined)`.
pub(super) fn typed_array_filter(site: &LoopSite, typed: Option<&TypedCallback>) -> String {
    let LoopSite {
        symbol,
        object: o,
        v,
        check,
        typed_failed,
        ..
    } = *site;
    let call = closure_call(
        "plain",
        "vrecv",
        &[
            "elem",
            "kiln_c99_val_from_double((double)idx)",
            &format!("v{o}"),
        ],
        "prod",
        check,
    );
    let generic = format!(
        "for (size_t idx = 0; idx < count0; ++idx) {{ \
             int live = !view->buffer->detached && idx < view->length; \
             uint64_t raw = 0; \
             if (live) memcpy(&raw, view->buffer->bytes + view->byte_offset + idx * width, width); \
             kiln_val elem = live ? kiln_c99_val_from_double(kiln_typed_load(view, idx)) : KILN_VAL_UNDEFINED; \
             kiln_val prod = KILN_VAL_UNDEFINED; \
             {call} \
             if (kiln_c99_val_truthy(prod)) {{ \
                 if (live) memcpy(keep + kept * width, &raw, width); else kiln_c99_typed_set(keep_view, kept, NAN); \
                 ++kept; \
             }} \
         }}"
    );
    let loops = match typed.filter(|c| !c.needs.runtime) {
        Some(c) => {
            let target = c.target;
            let call_args = typed_call_args_for(c.needs, "plain", "vrecv", &element_params(c));
            let (call, truthy) = if c.returns_double {
                (
                    format!(
                        "double r_elem = {symbol}_fn_{target}_t({call_args}); \
                         if (KILN_UNLIKELY(kiln_c99_typed_failed(r_elem))) {typed_failed}"
                    ),
                    "(r_elem == r_elem && r_elem != 0.0)",
                )
            } else {
                (
                    format!(
                        "kiln_val r_elem = {symbol}_fn_{target}_t({call_args}); \
                         if (KILN_UNLIKELY(r_elem == KILN_VAL_TYPED_FAIL)) {typed_failed}"
                    ),
                    "kiln_c99_val_truthy(r_elem)",
                )
            };
            let relies = &c.relies;
            format!(
                "if (plain->body == (kiln_body_fn){symbol}_fn_{target}{relies}) {{ \
                     const uint8_t *src = view->buffer->bytes + view->byte_offset; \
                     for (size_t idx = 0; idx < count0; ++idx) {{ \
                         double d_elem = kiln_typed_load(view, idx); \
                         {call} \
                         if ({truthy}) {{ memcpy(keep + kept * width, src + idx * width, width); ++kept; }} \
                     }} \
                 }} else {{ {generic} }}"
            )
        }
        None => generic,
    };
    typed_array_guard(
        site,
        &format!(
            "size_t count0 = view->length; \
             size_t width = view->kind <= KILN_TYPED_UINT8_CLAMPED ? 1 : view->kind <= KILN_TYPED_UINT16 ? 2 : view->kind <= KILN_TYPED_FLOAT32 ? 4 : 8; \
             kiln_value keep_val; \
             int status = kiln_c99_typed_alloc_ex(&keep_val, view->kind, count0, 0); \
             if (status != KILN_C99_OK) return status; \
             kiln_typed_array *keep_view = kiln_c99_typed_array_of(keep_val); \
             uint8_t *keep = keep_view->buffer->bytes + keep_view->byte_offset; \
             size_t kept = 0; \
             {loops} \
             kiln_value built_val; \
             status = kiln_c99_typed_alloc_ex(&built_val, view->kind, kept, 0); \
             if (status != KILN_C99_OK) return status; \
             kiln_typed_array *built_view = kiln_c99_typed_array_of(built_val); \
             if (kept != 0) memcpy(built_view->buffer->bytes + built_view->byte_offset, keep, kept * width); \
             v{v} = kiln_val_from_boundary(built_val); \
             handled = 1;"
        ),
    )
}

/// `%TypedArray%.prototype.reduce(callback, initial)` over a view: every index visited (a typed
/// array has no holes), the element `undefined` once a callback has detached the buffer. With
/// `typed` -- proven to return a number, the initial value proven one too, and reaching nothing
/// outside itself -- the accumulator stays a double for the whole loop.
pub(super) fn typed_array_reduce(
    site: &LoopSite,
    initial: u32,
    typed: Option<(&TypedCallback, &str)>,
) -> String {
    let LoopSite {
        symbol,
        object: o,
        v,
        check,
        typed_failed,
        ..
    } = *site;
    let call = closure_call(
        "plain",
        "vrecv",
        &[
            "vacc",
            ELEMENT,
            "kiln_c99_val_from_double((double)idx)",
            &format!("v{o}"),
        ],
        "prod",
        check,
    );
    let generic = format!(
        "kiln_val vacc = v{initial}; \
         for (size_t idx = 0; idx < count0; ++idx) {{ \
             kiln_val prod = KILN_VAL_UNDEFINED; \
             {call} \
             vacc = prod; \
         }} \
         v{v} = vacc;"
    );
    let loops = match typed.filter(|(c, _)| !c.needs.runtime && c.returns_double) {
        Some((c, init_dacc)) => {
            let target = c.target;
            let call_args = typed_call_args_for(c.needs, "plain", "vrecv", &["dacc", "d_elem"]);
            let relies = &c.relies;
            format!(
                "if (plain->body == (kiln_body_fn){symbol}_fn_{target}{relies}) {{ \
                     double dacc = {init_dacc}; \
                     for (size_t idx = 0; idx < count0; ++idx) {{ \
                         double d_elem = kiln_typed_load(view, idx); \
                         double r_acc = {symbol}_fn_{target}_t({call_args}); \
                         if (KILN_UNLIKELY(kiln_c99_typed_failed(r_acc))) {typed_failed} \
                         dacc = r_acc; \
                     }} \
                     v{v} = kiln_c99_val_from_double(dacc); \
                 }} else {{ {generic} }}"
            )
        }
        None => generic,
    };
    typed_array_guard(
        site,
        &format!("size_t count0 = view->length; {loops} handled = 1;"),
    )
}

/// `parallelReduce(callback, combine, initial)` run on the realm thread: the tree of
/// `kiln-builtins/src/parallel.ts` over an array (holes and indices past the current length
/// skipped, as `k in O` skips them) or a view of a Number kind. Every leaf is folded from `initial`
/// with `callback(acc, element, index, receiver)`, then each level of the tree is
/// `combine(r[2k], r[2k + 1])` left to right, an odd last one carried up; no input answers
/// `initial`. Each callback is called with `this` undefined, as the TypeScript's `.call(undefined,
/// ...)` does.
///
/// The leaf results live in a collected array the conservative stack scan finds through
/// `level_val`, so a callback that answers objects keeps them alive. With `typed` (both callbacks'
/// typed entries answering doubles and reaching nothing outside themselves, and `initial` a
/// number), the elements are read as doubles and the levels are a scratch `Float64Array`.
pub(super) fn reduce_tree(
    site: &LoopSite,
    combine: u32,
    initial: u32,
    typed: Option<(&TypedCallback, &TypedCallback)>,
) -> String {
    let LoopSite {
        symbol,
        object: o,
        callback: a0,
        v,
        check,
        typed_failed,
        ..
    } = *site;
    let leaf = crate::parallel::REDUCE_LEAF;
    let fold_recv = receiver("plain", "vrecv", "KILN_VAL_UNDEFINED");
    let comb_recv = receiver("pcomb", "crecv", "KILN_VAL_UNDEFINED");
    let fold_call = closure_call(
        "plain",
        "vrecv",
        &[
            "vacc",
            "elem",
            "kiln_c99_val_from_double((double)idx)",
            &format!("v{o}"),
        ],
        "prod",
        check,
    );
    let comb_call = closure_call(
        "pcomb",
        "crecv",
        &["level->elements[2 * i]", "level->elements[2 * i + 1]"],
        "prod",
        check,
    );
    let generic = format!(
        "kiln_value level_val; \
         int status = kiln_c99_new_array_with_capacity(&level_val, leaves, 0); \
         if (status != KILN_C99_OK) return status; \
         kiln_array *level = kiln_c99_array_of(level_val); \
         for (size_t start = 0; start < len; start += {leaf}) {{ \
             size_t end = start + {leaf} < len ? start + {leaf} : len; \
             kiln_val vacc = v{initial}; \
             for (size_t idx = start; idx < end; ++idx) {{ \
                 kiln_val elem; \
                 if (arr != NULL) {{ \
                     if (idx >= arr->count || arr->elements[idx] == KILN_VAL_HOLE) continue; \
                     elem = arr->elements[idx]; \
                 }} else {{ \
                     if (view->buffer->detached || idx >= view->length) continue; \
                     elem = kiln_c99_val_from_double(kiln_typed_load(view, idx)); \
                 }} \
                 kiln_val prod = KILN_VAL_UNDEFINED; \
                 {fold_call} \
                 vacc = prod; \
             }} \
             level->elements[level->count++] = vacc; \
         }} \
         size_t m = level->count; \
         while (m > 1) {{ \
             size_t pairs = m / 2; \
             for (size_t i = 0; i < pairs; ++i) {{ \
                 kiln_val prod = KILN_VAL_UNDEFINED; \
                 {comb_call} \
                 level->elements[i] = prod; \
             }} \
             if (m & 1) level->elements[pairs] = level->elements[m - 1]; \
             m = pairs + (m & 1); \
             level->count = m; \
         }} \
         v{v} = level->elements[0];"
    );
    let body = match typed.filter(|(f, c)| {
        !f.needs.runtime && !c.needs.runtime && f.returns_double && c.returns_double
    }) {
        Some((f, c)) => {
            let (fold, comb) = (f.target, c.target);
            let (fold_relies, comb_relies) = (&f.relies, &c.relies);
            let mut fold_params = vec!["dacc", "d_elem"];
            if f.arity == 3 {
                fold_params.push("(double)idx");
            }
            let fold_args = typed_call_args_for(f.needs, "plain", "vrecv", &fold_params);
            let comb_args =
                typed_call_args_for(c.needs, "pcomb", "crecv", &["lvl[2 * i]", "lvl[2 * i + 1]"]);
            format!(
                "if (plain->body == (kiln_body_fn){symbol}_fn_{fold}{fold_relies} && pcomb->body == (kiln_body_fn){symbol}_fn_{comb}{comb_relies} && KILN_VAL_IS_NUMBER(v{initial}) && (arr == NULL || arr->is_numeric)) {{ \
                     kiln_value level_val; \
                     int status = kiln_c99_typed_alloc_ex(&level_val, KILN_TYPED_FLOAT64, leaves, 0); \
                     if (status != KILN_C99_OK) return status; \
                     kiln_typed_array *level_view = kiln_c99_typed_array_of(level_val); \
                     double *lvl = (double *)(void *)(level_view->buffer->bytes + level_view->byte_offset); \
                     double dinit; memcpy(&dinit, &v{initial}, sizeof dinit); \
                     for (size_t start = 0, leaf_at = 0; start < len; start += {leaf}, ++leaf_at) {{ \
                         size_t end = start + {leaf} < len ? start + {leaf} : len; \
                         double dacc = dinit; \
                         for (size_t idx = start; idx < end; ++idx) {{ \
                             double d_elem; \
                             if (arr != NULL) memcpy(&d_elem, &arr->elements[idx], sizeof d_elem); \
                             else d_elem = kiln_typed_load(view, idx); \
                             double r_acc = {symbol}_fn_{fold}_t({fold_args}); \
                             if (KILN_UNLIKELY(kiln_c99_typed_failed(r_acc))) {typed_failed} \
                             dacc = r_acc; \
                         }} \
                         lvl[leaf_at] = dacc; \
                     }} \
                     size_t m = leaves; \
                     while (m > 1) {{ \
                         size_t pairs = m / 2; \
                         for (size_t i = 0; i < pairs; ++i) {{ \
                             double r_comb = {symbol}_fn_{comb}_t({comb_args}); \
                             if (KILN_UNLIKELY(kiln_c99_typed_failed(r_comb))) {typed_failed} \
                             lvl[i] = r_comb; \
                         }} \
                         if (m & 1) lvl[pairs] = lvl[m - 1]; \
                         m = pairs + (m & 1); \
                     }} \
                     v{v} = kiln_c99_val_from_double(lvl[0]); \
                 }} else {{ {generic} }}"
            )
        }
        None => generic,
    };
    format!(
        "if (!handled && kiln_c99_kiln_extensions_on() && kiln_c99_builtins_unmodified() && (v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && KILN_VAL_IS_CLOSURE(v{a0}) && KILN_VAL_IS_CLOSURE(v{combine}) && !kiln_inheriting && depth < KILN_C99_MAX_CALL_DEPTH) {{ \
             void *ptr = KILN_VAL_PTR(v{o}); \
             kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(v{a0}); \
             kiln_closure *pcomb = (kiln_closure *)KILN_VAL_PTR(v{combine}); \
             kiln_array *arr = NULL; \
             kiln_typed_array *view = NULL; \
             size_t len = 0; \
             if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {{ \
                 arr = (kiln_array *)ptr; \
                 if (arr->properties.tag == KILN_VALUE_UNDEFINED) len = arr->count; else arr = NULL; \
             }} else if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY) {{ \
                 view = (kiln_typed_array *)ptr; \
                 if (!view->is_buffer && !view->buffer->detached && view->kind <= KILN_TYPED_FLOAT64) len = view->length; else view = NULL; \
             }} \
             if ((arr != NULL || view != NULL) && !plain->generator && !pcomb->generator) {{ \
                 size_t leaves = (len + {leaf} - 1) / {leaf}; \
                 if (leaves == 0) {{ \
                     v{v} = v{initial}; \
                 }} else {{ \
                     {fold_recv} \
                     {comb_recv} \
                     {body} \
                 }} \
                 handled = 1; \
             }} \
         }}"
    )
}
