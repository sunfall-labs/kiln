
void kiln_c99_publish_diagnostic(void);

static kiln_value kiln_c99_builtin(uint64_t code) {
    return kiln_c99_value(KILN_VALUE_BUILTIN, code, NULL, 0);
}

/* A closure and a builtin are both callable, and every operator that coerces treats them alike:
 * `valueOf` hands the object back, so `toString` decides. */
static int kiln_c99_is_callable(kiln_value value) {
    /* A proxy answers for what it stands in front of: `new Proxy(f, {})` is a function and
     * `new Proxy({}, {})` is not, and `typeof` has to say so before any trap is consulted. */
    value = kiln_c99_proxy_inner(value);
    return value.tag == KILN_VALUE_FUNCTION || value.tag == KILN_VALUE_BUILTIN ||
           value.tag == KILN_VALUE_METHOD || value.tag == KILN_VALUE_SETTLER ||
           value.tag == KILN_VALUE_BOUND || value.tag == KILN_VALUE_COMMONJS_REQUIRE;
}

/* Every reference value reduces to a primitive before a coercing operator, and none of them can
 * do it by calling: this backend has no user `valueOf` to reach, because a property holding a
 * function is refused at the point it would be called. So reduction is conversion to text. */
static int kiln_c99_is_reference(kiln_value value) {
    /* What a coercing operator has to reduce to a primitive first.
     *
     * Listed the other way round -- what is *not* one -- for the same reason
     * `kiln_c99_crosses_boundary` below is: an allowlist of the object kinds silently stops being
     * true the day a tag is added, and says nothing when it does. This one was an allowlist and
     * had fallen three tags behind. A `Temporal` value, a promise and a regexp were all answered
     * "already primitive", so `"x" + aPlainDate` never reduced anything: the concatenation read
     * the unchecked `kiln_c99_text`, which had no case for the tag either, and appended the empty
     * span its `default:` hands back. The program printed "x" and carried on, where the language
     * throws -- `Temporal`'s `valueOf` is specified to, precisely so that `date1 < date2` cannot
     * quietly compare two NaNs.
     *
     * The primitives are the closed list, and the three internal tags are on it because they are
     * never an operand: a cell lives only in a captured slot, a hole only in an array's storage,
     * and an uninitialized slot throws when it is read.
     *
     * A Date is deliberately *not* a reference: `as_number` already gives its instant, which is
     * what `valueOf` would produce, so reducing it to text first would turn every date
     * subtraction into NaN. That is a shortcut that happens to reproduce both halves of the
     * language's answer, and it is the one exception here. */
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
    case KILN_VALUE_NULL:
    case KILN_VALUE_BOOLEAN:
    case KILN_VALUE_NUMBER:
    case KILN_VALUE_STRING:
    case KILN_VALUE_SYMBOL:
    case KILN_VALUE_BIGINT:
    case KILN_VALUE_CELL:
    case KILN_VALUE_HOLE:
    case KILN_VALUE_UNINITIALIZED:
    case KILN_VALUE_DATE:
        return 0;
    default:
        return 1;
    }
}

/* Whether this value's text form is a fact about its *tag* rather than about its property store.
 *
 * The companion to `kiln_c99_is_reference`, and the reason `kiln_c99_to_primitive` can fall back
 * to `kiln_c99_text` for some references and must not for others. A function, a namespace, a
 * promise, a generator, a collection, a regexp, a view, a buffer and a `Temporal` value each
 * convert to one string decided entirely by what they are; a plain object and an array convert to
 * something decided by what they hold, which only the `valueOf`/`toString` walk can ask. Answering
 * the tagged form for those two instead is what made `Object.create(null) + 1` produce a string
 * where the language stops the program. */
static int kiln_c99_has_intrinsic_text(kiln_value value) {
    switch (value.tag) {
    case KILN_VALUE_COLLECTION:
    case KILN_VALUE_PROMISE:
    case KILN_VALUE_GENERATOR:
    case KILN_VALUE_REGEXP:
    case KILN_VALUE_ARRAY_BUFFER:
    case KILN_VALUE_TYPED_ARRAY:
    case KILN_VALUE_DATA_VIEW:
    case KILN_VALUE_TEMPORAL:
    /* A Date's human string is intrinsic too. `kiln_c99_is_reference` keeps one away from
     * `kiln_c99_to_primitive` entirely -- see the note there -- so nothing reaches this through a
     * Date today; it is here because the answer is a fact about the tag and leaving it out would
     * make the classifier true only by accident of who calls it. */
    case KILN_VALUE_DATE:
        return 1;
    default:
        return kiln_c99_is_callable(value);
    }
}

/* Whether a value has to be converted before it crosses the embedding boundary.
 *
 * A different question from the one above, and asked the other way round on purpose: the
 * boundary carries the five scalar tags and nothing else, so everything above the last of them
 * needs converting. Listing what *cannot* cross means a tag added later is covered the day it is
 * added -- an allowlist of what needs conversion is how `console.log([1])` came to hand the host
 * a raw array, and a Date would have been next. */
static int kiln_c99_crosses_boundary(kiln_value value) {
    return value.tag > KILN_VALUE_STRING;
}

/* Negative zero compares equal to zero, so the sign is the only thing that tells them apart --
 * and a console shows the difference where `String` does not. */
static int kiln_c99_signbit(double value) {
    return signbit(value) != 0;
}

static kiln_value kiln_c99_hole(void) {
    return kiln_c99_value(KILN_VALUE_HOLE, 0, NULL, 0);
}

static int kiln_c99_is_hole(kiln_value value) {
    return value.tag == KILN_VALUE_HOLE;
}

