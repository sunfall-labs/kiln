/* The promise job lane (Phase 6.3).
 *
 * Generated code owns each work payload because only it understands `kiln_value`. The Realm
 * Runtime owns queueing, ordering, virtual time, cancellation and disposal timing through opaque
 * pointers; no language value layout crosses the private ABI. A *promise* job crosses none of
 * that: it is a handler, an argument and a derived promise, it runs in the turn it was queued
 * in, it is never cancelled and it has no deadline. Paying a GC cell and a call across the
 * private ABI for each one bought nothing -- and an `await` loop queues one per iteration.
 *
 * So the jobs live in an array in the heap (`kiln_gc_heap.jobs`), the collector marks it, and
 * the realm drains it by calling `kiln_c99_run_promise_jobs` at the point in its own drain where
 * promise jobs belong: after `process.nextTick`, before the next host completion. The lanes the
 * runtime still owns -- next tick, timers, completions -- are unchanged, because ordering
 * between lanes is the realm's to decide and only the promise lane's *contents* moved. */
/* Room for one more job: when the ring is full, its capacity doubles and its elements are
 * linearized into the new buffer. The capacity is always a power of two. The ring never shrinks,
 * so a realm that once fanned out a hundred thousand jobs keeps that array until it dies -- a few
 * megabytes at the sizes we have measured, held against the burst happening again, and never a
 * leak that grows with time. */
static int kiln_c99_jobs_grow(kiln_gc_heap *heap) {
    size_t capacity = heap->job_capacity == 0 ? 64 : heap->job_capacity * 2;
    kiln_microtask *grown;
    size_t live = heap->job_tail - heap->job_head;
    if (capacity <= heap->job_capacity || capacity > ((size_t)-1) / sizeof *grown) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    grown = (kiln_microtask *)malloc(capacity * sizeof *grown);
    if (grown == NULL) return KILN_C99_OUT_OF_MEMORY;
    if (live != 0) {
        size_t head_idx = heap->job_head & heap->job_mask;
        if (head_idx + live <= heap->job_capacity) {
            memcpy(grown, heap->jobs + head_idx, live * sizeof *grown);
        } else {
            size_t first_part = heap->job_capacity - head_idx;
            memcpy(grown, heap->jobs + head_idx, first_part * sizeof *grown);
            memcpy(grown + first_part, heap->jobs, (live - first_part) * sizeof *grown);
        }
    }
    free(heap->jobs);
    heap->jobs = grown;
    heap->job_head = 0;
    heap->job_tail = live;
    heap->job_capacity = capacity;
    heap->job_mask = capacity - 1;
    return KILN_C99_OK;
}

