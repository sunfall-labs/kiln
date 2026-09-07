
/* Generated code owns each work payload because only it understands `kiln_value`. The Realm
 * Runtime owns queueing, ordering, virtual time, cancellation and disposal timing through opaque
 * pointers; no language value layout crosses the private ABI. */
static int kiln_c99_enqueue(kiln_value handler, kiln_value argument, kiln_value derived,
                            int rejected, int continuation) {
    kiln_microtask *task;
    int status;
    task = (kiln_microtask *)kiln_c99_gc_alloc(KILN_GC_MICROTASK, sizeof *task);
    if (task == NULL) return KILN_C99_OUT_OF_MEMORY;
    task->handler = handler;
    task->argument = argument;
    task->derived = derived;
    task->rejected = rejected;
    task->continuation = continuation;
    status = kiln_realm_queue_microtask(kiln_c99_run_microtask, kiln_c99_drop_microtask,
                                        task);
    if (status != KILN_C99_OK) {
        kiln_c99_drop_microtask(task);
        return status;
    }
    return KILN_C99_OK;
}

static int kiln_c99_enqueue_next_tick(kiln_value callback, const kiln_value *args, size_t argc) {
    kiln_next_tick *task =
        (kiln_next_tick *)kiln_c99_gc_alloc(KILN_GC_NEXT_TICK, sizeof *task);
    int status;
    if (task == NULL) return KILN_C99_OUT_OF_MEMORY;
    task->callback = callback;
    task->argc = argc;
    task->args = NULL;
    if (argc != 0) {
        if (argc > SIZE_MAX / sizeof *task->args) {
            kiln_c99_gc_release(task);
            return KILN_C99_OUT_OF_MEMORY;
        }
        task->args = (kiln_value *)malloc(argc * sizeof *task->args);
        if (task->args == NULL) {
            kiln_c99_gc_release(task);
            return KILN_C99_OUT_OF_MEMORY;
        }
        memcpy(task->args, args, argc * sizeof *task->args);
    }
    status = kiln_realm_queue_next_tick(kiln_c99_run_next_tick, kiln_c99_drop_next_tick, task);
    if (status != KILN_C99_OK) {
        kiln_c99_drop_next_tick(task);
        return status;
    }
    return KILN_C99_OK;
}