static kiln_value kiln_c99_uninitialized(void) {
    return kiln_c99_value(KILN_VALUE_UNINITIALIZED, 0, NULL, 0);
}

static kiln_value kiln_c99_null(void) {
    return kiln_c99_value(KILN_VALUE_NULL, 0, NULL, 0);
}

static kiln_value kiln_c99_boolean(int value) {
    return kiln_c99_value(KILN_VALUE_BOOLEAN, value ? 1u : 0u, NULL, 0);
}

static kiln_value kiln_c99_function(kiln_closure *closure) {
    return kiln_c99_value(KILN_VALUE_FUNCTION, (uint64_t)(uintptr_t)closure, NULL, 0);
}

static kiln_closure *kiln_c99_closure_of(kiln_value value) {
    return (kiln_closure *)(uintptr_t)value.bits;
}


static kiln_val *kiln_c99_cell_of(kiln_value value) {
    return (kiln_val *)(uintptr_t)value.bits;
}

static int kiln_c99_val_make_cell(kiln_val *slot);

/* Turn a slot into a box in place.  Whatever the slot already held becomes the box's first
 * contents, which is what makes `let x = 1;` followed by a closure over `x` see the 1. */
static int kiln_c99_make_cell(kiln_value *slot) {
    kiln_val val = kiln_val_from_boundary(*slot);
    int status = kiln_c99_val_make_cell(&val);
    if (status != KILN_C99_OK) return status;
    *slot = kiln_val_to_boundary(val);
    return KILN_C99_OK;
}

/* The marker goes *through* an existing box rather than over it.  The cell is created before
 * the marker is written -- it has to exist before a hoisted function can capture it -- so
 * overwriting the slot here would drop the box every capture already points at. */
static void kiln_c99_mark_uninitialized(kiln_value *slot) {
    if (slot->tag == KILN_VALUE_CELL) {
        kiln_c99_heap_store_val(kiln_c99_cell_of(*slot), kiln_c99_cell_of(*slot),
                                 KILN_VAL_UNINITIALIZED);
    } else {
        *slot = kiln_c99_uninitialized();
    }
}

/* The captured values are boxes, copied by pointer rather than by contents: two closures over one
 * variable have to see each other's writes, which is the whole reason a cell exists. */
