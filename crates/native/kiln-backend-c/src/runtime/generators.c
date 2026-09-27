
static kiln_generator *kiln_c99_generator_of(kiln_value value) {
    return (kiln_generator *)(uintptr_t)value.bits;
}

/* The frame a generator will run in, filled with its arguments and nothing else.
 *
 * Both arrays are sized from the body's own counts, so a suspension has somewhere to put every
 * local and every operand the verifier proved it can hold. */
static int kiln_c99_new_generator(kiln_value *out, kiln_value function, kiln_value receiver,
                                  const kiln_value *args, size_t argc) {
    kiln_closure *closure = kiln_c99_closure_of(function);
    size_t locals = closure->local_count == 0 ? 1 : closure->local_count;
    size_t depth = closure->stack_size == 0 ? 1 : closure->stack_size;
    kiln_generator *frame;
    size_t index;
    if (locals > (((size_t)-1) - sizeof *frame) / sizeof(kiln_val) - depth) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    /* The locals and the operand stack live in the frame's own block, after the header: a
     * suspended async function is one allocation, found and freed with its frame. They used to
     * be four more `malloc`s -- two of them handler arrays sized by the body's instruction count
     * that no emitted body reads, since the graph emitter tracks handlers in `handler_top` -- and
     * a hundred thousand pending tasks paid about 600 bytes each for them. */
    frame = (kiln_generator *)kiln_c99_gc_alloc(
        KILN_GC_GENERATOR, sizeof *frame + (locals + depth) * sizeof(kiln_val));
    if (frame == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    frame->locals = (kiln_val *)(frame + 1);
    frame->stack = frame->locals + locals;
    frame->handlers = NULL;
    frame->handler_stack = NULL;
    frame->handler_top = 0;
    for (index = 0; index < locals; ++index) {
        frame->locals[index] = KILN_VAL_UNDEFINED;
    }
    for (index = 0; index < argc && index < locals; ++index) {
        frame->locals[index] = kiln_val_from_boundary(args[index]);
    }
    frame->function = function;
    frame->receiver = receiver;
    frame->sp = 0;
    frame->resume = 0;
    frame->sent = kiln_c99_undefined();
    frame->awaiting = 0;
    frame->async_generator = 0;
    frame->raising = 0;
    frame->binding = 0;
    frame->promise = kiln_c99_undefined();
    frame->is_async = 0;
    frame->started = 0;
    frame->done = 0;
    frame->running = 0;
    frame->queue_promises = NULL;
    frame->queue_sent = NULL;
    frame->queue_count = 0;
    frame->queue_capacity = 0;
    frame->queue_executing = 0;
    frame->delegate = kiln_c99_undefined();
    frame->delegate_exit = 0;
    *out = kiln_c99_value(KILN_VALUE_GENERATOR, (uint64_t)(uintptr_t)frame, NULL, 0);
    return KILN_C99_OK;
}