static int kiln_c99_new_promise(kiln_value *out) {
    kiln_promise *promise = (kiln_promise *)kiln_c99_gc_alloc(KILN_GC_PROMISE, sizeof *promise);
    if (promise == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    promise->state = KILN_C99_PENDING;
    promise->value = kiln_c99_undefined();
    promise->reactions = NULL;
    promise->count = 0;
    promise->capacity = 0;
    promise->properties = kiln_c99_undefined();
    *out = kiln_c99_value(KILN_VALUE_PROMISE, (uint64_t)(uintptr_t)promise, NULL, 0);
    return KILN_C99_OK;
}

static kiln_promise *kiln_c99_promise_of(kiln_value value) {
    return (kiln_promise *)(uintptr_t)value.bits;
}

static int kiln_c99_bind_with(kiln_value *out, kiln_value target, kiln_value receiver,
                              const kiln_value *arguments, size_t count) {
    kiln_bound *bound = (kiln_bound *)kiln_c99_gc_alloc(KILN_GC_BOUND, sizeof *bound);
    if (bound == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    bound->target = target;
    bound->receiver = receiver;
    bound->arguments = NULL;
    bound->count = count;
    bound->properties = kiln_c99_undefined();
    if (count != 0) {
        /* The argument array is the bound function's alone, so it stays malloc-backed and the
         * KILN_GC_BOUND finaliser is the one place it is released. */
        bound->arguments = (kiln_value *)malloc(count * sizeof *bound->arguments);
        if (bound->arguments == NULL) {
            kiln_c99_gc_release(bound);
            return KILN_C99_OUT_OF_MEMORY;
        }
        memcpy(bound->arguments, arguments, count * sizeof *bound->arguments);
    }
    *out = kiln_c99_value(KILN_VALUE_BOUND, (uint64_t)(uintptr_t)bound, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_bind(kiln_value *out, kiln_value target, kiln_value receiver) {
    return kiln_c99_bind_with(out, target, receiver, NULL, 0);
}

static int kiln_c99_settle(kiln_value promise, kiln_value value, int rejected);

/* Queue one reaction against a promise that has already settled. */
static int kiln_c99_schedule(kiln_promise *promise, const kiln_reaction *reaction) {
    int rejected = promise->state == KILN_C99_REJECTED;
    return kiln_c99_enqueue(rejected ? reaction->on_rejected : reaction->on_fulfilled,
                            promise->value, reaction->derived, rejected, reaction->continuation);
}

/* Settle a promise, queueing everything that was waiting for it.
 *
 * Fulfilling with another promise *adopts* it rather than nesting: the value a promise arrives
 * at is never itself a promise, which is what makes a `then` chain flat. */
static int kiln_c99_settle(kiln_value promise, kiln_value value, int rejected) {
    kiln_promise *state = kiln_c99_promise_of(promise);
    size_t index;
    int status;
    if (state->state != KILN_C99_PENDING) {
        return KILN_C99_OK;
    }
    if (!rejected && value.tag == KILN_VALUE_PROMISE) {
        /* Resolving with a promise does not settle this one: it *follows* the other. Attaching to
         * it is a job in its own right, so a chain that returns a promise from a handler takes one
         * more turn than one that returns a plain value -- which is a difference a program sees in
         * what order things print, and therefore one both engines have to make. */
        kiln_value step;
        status = kiln_c99_bind(&step, kiln_c99_method(KILN_C99_METHOD_ADOPT_STEP), promise);
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_enqueue(step, value, kiln_c99_undefined(), 0, 0);
    }
    state->state = rejected ? KILN_C99_REJECTED : KILN_C99_FULFILLED;
    state->value = value;
    for (index = 0; index < state->count; ++index) {
        status = kiln_c99_schedule(state, &state->reactions[index]);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    state->count = 0;
    return KILN_C99_OK;
}

static int kiln_c99_new_settler(kiln_value *out, kiln_value promise, int rejects) {
    kiln_settler *settler = (kiln_settler *)kiln_c99_gc_alloc(KILN_GC_SETTLER, sizeof *settler);
    if (settler == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    settler->promise = promise;
    settler->rejects = rejects;
    *out = kiln_c99_value(KILN_VALUE_SETTLER, (uint64_t)(uintptr_t)settler, NULL, 0);
    return KILN_C99_OK;
}

/* `p.then(f, r)`: the derived promise settles with what the handler produces, or with what `p`
 * produced when there is no handler for that side -- which is what makes a rejection travel down
 * a chain of `then`s until something catches it. */
static int kiln_c99_then(kiln_value promise, kiln_value on_fulfilled, kiln_value on_rejected,
                         kiln_value *out) {
    kiln_promise *state = kiln_c99_promise_of(promise);
    kiln_reaction reaction;
    int status = kiln_c99_new_promise(&reaction.derived);
    if (status != KILN_C99_OK) {
        return status;
    }
    reaction.on_fulfilled = on_fulfilled;
    reaction.on_rejected = on_rejected;
    reaction.continuation = 0;
    if (state->properties.tag == KILN_VALUE_OBJECT) {
        kiln_value proto = kiln_c99_object_of(state->properties)->prototype;
        if (proto.tag != KILN_VALUE_UNDEFINED && proto.tag != KILN_VALUE_NULL) {
            (void)kiln_c99_set_prototype(reaction.derived, proto);
        }
    }
    *out = reaction.derived;
    if (state->state != KILN_C99_PENDING) {
        return kiln_c99_schedule(state, &reaction);
    }
    status = kiln_c99_reserve((void **)&state->reactions, &state->capacity, state->count + 1,
                              sizeof *state->reactions);
    if (status != KILN_C99_OK) {
        return status;
    }
    state->reactions[state->count] = reaction;
    state->count += 1;
    return KILN_C99_OK;
}

/* `await p`: the suspended frame is the reaction, on both sides.
 *
 * The general `then` binds `@@resume` and `@@throw` to the frame -- two closures -- and makes a
 * derived promise nobody can reach; an await needs none of the three, only to be stepped with
 * what arrived and which side it arrived on. Four allocations an await, and a bound call through
 * the method dispatch on every resumption, become one microtask and a direct step. */
static int kiln_c99_then_continue(kiln_value promise, kiln_value generator) {
    kiln_promise *state = kiln_c99_promise_of(promise);
    kiln_reaction reaction;
    int status;
    reaction.on_fulfilled = generator;
    reaction.on_rejected = generator;
    reaction.derived = kiln_c99_undefined();
    reaction.continuation = 1;
    if (state->state != KILN_C99_PENDING) {
        return kiln_c99_schedule(state, &reaction);
    }
    status = kiln_c99_reserve((void **)&state->reactions, &state->capacity, state->count + 1,
                              sizeof *state->reactions);
    if (status != KILN_C99_OK) {
        return status;
    }
    state->reactions[state->count] = reaction;
    state->count += 1;
    return KILN_C99_OK;
}

/* A job that has run, or one dropped with its queue, is a cell in a nursery chunk that the next
 * collection settles with everything else in the chunk; releasing it by hand was a lookup, a
 * finaliser call and a free-list push per job, a tenth of an await loop, for a slot the chunk
 * reset reclaims wholesale. */
static void kiln_c99_drop_microtask(void *work) {
    (void)work;
}

/* Give a refusal inside a turn somewhere to say what it was.
 *
 * A module body hands its diagnostic back through the entry point's result, and
 * `kiln_c99_invoke_closure_body` composes one into the *call result* of any generated body that
 * refuses -- so by the time a refusal reaches one of the work callbacks below, the provenance
 * usually exists and is sitting in a value about to be dropped on the floor. Every one of them
 * discarded it: their only shared channel back to the embedding is the thrown slot the drive was
 * handed, and none of them wrote to it. A refusal from the turn loop therefore arrived carrying
 * nothing, and could be reported only as the phase it happened in -- which is what a bundled
 * `rxjs` prints, and a phase is not a name.
 *
 * The program is over once this runs: a refusal is not an exception, nothing downstream will read
 * the slot as one, and overwriting it loses nothing. The callee's own composition wins where it
 * made one, because a closure body's is the deepest and so the most specific; this unit's
 * diagnostic statics answer otherwise. */
static void kiln_c99_publish_turn_refusal(int status, kiln_value produced, kiln_value *thrown) {
    if (status != KILN_C99_CONTROL_ERROR || thrown == NULL) {
        return;
    }
    *thrown = produced.tag == KILN_VALUE_STRING && produced.len != 0
                  ? produced
                  : kiln_c99_control_diagnostic();
}

static int kiln_c99_run_microtask(void *work, void *host_pointer, void *thrown_pointer) {
    kiln_microtask *task = (kiln_microtask *)work;
    kiln_host *host = (kiln_host *)host_pointer;
    kiln_value *thrown = (kiln_value *)thrown_pointer;
    kiln_value produced = kiln_c99_undefined();
    int status;
    if (task == NULL || thrown == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (task->continuation) {
        /* An await's value arriving at its frame: step it where `@@resume` bound to it used to. */
        status = kiln_c99_generator_of(task->handler)->async_generator
                     ? kiln_c99_async_generator_step(host, task->handler, task->argument,
                                                     task->rejected, 0, thrown)
                     : kiln_c99_async_step(host, task->handler, task->argument, task->rejected,
                                           0, thrown, NULL, 0);
        if (status == KILN_C99_THROWN) return KILN_C99_THROWN;
        kiln_c99_publish_turn_refusal(status, kiln_c99_undefined(), thrown);
        return status;
    }
    if (!kiln_c99_is_callable(task->handler)) {
        /* No handler for this side, so the value travels on unchanged. */
        if (task->derived.tag == KILN_VALUE_PROMISE) {
            status = kiln_c99_settle(task->derived, task->argument, task->rejected);
            kiln_c99_publish_turn_refusal(status, kiln_c99_undefined(), thrown);
            return status;
        }
        return KILN_C99_OK;
    }
    status = kiln_c99_call(host, &produced, task->handler, kiln_c99_undefined(),
                           &task->argument, 1, 0, thrown);
    if (status == KILN_C99_THROWN) {
        /* A handler that throws rejects the promise it was producing, rather than failing the
         * program: that is the whole reason `catch` exists further down the chain. */
        if (task->derived.tag == KILN_VALUE_PROMISE) {
            status = kiln_c99_settle(task->derived, *thrown, 1);
            kiln_c99_publish_turn_refusal(status, kiln_c99_undefined(), thrown);
            return status;
        }
        return KILN_C99_THROWN;
    }
    if (status != KILN_C99_OK) {
        kiln_c99_publish_turn_refusal(status, produced, thrown);
        return status;
    }
    if (task->derived.tag == KILN_VALUE_PROMISE) {
        status = kiln_c99_settle(task->derived, produced, 0);
        kiln_c99_publish_turn_refusal(status, kiln_c99_undefined(), thrown);
        return status;
    }
    return KILN_C99_OK;
}

/* The finaliser frees `args`, so the release below must not free it a second time. */
static void kiln_c99_drop_next_tick(void *work) {
    kiln_c99_gc_release(work);
}

static int kiln_c99_run_next_tick(void *work, void *host_pointer, void *thrown_pointer) {
    kiln_next_tick *task = (kiln_next_tick *)work;
    kiln_host *host = (kiln_host *)host_pointer;
    kiln_value *thrown = (kiln_value *)thrown_pointer;
    kiln_value produced = kiln_c99_undefined();
    int status;
    if (task == NULL || thrown == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_call(host, &produced, task->callback, kiln_c99_undefined(),
                           task->argc == 0 ? NULL : task->args, task->argc, 0, thrown);
    kiln_c99_publish_turn_refusal(status, produced, thrown);
    return status;
}

/* The finaliser frees `args`, so the release below must not free it a second time. */
static void kiln_c99_drop_timer(void *work) {
    kiln_c99_gc_release(work);
}

static int kiln_c99_run_timer(void *work, void *host_pointer, void *thrown_pointer) {
    kiln_timer *timer = (kiln_timer *)work;
    kiln_host *host = (kiln_host *)host_pointer;
    kiln_value *thrown = (kiln_value *)thrown_pointer;
    kiln_value produced = kiln_c99_undefined();
    int status;
    if (timer == NULL || thrown == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_call(host, &produced, timer->callback, kiln_c99_undefined(),
                           timer->argc == 0 ? NULL : timer->args, timer->argc, 0, thrown);
    kiln_c99_publish_turn_refusal(status, produced, thrown);
    return status;
}

/* One step of an async function: resume the body, and decide what to do with where it stopped.
 *
 * It yields when it awaits, so the value it hands back is what it is waiting on. Everything that
 * has to happen after arrival is attached to that as two handlers -- resume with the value, or
 * throw into the body -- and the machine goes back to the queue. That is the whole of `await`:
 * the suspension is the generator's, and the scheduling is the promise's. */
/* Settle one step's promise with `{ value, done }`, which is the shape the iteration protocol is
 * written against -- an async generator's `next` promises that object rather than the value. */
static int kiln_c99_async_generator_settle(kiln_host *host, kiln_value step, kiln_value value,
                                           int finished, int depth, kiln_value *thrown) {
    kiln_value shaped;
    int status = kiln_c99_new_object(&shaped);
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, shaped, "value", 5, value, depth, thrown);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, shaped, "done", 4, kiln_c99_boolean(finished), depth, thrown);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    return kiln_c99_settle(step, shaped, 0);
}

/* One step of an async generator: resume the body until it either awaits or finishes a step.
 *
 * The difference from an async *function* is the whole reason this exists. A function has one
 * promise and runs to completion; a generator has one promise per `next`, and an `await` between
 * two `yield`s must leave that promise pending -- the step is not finished, and the consumer has
 * not been told anything yet. `frame->awaiting` is what says which suspension happened. */
static int kiln_c99_async_generator_step(kiln_host *host, kiln_value generator, kiln_value sent,
                                         int rejected, int depth, kiln_value *thrown) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    kiln_value step = frame->promise;
    kiln_closure *closure;
    kiln_value produced;
    int status;
    if (frame->done) {
        return kiln_c99_async_generator_settle(host, step, kiln_c99_undefined(), 1, depth, thrown);
    }
    if (rejected && !frame->started) {
        frame->done = 1;
        return kiln_c99_settle(step, sent, 1);
    }
    frame->sent = sent;
    frame->raising = rejected;
    closure = kiln_c99_closure_of(frame->function);
    kiln_val vproduced = KILN_VAL_UNDEFINED;
    status = kiln_c99_invoke_closure_body(host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0,
                                          depth, thrown, frame, 0);
    produced = kiln_val_to_boundary(vproduced);
    if (status == KILN_C99_YIELDED) {
        frame->started = 1;
        if (frame->awaiting) {
            /* Still inside the step: wait for what was awaited and come back here. A plain value
             * is one job away, as a promise already fulfilled with it would be. */
            if (produced.tag != KILN_VALUE_PROMISE) {
                return kiln_c99_enqueue(generator, produced, kiln_c99_undefined(), 0, 1);
            }
            return kiln_c99_then_continue(produced, generator);
        }
        status = kiln_c99_async_generator_settle(host, step, produced, 0, depth, thrown);
        frame->queue_executing = 0;
        if (frame->queue_count > 0) {
            kiln_value next_p = frame->queue_promises[0];
            kiln_value next_s = frame->queue_sent[0];
            size_t idx;
            frame->queue_count--;
            for (idx = 0; idx < frame->queue_count; ++idx) {
                frame->queue_promises[idx] = frame->queue_promises[idx + 1];
                frame->queue_sent[idx] = frame->queue_sent[idx + 1];
            }
            frame->queue_executing = 1;
            frame->promise = next_p;
            if (frame->done) {
                frame->queue_executing = 0;
                return kiln_c99_async_generator_settle(host, next_p, kiln_c99_undefined(), 1, depth, thrown);
            }
            return kiln_c99_async_generator_step(host, generator, next_s, 0, depth, thrown);
        }
        return status;
    }
    if (status == KILN_C99_THROWN) {
        frame->done = 1;
        frame->queue_executing = 0;
        status = kiln_c99_settle(step, *thrown, 1);
        while (frame->queue_count > 0) {
            kiln_value next_p = frame->queue_promises[0];
            size_t idx;
            frame->queue_count--;
            for (idx = 0; idx < frame->queue_count; ++idx) {
                frame->queue_promises[idx] = frame->queue_promises[idx + 1];
                frame->queue_sent[idx] = frame->queue_sent[idx + 1];
            }
            (void)kiln_c99_async_generator_settle(host, next_p, kiln_c99_undefined(), 1, depth, thrown);
        }
        return status;
    }
    if (status != KILN_C99_OK) {
        frame->queue_executing = 0;
        return status;
    }
    frame->done = 1;
    status = kiln_c99_async_generator_settle(host, step, produced, 1, depth, thrown);
    frame->queue_executing = 0;
    while (frame->queue_count > 0) {
        kiln_value next_p = frame->queue_promises[0];
        size_t idx;
        frame->queue_count--;
        for (idx = 0; idx < frame->queue_count; ++idx) {
            frame->queue_promises[idx] = frame->queue_promises[idx + 1];
            frame->queue_sent[idx] = frame->queue_sent[idx + 1];
        }
        (void)kiln_c99_async_generator_settle(host, next_p, kiln_c99_undefined(), 1, depth, thrown);
    }
    return status;
}

/* `args` and `argc` are the *call's* arguments, and only the entry that runs the prologue passes
 * them: an async function is the one suspending shape whose parameter binding is not run by
 * `kiln_c99_bind_parameters` at the call (`binds_at_call` excludes it, so its instantiation
 * failure rejects rather than throwing), so its prologue -- `...rest` and `arguments` among it --
 * runs here, in the first step. Resuming after an `await` passes none, because the prologue is
 * behind the resume point by then and the bindings are already in the frame's locals. */
static int kiln_c99_async_step(kiln_host *host, kiln_value generator, kiln_value sent,
                               int rejected, int depth, kiln_value *thrown,
                               const kiln_val *args, size_t argc) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    kiln_closure *closure;
    kiln_value produced;
    int status;
    if (frame->done) {
        return KILN_C99_OK;
    }
    if (rejected && !frame->started) {
        /* Nothing has run yet, so there is no suspension point to raise at and the only place the
         * rejection can go is the function's own promise. */
        frame->done = 1;
        return kiln_c99_settle(frame->promise, sent, 1);
    }
    frame->sent = sent;
    frame->raising = rejected;
    closure = kiln_c99_closure_of(frame->function);
    kiln_val vproduced = KILN_VAL_UNDEFINED;
    status = kiln_c99_invoke_closure_body(host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), args, argc,
                                          depth, thrown, frame, 0);
    produced = kiln_val_to_boundary(vproduced);
    if (status == KILN_C99_YIELDED) {
        frame->started = 1;
        if (produced.tag != KILN_VALUE_PROMISE) {
            /* Awaiting a plain value still yields to the queue: `await 1` is a suspension, and
             * the resumption is a job -- one job, as a promise already fulfilled with the value
             * would have queued. */
            return kiln_c99_enqueue(generator, produced, kiln_c99_undefined(), 0, 1);
        }
        return kiln_c99_then_continue(produced, generator);
    }
    frame->done = 1;
    if (status == KILN_C99_THROWN) {
        /* The body threw where nothing caught it, which rejects the promise rather than failing
         * the program: `async function f() { throw e }` is how a rejection is often written. */
        return kiln_c99_settle(frame->promise, *thrown, 1);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    return kiln_c99_settle(frame->promise, produced, 0);
}

/* `new f(...)`: build the instance, run the body with it as `this`, and yield the instance --
 * unless the body returned an object, which wins. That override is what makes a factory
 * constructor work. */