static int kiln_c99_make_closure(kiln_value *out, kiln_body_fn body, const kiln_string *name,
                                 const char *source, size_t source_len,
                                 const char *diagnostic_source,
                                 uint32_t proto, uint32_t arity, int generator,
                                 int asynchronous,
                                 int async_generator, int is_method, int arrow, int strict, kiln_val receiver,
                                 kiln_val new_target, uint32_t local_count,
                                 uint32_t stack_size,
                                 uint32_t handler_depth, uint32_t prologue_resume,
                                 const kiln_val *captured,
                                 uint32_t count) {
    kiln_closure *closure = (kiln_closure *)kiln_c99_gc_alloc_hot(KILN_GC_CLOSURE, sizeof *closure, 1);
    kiln_val *upvalues = NULL;
    if (closure == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    if (count != 0) {
        if (count <= 4) {
            upvalues = closure->inline_upvalues;
        } else {
            upvalues = (kiln_val *)malloc((size_t)count * sizeof *upvalues);
            if (upvalues == NULL) {
                kiln_c99_gc_release(closure);
                return KILN_C99_OUT_OF_MEMORY;
            }
            /* The closure owns memory outside its block now (`KILN_GC_FINALIZED_AT_ALLOC`). */
            kiln_c99_gc_note_finalizable(closure);
        }
        kiln_c99_heap_copy_vals(closure, upvalues, captured, count);
    }
    closure->body = body;
    closure->generator = generator;
    closure->asynchronous = asynchronous;
    closure->local_count = local_count;
    closure->stack_size = stack_size;
    closure->handler_depth = handler_depth;
    closure->arrow = arrow != 0;
    closure->strict = strict;
    closure->publishes_frame = 1;
    kiln_c99_heap_store_val(closure, &closure->captured_this, arrow ? receiver : KILN_VAL_UNDEFINED);
    closure->async_generator = async_generator;
    closure->is_method = is_method;
    /* Captured whether or not it is wanted, because whether it is wanted is a property of the
     * closure and the value is a property of *here*. */
    kiln_c99_heap_store_val(closure, &closure->captured_new_target, new_target);
    kiln_c99_heap_store_value(closure, &closure->captured_arguments, kiln_c99_undefined());
    /* `arrow` is 1 for an arrow that reads the enclosing frame and 2 for one the compiler proved
     * never does (neither it nor any arrow nested in it names `arguments` or `new.target`; the
     * `publishes_frame` fixpoint). Capturing for the second kind built the enclosing call's whole
     * `arguments` object -- an object, a property array and four keys -- at every arrow's
     * creation, for nobody to read: `new Promise((resolve) => setTimeout(resolve, ms))` paid it on
     * every sleep, and it was most of what a pending task cost. */
    if (arrow == 1) {
        /* Only an arrow, and only at creation: the list belongs to a frame that will be gone
         * before the arrow is called, so it has to be copied out now rather than pointed at. */
        int captured = kiln_c99_visible_arguments(&closure->captured_arguments);
        if (captured != KILN_C99_OK) {
            if (upvalues != closure->inline_upvalues) free(upvalues);
            kiln_c99_gc_release(closure);
            return captured;
        }
    }
    closure->region = 0;
    kiln_c99_heap_store_value(closure, &closure->own_prototype, kiln_c99_undefined());
    kiln_c99_heap_store_value(closure, &closure->own_store, kiln_c99_undefined());
    kiln_c99_heap_store_pointer(closure, &closure->name, name);
    kiln_c99_heap_store_pointer(closure, &closure->source, source);
    closure->source_len = source_len;
    kiln_c99_heap_store_value(closure, &closure->source_value, kiln_c99_undefined());
    kiln_c99_heap_store_pointer(closure, &closure->diagnostic_source, diagnostic_source);
    closure->arity = arity;
    closure->prologue_resume = prologue_resume;
    closure->proto = proto;
    closure->upvalue_count = count;
    closure->upvalues = upvalues;
    *out = kiln_c99_function(closure);
    return KILN_C99_OK;
}

/* A closure-making site the emitter proved runs once per realm, at the runtime bootstrap's top
 * level, capturing nothing (`EmissionPolicy::shared_region`; realms issue 08): the closure lives
 * in the shared immutable region, made by the realm that builds it and handed to every realm
 * after. `*site` is the site's process-wide slot. A realm that does not use the region, a site
 * the builder never reached, a full reservation, or an arrow whose receiver is not the module's
 * `undefined` all make an ordinary closure exactly as `kiln_c99_make_closure` would -- the
 * region is an economy, never a requirement. */
static int kiln_c99_region_closure(kiln_value *out, kiln_closure **site, kiln_body_fn body,
                                   const kiln_string *name, const char *source, size_t source_len,
                                   const char *diagnostic_source, uint32_t proto, uint32_t arity,
                                   int generator, int asynchronous, int async_generator,
                                   int is_method, int arrow, int strict, kiln_val receiver,
                                   uint32_t local_count, uint32_t stack_size,
                                   uint32_t handler_depth, uint32_t prologue_resume,
                                   int publishes_frame) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    int mode = kiln_c99_region_mode(heap);
    int status;
    if (mode == KILN_C99_REGION_MODE_USER) {
        kiln_closure *shared = __atomic_load_n(site, __ATOMIC_ACQUIRE);
        if (shared != NULL) {
            *out = kiln_c99_function(shared);
            return KILN_C99_OK;
        }
    } else if (mode == KILN_C99_REGION_MODE_BUILDER && arrow != 1 &&
               receiver == KILN_VAL_UNDEFINED && __atomic_load_n(site, __ATOMIC_ACQUIRE) == NULL) {
        kiln_closure *closure = (kiln_closure *)kiln_c99_region_alloc(KILN_GC_CLOSURE, sizeof *closure);
        if (closure != NULL) {
            closure->body = body;
            closure->generator = generator;
            closure->asynchronous = asynchronous;
            closure->local_count = local_count;
            closure->stack_size = stack_size;
            closure->handler_depth = handler_depth;
            closure->arrow = arrow != 0;
            closure->strict = strict;
            closure->publishes_frame = publishes_frame;
            closure->region = 1;
            kiln_c99_heap_store_val(closure, &closure->captured_this, KILN_VAL_UNDEFINED);
            closure->async_generator = async_generator;
            closure->is_method = is_method;
            kiln_c99_heap_store_val(closure, &closure->captured_new_target, KILN_VAL_UNDEFINED);
            kiln_c99_heap_store_value(closure, &closure->captured_arguments, kiln_c99_undefined());
            kiln_c99_heap_store_value(closure, &closure->own_prototype, kiln_c99_undefined());
            kiln_c99_heap_store_value(closure, &closure->own_store, kiln_c99_undefined());
            kiln_c99_heap_store_pointer(closure, &closure->name, name);
            kiln_c99_heap_store_pointer(closure, &closure->source, source);
            closure->source_len = source_len;
            kiln_c99_heap_store_value(closure, &closure->source_value, kiln_c99_undefined());
            kiln_c99_heap_store_pointer(closure, &closure->diagnostic_source, diagnostic_source);
            closure->arity = arity;
            closure->prologue_resume = prologue_resume;
            closure->proto = proto;
            closure->upvalue_count = 0;
            closure->upvalues = NULL;
            __atomic_store_n(site, closure, __ATOMIC_RELEASE);
            *out = kiln_c99_function(closure);
            return KILN_C99_OK;
        }
    }
    status = kiln_c99_make_closure(out, body, name, source, source_len, diagnostic_source, proto,
                                   arity, generator, asynchronous, async_generator, is_method,
                                   arrow, strict, receiver,
                                   kiln_val_from_boundary(kiln_c99_new_target), local_count,
                                   stack_size, handler_depth, prologue_resume, NULL, 0);
    if (status == KILN_C99_OK && !publishes_frame) kiln_c99_closure_of(*out)->publishes_frame = 0;
    return status;
}

/* Stack-tail operations are outlined as operations, not as wrappers around operations. Compact
 * generated bodies otherwise repeat the same conversion, validation, and diagnostic plumbing at
 * every site, even though only the stack cursor and diagnostic coordinates vary. */
static int kiln_c99_tail_define_accessor(kiln_val *stack, size_t *sp, int setter_half,
                                          int enumerable, const char *site, size_t pc,
                                          const char *operation, size_t diagnostic_depth) {
    size_t cursor = *sp;
    kiln_value half = kiln_val_to_boundary(stack[--cursor]);
    kiln_value key = kiln_val_to_boundary(stack[--cursor]);
    kiln_value other = kiln_c99_undefined();
    kiln_value name = kiln_c99_property_key(key);
    kiln_value getter = setter_half ? other : half;
    kiln_value setter = setter_half ? half : other;
    int status;
    *sp = cursor;
    if (name.tag != KILN_VALUE_STRING) {
        KILN_C99_REFUSE_AT(site, pc, operation, diagnostic_depth);
    }
    status = kiln_c99_set_function_name(half, key, setter_half ? "set" : "get");
    if (status != KILN_C99_OK) return status;
    /* The headed form: the byte adapter would read the header's words as key bytes. */
    return kiln_c99_define_accessor_key(kiln_val_to_boundary(stack[cursor - 1]),
                                        (const kiln_string *)name.ptr,
                                        getter, setter, enumerable);
}