static int kiln_c99_enqueue(kiln_value handler, kiln_value argument, kiln_value derived,
                            int rejected, int continuation) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    kiln_microtask *job;
    if (heap == NULL) return KILN_C99_OUT_OF_MEMORY;
    /* The heapless fallback has no realm, so no lane was registered for it and nothing would
     * ever drain this ring. Refusing names the situation where appending would instead leave a
     * promise that never settles -- the failure this repository spends the most effort not
     * having, because nothing stops and nothing says why. */
    if (KILN_UNLIKELY(heap->fallback)) {
        KILN_C99_REFUSE("a promise job queued outside any realm, where nothing would run it");
    }
    if (KILN_UNLIKELY(heap->job_tail - heap->job_head == heap->job_capacity)) {
        int status = kiln_c99_jobs_grow(heap);
        if (status != KILN_C99_OK) return status;
    }
    job = &heap->jobs[heap->job_tail++ & heap->job_mask];
    job->handler = handler;
    job->argument = argument;
    job->derived = derived;
    job->rejected = rejected;
    job->continuation = continuation;
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
        if (value.bits == promise.bits) {
            kiln_value cycle_err;
            (void)kiln_c99_throw_error(&cycle_err, KILN_C99_BUILTIN_TYPE_ERROR,
                                       "Chaining cycle detected for promise");
            return kiln_c99_settle(promise, cycle_err, 1);
        }
        kiln_value step;
        status = kiln_c99_bind(&step, kiln_c99_method(KILN_C99_METHOD_ADOPT_STEP), promise);
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_enqueue(step, value, kiln_c99_undefined(), 0, 0);
    }
    if (!rejected && kiln_c99_is_reference(value)) {
        kiln_value then_prop = kiln_c99_undefined();
        kiln_value thrown = kiln_c99_undefined();
        status = kiln_c99_get(NULL, value, "then", 4, &then_prop, 0, &thrown);
        if (status == KILN_C99_THROWN) {
            return kiln_c99_settle(promise, thrown, 1);
        }
        if (status != KILN_C99_OK && status != KILN_C99_THROWN) {
            return status;
        }
        if (kiln_c99_is_callable(then_prop)) {
            kiln_value step;
            status = kiln_c99_bind_with(&step, kiln_c99_method(KILN_C99_METHOD_ADOPT_STEP), promise, &then_prop, 1);
            if (status != KILN_C99_OK) {
                return status;
            }
            return kiln_c99_enqueue(step, value, kiln_c99_undefined(), 0, 0);
        }
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
static int kiln_c99_then_continue_mode(kiln_value promise, kiln_value generator, int mode) {
    kiln_promise *state = kiln_c99_promise_of(promise);
    kiln_reaction reaction;
    int status;
    reaction.on_fulfilled = generator;
    reaction.on_rejected = generator;
    reaction.derived = kiln_c99_undefined();
    reaction.continuation = mode;
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

static int kiln_c99_then_continue(kiln_value promise, kiln_value generator) {
    return kiln_c99_then_continue_mode(promise, generator, 1);
}

static int kiln_c99_await_suspend(kiln_host *host, kiln_value generator, kiln_value produced,
                                  int mode, int depth) {
    if (produced.tag == KILN_VALUE_PROMISE) {
        return kiln_c99_then_continue_mode(produced, generator, mode);
    }
    if (kiln_c99_is_reference(produced)) {
        kiln_value then_prop = kiln_c99_undefined();
        kiln_value get_thrown = kiln_c99_undefined();
        int get_status = kiln_c99_get(host, produced, "then", 4, &then_prop, depth, &get_thrown);
        if (get_status == KILN_C99_THROWN) {
            return kiln_c99_enqueue(generator, get_thrown, kiln_c99_undefined(), 1, mode);
        }
        if (get_status != KILN_C99_OK && get_status != KILN_C99_THROWN) {
            return get_status;
        }
        if (kiln_c99_is_callable(then_prop)) {
            kiln_value wrapper;
            int wrap_status = kiln_c99_new_promise(&wrapper);
            if (wrap_status != KILN_C99_OK) return wrap_status;
            kiln_value step;
            wrap_status = kiln_c99_bind_with(&step, kiln_c99_method(KILN_C99_METHOD_ADOPT_STEP), wrapper, &then_prop, 1);
            if (wrap_status != KILN_C99_OK) return wrap_status;
            wrap_status = kiln_c99_enqueue(step, produced, kiln_c99_undefined(), 0, 0);
            if (wrap_status != KILN_C99_OK) return wrap_status;
            return kiln_c99_then_continue_mode(wrapper, generator, mode);
        }
    }
    return kiln_c99_enqueue(generator, produced, kiln_c99_undefined(), 0, mode);
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

/* Run one promise job. `task` is the drain's own copy, not a pointer into the ring: a handler
 * that enqueues can grow the array out from under it. */
static int kiln_c99_run_job(kiln_microtask *task, kiln_host *host, kiln_value *thrown) {
    kiln_value produced = kiln_c99_undefined();
    int status;
    if (task->continuation) {
        if (task->continuation == 2) {
            kiln_generator *frame = kiln_c99_generator_of(task->handler);
            if (task->rejected) {
                status = kiln_c99_async_generator_step(host, task->handler, task->argument,
                                                       1, 0, thrown);
                if (status == KILN_C99_THROWN) return KILN_C99_THROWN;
                kiln_c99_publish_turn_refusal(status, kiln_c99_undefined(), thrown);
                return status;
            }
            kiln_value step = frame->promise;
            status = kiln_c99_async_generator_settle(host, step, task->argument, 0, 0, thrown);
            if (status != KILN_C99_OK) return status;
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
                    return kiln_c99_async_generator_settle(host, next_p, kiln_c99_undefined(), 1, 0, thrown);
                }
                return kiln_c99_async_generator_step(host, task->handler, next_s, 0, 0, thrown);
            }
            return status;
        }
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

/* Does the lane hold a job the realm has not run yet? */
static int kiln_c99_promise_jobs_pending(void *context) {
    kiln_gc_heap *heap = (kiln_gc_heap *)context;
    return heap != NULL && heap->job_head < heap->job_tail;
}

/* Run the lane empty, including everything queued while it runs -- which is the whole point: a
 * `then` chain and an `await` loop both queue their next job from inside the current one, and a
 * checkpoint that ran only what was there when it started would leave the rest for the next
 * turn and print in the wrong order.
 *
 * The copy into `job` is what keeps the values alive: a popped job is no longer marked through
 * the ring, and the local is a C frame the conservative stack scan reads, exactly as the popped
 * microtask cell used to be kept alive by the local that held it. */
static int kiln_c99_run_promise_jobs(void *context, void *host_pointer, void *thrown_pointer) {
    kiln_gc_heap *heap = (kiln_gc_heap *)context;
    kiln_host *host = (kiln_host *)host_pointer;
    kiln_value *thrown = (kiln_value *)thrown_pointer;
    if (heap == NULL || thrown == NULL) return KILN_C99_INVALID_ARGUMENT;
    while (heap->job_head < heap->job_tail) {
        kiln_microtask job = heap->jobs[heap->job_head++ & heap->job_mask];
        int status;
        /* An emptied ring starts again at zero, so a program that never has two jobs waiting at
         * once never grows the array past its first block. */
        if (heap->job_head == heap->job_tail) {
            heap->job_head = 0;
            heap->job_tail = 0;
        }
        status = kiln_c99_run_job(&job, host, thrown);
        if (status != KILN_C99_OK) return status;
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
    kiln_c99_gc_keep_alive(task);
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
    /* The Realm removes one shot work from its queue before entering the callback. Keep the
     * payload live through the callback's final use; otherwise an optimized build can retire
     * this pointer while its callback's captures are still live. */
    kiln_c99_gc_keep_alive(timer);
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
            return kiln_c99_await_suspend(host, generator, produced, 1, depth);
        }
        return kiln_c99_await_suspend(host, generator, produced, 2, depth);
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
        return kiln_c99_await_suspend(host, generator, produced, 1, depth);
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