static int kiln_c99_tail_define_method(kiln_val *stack, size_t *sp, const char *site,
                                        size_t pc, const char *operation,
                                        size_t diagnostic_depth) {
    size_t cursor = *sp;
    kiln_value method = kiln_val_to_boundary(stack[--cursor]);
    kiln_value key = kiln_val_to_boundary(stack[--cursor]);
    kiln_value name = kiln_c99_property_key(key);
    *sp = cursor;
    if (name.tag != KILN_VALUE_STRING ||
        kiln_c99_set_function_name(method, key, NULL) != KILN_C99_OK ||
        kiln_c99_define_method_key(kiln_val_to_boundary(stack[cursor - 1]),
                                   (const kiln_string *)name.ptr,
                                   method) != KILN_C99_OK) {
        KILN_C99_REFUSE_AT(site, pc, operation, diagnostic_depth);
    }
    return KILN_C99_OK;
}

static int kiln_c99_tail_apply(kiln_host *host, kiln_val *stack, size_t *sp, int depth,
                                kiln_value *thrown, const char *site, size_t pc,
                                const char *operation, size_t diagnostic_depth) {
    size_t cursor = *sp;
    kiln_value list = kiln_val_to_boundary(stack[--cursor]);
    kiln_val callee = stack[--cursor];
    kiln_val self_value = stack[cursor - 1];
    kiln_val call_result = KILN_VAL_UNDEFINED;
    kiln_array *given;
    int status;
    *sp = cursor;
    if (list.tag != KILN_VALUE_ARRAY) {
        KILN_C99_REFUSE_AT(site, pc, operation, diagnostic_depth);
    }
    given = kiln_c99_array_of(list);
    status = kiln_c99_call_val(host, &call_result, callee, self_value,
                               given->count == 0 ? NULL : given->elements,
                               given->count, depth, thrown);
    if (status == KILN_C99_OK) stack[cursor - 1] = call_result;
    return status;
}

/* Recursion runs on the C stack here, so depth is carried explicitly and checked before the
 * frame is entered rather than discovered by crashing. */
/* The receiver is `this` inside the body.  A plain call passes undefined, which is what `this`
 * is in a module: modules are strict, and strict mode does not substitute the global object. */
/* Establish `new.target` for a body that is about to run, and hand back what to restore.
 *
 * An arrow has no `new.target` of its own: it sees the one from where it was written. An ordinary
 * function gets `undefined`, because reaching a body through a call is exactly the case where no
 * constructor was named. `kiln_c99_call` does this inline and correctly, but a generator body is
 * not entered by that path -- calling a generator only *builds* it, and the body runs once per
 * resumption, arbitrarily later. So an async arrow read whatever the global happened to hold and
 * answered `new.target === undefined` where the interpreter and node both say the captured one.
 * Re-establishing it per entry rather than per call is what makes the two engines agree. */
/* One body-entry boundary for every closure shape.
 *
 * Ordinary calls, parameter instantiation, generators and async resumptions used to establish
 * overlapping subsets of this dynamic frame state before calling `closure->body` directly. That
 * made the newest path the easiest place to forget `arguments`, lexical `this`, `new.target`, or
 * diagnostic depth. Keep the mutable process globals behind one save/restore scope instead. */
static int kiln_c99_invoke_closure_body(kiln_host *host, kiln_val *result,
                                        kiln_closure *closure, kiln_val receiver,
                                        const kiln_val *args, size_t argc, int depth,
                                        kiln_value *thrown, kiln_generator *frame,
                                        int inherit_new_target) {
    kiln_value saved_target = kiln_c99_new_target;
    const kiln_val *saved_args = kiln_c99_frame_args;
    size_t saved_argc = kiln_c99_frame_argc;
    kiln_value saved_captured = kiln_c99_captured_arguments;
    kiln_value saved_arguments_object = kiln_c99_frame_arguments_object;
    int saved_is_arrow = kiln_c99_frame_is_arrow;
    int saved_mapped_arguments = kiln_c99_frame_mapped_arguments;
    kiln_value saved_callee = kiln_c99_frame_callee;
    int saved_diagnostic_depth = kiln_c99_diagnostic_active_depth;
    int status;
    if (!inherit_new_target) {
        kiln_c99_new_target =
            closure->arrow ? kiln_val_to_boundary(closure->captured_new_target) : kiln_c99_undefined();
    }
    kiln_c99_frame_is_arrow = closure->arrow;
    kiln_c99_frame_arguments_object = kiln_c99_undefined();
    /* The body's prologue overwrites the kind with its own compiled constant; this is only what
     * a body that never asks leaves behind. */
    kiln_c99_frame_callee = kiln_c99_function(closure);
    if (closure->arrow) {
        kiln_c99_captured_arguments = closure->captured_arguments;
    } else {
        kiln_c99_frame_args = args;
        kiln_c99_frame_argc = argc;
    }
    kiln_c99_diagnostic_active_depth = depth + 1;
    kiln_call_context ctx;
    ctx.host = host;
    ctx.thrown = thrown;
    ctx.frame = frame;
    ctx.depth = depth + 1;
    status = closure->body(&ctx, result, closure->upvalues, &receiver, args, argc);
    if ((status == KILN_C99_CONTROL_ERROR || status == KILN_C99_THROWN) &&
        kiln_c99_diagnostic_site == NULL) {
        kiln_c99_diagnostic_site = closure->diagnostic_source;
        kiln_c99_diagnostic_operation = "FunctionBody";
        kiln_c99_diagnostic_function = NULL;
        kiln_c99_diagnostic_function_len = 0;
        kiln_c99_diagnostic_function_key = closure->name;
        kiln_c99_diagnostic_depth = (size_t)(depth + 1);
        kiln_c99_diagnostic_receiver_tag = KILN_VALUE_FUNCTION;
        kiln_c99_diagnostic_detail =
            "generated function body refused before an instruction owned the failure";
    }
    /* Closures are the actual cross-module seam: an imported function pointer enters the C
     * translation unit that emitted its body without going through that unit's module wrapper.
     * Its diagnostic globals are private there, so preserve the owned refusal in the existing
     * call-result channel before returning to a caller whose globals describe another module. */
    if (status == KILN_C99_CONTROL_ERROR) {
        *result = kiln_val_from_boundary(kiln_c99_control_diagnostic());
    }
    kiln_c99_new_target = saved_target;
    kiln_c99_frame_args = saved_args;
    kiln_c99_frame_argc = saved_argc;
    kiln_c99_captured_arguments = saved_captured;
    kiln_c99_frame_arguments_object = saved_arguments_object;
    kiln_c99_frame_is_arrow = saved_is_arrow;
    kiln_c99_frame_mapped_arguments = saved_mapped_arguments;
    kiln_c99_frame_callee = saved_callee;
    kiln_c99_diagnostic_active_depth = saved_diagnostic_depth;
    return status;
}

/* Attribute a refusal that reached the caller without an instruction claiming it. Out of line
 * because it runs only on the refusal path, which is what keeps `kiln_c99_invoke_closure` small
 * enough for the compiler to inline. */
static void kiln_c99_attribute_body_refusal(const kiln_closure *closure, int depth) {
    kiln_c99_diagnostic_site = closure->diagnostic_source;
    kiln_c99_diagnostic_operation = "FunctionBody";
    if (kiln_c99_diagnostic_function == NULL && kiln_c99_diagnostic_function_key == NULL) {
        kiln_c99_diagnostic_function = NULL;
        kiln_c99_diagnostic_function_len = 0;
        kiln_c99_diagnostic_function_key = closure->name;
    }
    kiln_c99_diagnostic_depth = (size_t)(depth + 1);
    if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = KILN_VALUE_FUNCTION;
    if (kiln_c99_diagnostic_detail == NULL) {
        kiln_c99_diagnostic_detail =
            "generated function body refused before an instruction owned the failure";
    }
}

static void kiln_c99_attribute_body_function(const kiln_closure *closure) {
    if (kiln_c99_diagnostic_function == NULL && kiln_c99_diagnostic_function_key == NULL) {
        kiln_c99_diagnostic_function = NULL;
        kiln_c99_diagnostic_function_len = 0;
        kiln_c99_diagnostic_function_key = closure->name;
        /* A generated instruction may already have recorded the site in the realm before this
         * closure returns. Re-publish the completed cold record so the rendered control value
         * sees the function name too. */
        if (kiln_c99_diagnostic_site != NULL && kiln_c99_diagnostic_operation != NULL) {
            kiln_c99_publish_diagnostic();
        }
    }
}

#if defined(__GNUC__) || defined(__clang__)
__attribute__((always_inline))
#endif
static inline int kiln_c99_invoke_closure(kiln_host *host, kiln_val *result,
                                          kiln_closure *closure, kiln_val receiver,
                                          const kiln_val *args, size_t argc, int depth,
                                          kiln_value *thrown, kiln_generator *frame,
                                          int inherit_new_target) {
    if (KILN_UNLIKELY(kiln_c99_stack_exhausted())) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Maximum call stack size exceeded");
    }
    if (!closure->publishes_frame && !inherit_new_target) {
        int saved_depth = kiln_c99_diagnostic_active_depth;
        int quick;
        kiln_c99_diagnostic_active_depth = depth + 1;
        kiln_call_context ctx;
        ctx.host = host;
        ctx.thrown = thrown;
        ctx.frame = frame;
        ctx.depth = depth + 1;
        quick = closure->body(&ctx, result, closure->upvalues, &receiver, args, argc);
        if (quick == KILN_C99_CONTROL_ERROR) {
            if (kiln_c99_diagnostic_site == NULL) kiln_c99_attribute_body_refusal(closure, depth);
            else kiln_c99_attribute_body_function(closure);
        } else if (quick == KILN_C99_THROWN) {
            /* Ordinary JavaScript throws get their generic call detail from the caller. Only
             * add the owning function here; a FunctionBody fallback would suppress that detail. */
            kiln_c99_attribute_body_function(closure);
        }
        kiln_c99_diagnostic_active_depth = saved_depth;
        return quick;
    }
    return kiln_c99_invoke_closure_body(host, result, closure, receiver, args, argc, depth,
                                        thrown, frame, inherit_new_target);
}

/* Bind a generator's parameters, before the generator object reaches the program.
 *
 * The body runs in the generator's own frame, because the binding is compiled against the
 * callee's slots and the locals it writes are the ones the body will resume with. `binding` is
 * what stops it at the seam -- the emitted body is one flat instruction sequence, so nothing
 * else could -- and the resume position it leaves behind is what makes the first `next()` start
 * at the body rather than at the parameters a second time.
 *
 * The frame globals are established here and not at the resumption for the same reason `this`
 * is captured at the call: `...rest` is taken from the *caller's* argument list, and by the time
 * `next` runs those globals belong to whoever is stepping the generator. */
static int kiln_c99_bind_parameters(kiln_host *host, kiln_value generator, kiln_closure *closure,
                                    kiln_val receiver, const kiln_val *args, size_t argc,
                                    int depth, kiln_value *thrown) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    kiln_val produced = KILN_VAL_UNDEFINED;
    int status;
    frame->binding = 1;
    status = kiln_c99_invoke_closure_body(host, &produced, closure, receiver, args, argc, depth,
                                          thrown, frame, 0);
    frame->binding = 0;
    if (status != KILN_C99_YIELDED) {
        /* The seam is the only exit a prologue has: it holds no `Yield` and no `Return`, so
         * `OK` here would mean the guard was never reached and the whole body ran. A throw is
         * the expected other outcome and travels out of the *call*, which is the point. */
        return status == KILN_C99_OK ? kiln_c99_refuse("a generator prologue ran past "
                                                       "its parameter-binding seam") : status;
    }
    frame->started = 1;
    frame->resume = closure->prologue_resume;
    return KILN_C99_OK;
}

static int kiln_c99_dynamic_new_function(kiln_host *host, kiln_value *result,
                                         const kiln_value *args, size_t argc,
                                         int depth, kiln_value *thrown);

static int kiln_c99_call(kiln_host *host, kiln_value *result, kiln_value callee,
                         kiln_value receiver, const kiln_value *args, size_t argc, int depth,
                         kiln_value *thrown) {
    kiln_closure *closure;
    if (!kiln_c99_is_callable(callee)) {
        return kiln_c99_throw_not_a(thrown, callee, "function");
    }
    if (depth >= KILN_C99_MAX_CALL_DEPTH || kiln_c99_stack_exhausted()) {
        /* A `RangeError` the program can catch, which is what a real engine raises when the
         * stack runs out -- and what a recursive descent parser catches to report a nesting
         * limit rather than dying. */
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Maximum call stack size exceeded");
    }
    if (callee.tag == KILN_VALUE_COMMONJS_REQUIRE) {
        kiln_commonjs_require *require = (kiln_commonjs_require *)(uintptr_t)callee.bits;
        return require->call(require->context, host,
                             argc > 0 ? args[0] : kiln_c99_undefined(), result, thrown);
    }
    if (callee.tag == KILN_VALUE_PROXY) {
        /* Calling a proxy is the `apply` trap, which receives the arguments as one array --
         * that is what lets a handler inspect or rewrite them before forwarding. */
        kiln_value trap;
        kiln_value inner = kiln_c99_proxy_of(callee)->target;
        kiln_value handler = kiln_c99_proxy_of(callee)->handler;
        int status = kiln_c99_proxy_trap(host, callee, "apply", &trap, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (trap.tag == KILN_VALUE_UNDEFINED) {
            return kiln_c99_call(host, result, inner, receiver, args, argc, depth, thrown);
        }
        {
            kiln_value list;
            kiln_value arguments[3];
            size_t at;
            status = kiln_c99_new_array(&list);
            if (status != KILN_C99_OK) {
                return status;
            }
            for (at = 0; at < argc; ++at) {
                status = kiln_c99_array_push(list, args[at]);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            arguments[0] = inner;
            arguments[1] = receiver;
            arguments[2] = list;
            return kiln_c99_call(host, result, trap, handler, arguments, 3, depth, thrown);
        }
    }
    if (callee.tag == KILN_VALUE_BOUND) {
        /* Binding is transparent to the call: the receiver is the one that was fixed, whatever
         * the target turns out to be, and the fixed arguments come before the call's own. */
        kiln_bound *bound = (kiln_bound *)(uintptr_t)callee.bits;
        kiln_value *joined;
        int status;
        if (bound->count == 0) {
            return kiln_c99_call(host, result, bound->target, bound->receiver, args, argc, depth,
                                 thrown);
        }
        joined = (kiln_value *)malloc((bound->count + argc) * sizeof *joined);
        if (joined == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        memcpy(joined, bound->arguments, bound->count * sizeof *joined);
        if (argc != 0) {
            memcpy(joined + bound->count, args, argc * sizeof *joined);
        }
        status = kiln_c99_call(host, result, bound->target, bound->receiver, joined,
                               bound->count + argc, depth, thrown);
        free(joined);
        return status;
    }
    if (callee.tag == KILN_VALUE_SETTLER) {
        /* `resolve` and `reject` from an executor. Calling one settles its promise and evaluates
         * to undefined, however many times the program calls it. */
        kiln_settler *settler = (kiln_settler *)(uintptr_t)callee.bits;
        *result = kiln_c99_undefined();
        return kiln_c99_settle(settler->promise, argc > 0 ? args[0] : kiln_c99_undefined(),
                               settler->rejects);
    }
    if (callee.tag == KILN_VALUE_METHOD) {
        return kiln_c99_invoke_method(host, result, callee.bits, receiver, args, argc, depth,
                                      thrown);
    }
    if (callee.tag == KILN_VALUE_BUILTIN) {
        static const char empty[] = "";
        switch (callee.bits) {
        case KILN_C99_BUILTIN_STRING: {
            /* `String()` with no argument is the empty string, not `"undefined"`. */
            kiln_value subject;
            int status;
            if (argc == 0) {
                *result = kiln_c99_value(KILN_VALUE_STRING, 0, empty, 0);
                return KILN_C99_OK;
            }
            /* The *string* hint: `toString` before `valueOf`, which is the other way round from
             * every operator. `${x}` compiles to this call for exactly that reason -- a template
             * is not `"" + x`, and for an object with both methods the two disagree. */
            status = kiln_c99_to_primitive(host, args[0], 1, &subject, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_text(subject);
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_NUMBER: {
            kiln_value subject;
            kiln_value reduced;
            double converted;
            int status;
            /* `Number()` with no argument is zero, not NaN: there is nothing to fail to convert. */
            if (argc == 0) {
                *result = kiln_c99_number(0.0);
                return KILN_C99_OK;
            }
            subject = args[0];
            reduced = subject;
            if (kiln_c99_is_reference(subject)) {
                status = kiln_c99_to_primitive(host, subject, 0, &reduced, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            if (reduced.tag == KILN_VALUE_BIGINT) {
                *result = kiln_c99_number(kiln_c99_as_number(reduced));
                return KILN_C99_OK;
            }
            /* The *number* hint, the reverse of the `String` builtin above: `Number([6])` is 6
             * because an array declines `valueOf` and its `toString` answers "6". */
            status = kiln_c99_to_number_checked(host, reduced, &converted, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_number(converted);
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_BIGINT: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_number(0.0);
            void *handle = NULL;
            if (subject.tag == KILN_VALUE_BIGINT) {
                *result = subject;
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_STRING) {
                /* A string that is not an integer is a SyntaxError, unlike `Number("x")` which
                 * is NaN -- a BigInt has no NaN to be. */
                size_t subject_len = 0;
                const char *subject_bytes = kiln_c99_string_ascii_bytes(subject, &subject_len);
                if (subject_bytes == NULL || kiln_bigint_parse((const unsigned char *)subject_bytes,
                                      subject_len, 10,
                                      &handle) != 0) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                                "Cannot convert this string to a BigInt");
                }
                *result = kiln_c99_bigint_value(handle);
                return KILN_C99_OK;
            }
            /* Only an exact integer converts: `BigInt(1.5)` is a RangeError rather than a
             * rounding, because rounding is the thing this type exists to avoid. */
            if (kiln_bigint_from_double(kiln_c99_as_number(subject), &handle) != 0) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "The number is not a safe integer");
            }
            *result = kiln_c99_bigint_value(handle);
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_ARRAY_BUFFER: {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "ArrayBuffer constructor requires 'new'");
        }
        case KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER: {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Constructor SharedArrayBuffer requires 'new'");
        }
        case KILN_C99_BUILTIN_DATA_VIEW: {
            /* The interpreter's wording, word for word, which is not the wording of the
             * `ArrayBuffer` arm above. `error.message` is program-visible, so the two engines
             * saying this differently is a divergence; the older arm predates that being
             * checked and is left alone rather than changed from here. */
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Constructor DataView requires 'new'");
        }
        case KILN_C99_BUILTIN_INT8_ARRAY:
        case KILN_C99_BUILTIN_UINT8_ARRAY:
        case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY:
        case KILN_C99_BUILTIN_INT16_ARRAY:
        case KILN_C99_BUILTIN_UINT16_ARRAY:
        case KILN_C99_BUILTIN_INT32_ARRAY:
        case KILN_C99_BUILTIN_UINT32_ARRAY:
        case KILN_C99_BUILTIN_FLOAT32_ARRAY:
        case KILN_C99_BUILTIN_FLOAT64_ARRAY: {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "typed array constructor requires 'new'");
        }
        case KILN_C99_BUILTIN_SYMBOL:
            return kiln_c99_new_symbol(result, argc > 0 ? args[0] : kiln_c99_undefined());
        case KILN_C99_BUILTIN_REGEXP: {
            /* `RegExp(re)` keeps the original's source *and* its flags unless new ones are
             * given, which is the difference from `new RegExp(String(re))`. */
            kiln_value pattern = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value source;
            kiln_value flags;
            if (pattern.tag == KILN_VALUE_REGEXP) {
                kiln_regexp *existing = kiln_c99_regexp_of(pattern);
                source = kiln_c99_value(KILN_VALUE_STRING, 0,
                                        (const char *)existing->source,
                                        existing->source == NULL ? 0 : existing->source->unit_len);
                flags = argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED
                            ? kiln_c99_text(args[1])
                            : kiln_c99_value(KILN_VALUE_STRING, 0,
                                             (const char *)existing->flags,
                                             existing->flags == NULL ? 0 : existing->flags->unit_len);
            } else {
                source = argc > 0 ? kiln_c99_text(pattern) : kiln_c99_from_units(NULL, 0);
                flags = argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED
                            ? kiln_c99_text(args[1])
                            : kiln_c99_from_units(NULL, 0);
            }
            if (source.tag != KILN_VALUE_STRING || flags.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            return kiln_c99_new_regexp(result, (const kiln_string *)source.ptr,
                                       (const kiln_string *)flags.ptr, thrown);
        }
        case KILN_C99_BUILTIN_BOOLEAN:
            /* `Boolean(x)` is `!!x` spelled as a call, and `Boolean()` is false. */
            *result = kiln_c99_boolean(
                argc == 0 ? 0 : kiln_c99_truthy(args[0]));
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_AGGREGATE_ERROR:
            return kiln_c99_aggregate_error_from_args(host, result, args, argc, depth, thrown);
        /* The two constructors that do the same thing called as they do constructed. Everything
         * else in this enum either has no call behaviour at all (`Map()` without `new` is a
         * TypeError) or is a namespace, and both keep saying so. */
        case KILN_C99_BUILTIN_ARRAY:
            return kiln_c99_array_constructor(result, args, argc, thrown);
        case KILN_C99_BUILTIN_OBJECT:
            return kiln_c99_object_constructor(result, args, argc);
        case KILN_C99_BUILTIN_FUNCTION:
            return kiln_c99_dynamic_new_function(host, result, args, argc, depth, thrown);
        case KILN_C99_BUILTIN_ERROR:
        case KILN_C99_BUILTIN_TYPE_ERROR:
        case KILN_C99_BUILTIN_RANGE_ERROR:
        case KILN_C99_BUILTIN_REFERENCE_ERROR:
        case KILN_C99_BUILTIN_SYNTAX_ERROR:
        case KILN_C99_BUILTIN_URI_ERROR:
        case KILN_C99_BUILTIN_EVAL_ERROR: {
            int status = kiln_c99_construct(host, result, callee, args, argc, depth, thrown);
            if (status == KILN_C99_OK && receiver.tag == KILN_VALUE_OBJECT && argc > 0) {
                (void)kiln_c99_put(host, receiver, "message", 7, args[0], 0, NULL);
            }
            return status;
        }
        default: {
            /* Which built-in, by name. `f.call(x)` and `f.apply(x, a)` both arrive here through
             * `Function.prototype`, so the instruction says `CallMethod(call)` -- the borrow, not
             * the borrowed thing -- and three separate bundles reported the same string for three
             * unrelated receivers. The name is what tells them apart, and only this site has it. */
            const char *named = kiln_c99_builtin_name(callee.bits);
            if (named == NULL) {
                KILN_C99_REFUSE("a built-in with no call behaviour in the native backend");
            }
            KILN_C99_REFUSE_NAMED("a built-in with no call behaviour in the native backend",
                                  named, strlen(named));
        }
        }
    }
    closure = kiln_c99_closure_of(callee);
    if (closure->arrow) {
        receiver = kiln_val_to_boundary(closure->captured_this);
    } else if (!closure->strict &&
               (receiver.tag == KILN_VALUE_UNDEFINED || receiver.tag == KILN_VALUE_NULL)) {
        /* `OrdinaryCallBindThis`: sloppy code never sees `undefined` or `null` as `this`. The
         * specification substitutes the global object for both, which is what a UMD header reads
         * when it writes `var root = this || global`.
         *
         * The specification also `ToObject`s a *primitive* receiver, and that half is not here:
         * this backend has no wrapper objects to make one out of, so `f.call(7)` still sees 7.
         * That is a gap this rule shares with every other primitive-wrapper site in the C
         * runtime, not a decision taken here -- and it is unreachable from the substitution
         * above, which is the case a real program actually depends on. Constructing does not
         * reach this at all: `new C()` enters the body through `kiln_c99_construct` with the new
         * instance as its receiver, which is already an object. */
        int global_status = kiln_c99_global_object(&receiver);
        if (global_status != KILN_C99_OK) {
            return global_status;
        }
    }
    if (closure->generator) {
        /* Calling a generator function does not run it: it builds the object that will. An
         * async function is compiled as one -- `await` suspends exactly as `yield` does -- and
         * differs only in who drives it, which is this rather than the program. */
        /* The call's arguments as frame values, for whichever of the two entries below runs the
         * prologue: a generator binds its parameters here, an async function binds them inside
         * its first step. Both build `...rest` and `arguments` from this list, so both need it. */
        kiln_val vargs[argc > 0 ? argc : 1];
        kiln_value built;
        int status;
        for (size_t i = 0; i < argc; ++i) vargs[i] = kiln_val_from_boundary(args[i]);
        status = kiln_c99_new_generator(&built, callee, receiver, args, argc);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (closure->async_generator) {
            kiln_c99_generator_of(built)->async_generator = 1;
        }
        /* Parameter binding belongs to the call, not to the first resumption -- so it runs
         * before the object is handed over, and its throw leaves from here. */
        if (closure->prologue_resume != 0) {
            kiln_val vrec = kiln_val_from_boundary(receiver);
            status =
                kiln_c99_bind_parameters(host, built, closure, vrec, argc > 0 ? vargs : NULL, argc, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        if (!closure->asynchronous || closure->async_generator) {
            /* An async generator is handed over too: it awaits *and* yields, so nothing can drive
             * it to completion -- each `next` drives one step and gets a promise of its own. */
            *result = built;
            return KILN_C99_OK;
        }
        {
            kiln_generator *frame = kiln_c99_generator_of(built);
            frame->is_async = 1;
            kiln_value frame_promise;
            status = kiln_c99_new_promise(&frame_promise);
            if (status == KILN_C99_OK)
                kiln_c99_heap_store_value(frame, &frame->promise, frame_promise);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = frame->promise;
            /* The body starts *now*, synchronously, and runs until its first `await` -- which is
             * what makes the code before the first one behave like an ordinary call. It carries
             * the call's arguments because this entry is also the one that runs the prologue:
             * the caller's list is still alive here, and every later resumption is behind it. */
            return kiln_c99_async_step(host, built, kiln_c99_undefined(), 0, depth, thrown,
                                       argc > 0 ? vargs : NULL, argc);
        }
    }
    {
        /* `new.target` is the constructor this call named -- nothing, for an ordinary call, and an
         * arrow's captured one, because an arrow has none of its own. `super(...)` is the one call
         * that keeps what it found: the base constructor is being run *for* the subclass. */
        int inherit_new_target = kiln_c99_inheriting_new_target;
        if (kiln_c99_inheriting_new_target) {
            kiln_c99_inheriting_new_target = 0;
        }
        kiln_val vrec = kiln_val_from_boundary(receiver);
        kiln_val vargs[argc > 0 ? argc : 1];
        for (size_t i = 0; i < argc; ++i) vargs[i] = kiln_val_from_boundary(args[i]);
        kiln_val vres = KILN_VAL_UNDEFINED;
        int status = kiln_c99_invoke_closure_body(host, &vres, closure, vrec, argc > 0 ? vargs : NULL, argc, depth,
                                                  thrown, NULL, inherit_new_target);
        *result = kiln_val_to_boundary(vres);
        return status;
    }
}
