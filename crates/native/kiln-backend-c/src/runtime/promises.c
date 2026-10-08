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
    job->rejected = (rejected != 0);
    job->continuation = (uint8_t)continuation;
    return KILN_C99_OK;
}

int kiln_c99_enqueue_finalizer(kiln_value callback, kiln_value held_value) {
    return kiln_c99_enqueue(callback, held_value, kiln_c99_undefined(), 0, 0);
}

static int kiln_c99_enqueue_next_tick(kiln_value callback, const kiln_value *args, size_t argc) {
    kiln_next_tick *task =
        (kiln_next_tick *)kiln_c99_gc_alloc(KILN_GC_NEXT_TICK, sizeof *task);
    int status;
    if (task == NULL) return KILN_C99_OUT_OF_MEMORY;
    kiln_c99_heap_store_value(task, &task->callback, callback);
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
        kiln_c99_heap_copy_values(task, task->args, args, argc);
    }
    /* Counted before the hand-off: a refused hand-off disposes of the task, which uncounts it. */
    task->counted = 0;
    if (kiln_c99_gc_heap() != NULL) {
        kiln_c99_gc_heap()->next_ticks_queued += 1;
        task->counted = 1;
    }
    status = kiln_realm_queue_next_tick(kiln_c99_run_next_tick, kiln_c99_drop_next_tick, task);
    if (status != KILN_C99_OK) {
        kiln_c99_drop_next_tick(task);
        return status;
    }
    return KILN_C99_OK;
}

static int kiln_c99_new_promise(kiln_value *out) {
    /* The inlined bump: a promise is made on every `await`-driven sleep and every `then`. */
    kiln_promise *promise =
        (kiln_promise *)kiln_c99_gc_alloc_hot(KILN_GC_PROMISE, sizeof *promise, 1);
    if (promise == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    promise->state = KILN_C99_PENDING;
    promise->handled = 0;
    promise->rejection = KILN_C99_REJECTION_NONE;
    promise->value = kiln_c99_undefined();
    kiln_c99_heap_store_pointer(promise, &promise->reactions, NULL);
    promise->count = 0;
    promise->capacity = 0;
    kiln_c99_heap_store_value(promise, &promise->properties, kiln_c99_undefined());
    *out = kiln_c99_value(KILN_VALUE_PROMISE, (uint64_t)(uintptr_t)promise, NULL, 0);
    return KILN_C99_OK;
}

static kiln_promise *kiln_c99_promise_of(kiln_value value) {
    return (kiln_promise *)(uintptr_t)value.bits;
}

/* Node's unhandled-rejection tracking (`kiln-builtins/src/rejections.ts`, shared with the
 * interpreter). This side keeps what needs a `kiln_promise`: the handled bit, the promises that
 * rejected with nothing attached (`[promise, reason, ...]`), and the reported ones something
 * handled since (`[promise, ...]`). The promise lane hands both lists to the registered tracking
 * once the tick and promise queues are empty -- node's `processPromiseRejections` point. */
#define KILN_C99_REALM_SLOT_REJECTED UINT32_C(9)
#define KILN_C99_REALM_SLOT_HANDLED_LATE UINT32_C(10)
#define KILN_C99_REALM_SLOT_REJECTION_TRACKING UINT32_C(11)

static int kiln_c99_rejection_note(uint32_t slot_key, kiln_value first, const kiln_value *second) {
    kiln_value *slot;
    int status = kiln_c99_realm_value_slot(slot_key, &slot);
    if (status != KILN_C99_OK) return status;
    if (slot->tag != KILN_VALUE_ARRAY) {
        status = kiln_c99_new_array(slot);
        if (status != KILN_C99_OK) return status;
    }
    status = kiln_c99_array_push(*slot, first);
    if (status == KILN_C99_OK && second != NULL) status = kiln_c99_array_push(*slot, *second);
    if (status == KILN_C99_OK) kiln_c99_gc_heap()->rejections_waiting += 1;
    return status;
}

/* HostPromiseRejectionTracker's "reject": `promise` rejected with nothing attached. */
static int kiln_c99_rejection_track(kiln_value promise, kiln_value reason) {
    kiln_c99_promise_of(promise)->rejection = KILN_C99_REJECTION_WAITING;
    return kiln_c99_rejection_note(KILN_C99_REALM_SLOT_REJECTED, promise, &reason);
}

/* HostPromiseRejectionTracker's "handle", from every place a reaction attaches. A rejection
 * still waiting is simply no longer reported (its list entry is skipped); a reported one is
 * `rejectionHandled`. */
static int kiln_c99_promise_observe(kiln_promise *state) {
    unsigned char rejection;
    if (state->handled) return KILN_C99_OK;
    state->handled = 1;
    rejection = state->rejection;
    if (rejection == KILN_C99_REJECTION_NONE) return KILN_C99_OK;
    state->rejection = KILN_C99_REJECTION_NONE;
    if (rejection == KILN_C99_REJECTION_WAITING) {
        kiln_c99_gc_heap()->rejections_waiting -= 1;
        return KILN_C99_OK;
    }
    return kiln_c99_rejection_note(KILN_C99_REALM_SLOT_HANDLED_LATE,
                                   kiln_c99_value(KILN_VALUE_PROMISE, (uint64_t)(uintptr_t)state,
                                                  NULL, 0),
                                   NULL);
}

/* `__kilnPromiseRejections(process)`: the realm bootstrap registers the shared tracking. The
 * first registration stands. */
static int kiln_c99_promise_rejections_raw(kiln_value *result, const kiln_value *args,
                                           size_t argc) {
    kiln_value *slot;
    int status;
    /* A registration, not a read: its answer is the call's `undefined`. */
    *result = kiln_c99_undefined();
    status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_REJECTION_TRACKING, &slot);
    if (status == KILN_C99_OK && argc > 0 && kiln_c99_is_callable(args[0]) &&
        !kiln_c99_is_callable(*slot))
        *slot = args[0];
    return status;
}

/* Take a list out of its slot, leaving the slot empty for whatever the tracking itself causes. */
static kiln_value kiln_c99_rejection_take(uint32_t slot_key) {
    kiln_value *slot;
    kiln_value taken = kiln_c99_undefined();
    if (kiln_c99_realm_value_slot(slot_key, &slot) == KILN_C99_OK) {
        taken = *slot;
        *slot = kiln_c99_undefined();
    }
    return taken;
}

/* Node's `processPromiseRejections`, run by the promise lane once both queues are empty: the
 * rejections still unhandled, marked reported, and the late handles, to the tracking. What it
 * throws -- the default `throw` mode's report -- ends the checkpoint as an uncaught exception. */
static int kiln_c99_process_rejections(kiln_gc_heap *heap, kiln_host *host, kiln_value *thrown) {
    kiln_value rejected = kiln_c99_rejection_take(KILN_C99_REALM_SLOT_REJECTED);
    kiln_value handled = kiln_c99_rejection_take(KILN_C99_REALM_SLOT_HANDLED_LATE);
    kiln_value reported;
    kiln_value *tracking;
    kiln_value arguments[2];
    kiln_value produced = kiln_c99_undefined();
    size_t index;
    int status;
    heap->rejections_waiting = 0;
    status = kiln_c99_new_array(&reported);
    if (status != KILN_C99_OK) return status;
    if (rejected.tag == KILN_VALUE_ARRAY) {
        kiln_array *entries = kiln_c99_array_of(rejected);
        for (index = 0; index + 1 < entries->count; index += 2) {
            kiln_value promise = kiln_val_to_boundary(entries->elements[index]);
            kiln_promise *state = kiln_c99_promise_of(promise);
            if (state->rejection != KILN_C99_REJECTION_WAITING) continue;
            state->rejection = KILN_C99_REJECTION_REPORTED;
            status = kiln_c99_array_push(reported, promise);
            if (status == KILN_C99_OK)
                status = kiln_c99_array_push(reported,
                                             kiln_val_to_boundary(entries->elements[index + 1]));
            if (status != KILN_C99_OK) return status;
        }
    }
    if (handled.tag != KILN_VALUE_ARRAY) {
        if (kiln_c99_array_of(reported)->count == 0) return KILN_C99_OK;
        status = kiln_c99_new_array(&handled);
        if (status != KILN_C99_OK) return status;
    }
    status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_REJECTION_TRACKING, &tracking);
    if (status != KILN_C99_OK) return status;
    if (!kiln_c99_is_callable(*tracking)) {
        /* A realm without the bootstrap: the first rejection is the uncaught exception. */
        if (kiln_c99_array_of(reported)->count == 0) return KILN_C99_OK;
        *thrown = kiln_val_to_boundary(kiln_c99_array_of(reported)->elements[1]);
        return KILN_C99_THROWN;
    }
    arguments[0] = reported;
    arguments[1] = handled;
    return kiln_c99_call(host, &produced, *tracking, kiln_c99_undefined(), arguments, 2, 0,
                         thrown);
}

static int kiln_c99_bind_with(kiln_value *out, kiln_value target, kiln_value receiver,
                              const kiln_value *arguments, size_t count) {
    kiln_bound *bound = (kiln_bound *)kiln_c99_gc_alloc(KILN_GC_BOUND, sizeof *bound);
    if (bound == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    kiln_c99_heap_store_value(bound, &bound->target, target);
    kiln_c99_heap_store_value(bound, &bound->receiver, receiver);
    kiln_c99_heap_store_pointer(bound, &bound->arguments, NULL);
    bound->count = count;
    kiln_c99_heap_store_value(bound, &bound->properties, kiln_c99_undefined());
    if (count != 0) {
        /* The argument array is the bound function's alone, so it stays malloc-backed and the
         * KILN_GC_BOUND finaliser is the one place it is released. */
        kiln_c99_heap_store_pointer(bound, &bound->arguments,
                                   malloc(count * sizeof *bound->arguments));
        if (bound->arguments == NULL) {
            kiln_c99_gc_release(bound);
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_heap_copy_values(bound, bound->arguments, arguments, count);
    }
    *out = kiln_c99_value(KILN_VALUE_BOUND, (uint64_t)(uintptr_t)bound, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_bind(kiln_value *out, kiln_value target, kiln_value receiver) {
    return kiln_c99_bind_with(out, target, receiver, NULL, 0);
}

static int kiln_c99_settle(kiln_value promise, kiln_value value, int rejected);

/* Append one reaction to a pending promise: into the inline slot when it is the first, then into
 * a doubling malloc'd array that starts by copying the inline one out. */
static int kiln_c99_promise_add_reaction(kiln_promise *promise, const kiln_reaction *reaction) {
    if (promise->capacity == 0) {
        kiln_c99_heap_store_pointer(promise, &promise->reactions, &promise->inline_reaction);
        promise->capacity = 1;
    }
    if (promise->count == promise->capacity) {
        size_t grown = promise->capacity < 4 ? 4 : promise->capacity * 2;
        kiln_reaction *moved;
        if (grown > ((size_t)-1) / sizeof *moved) return KILN_C99_OUT_OF_MEMORY;
        if (promise->reactions == &promise->inline_reaction) {
            moved = (kiln_reaction *)malloc(grown * sizeof *moved);
            if (moved == NULL) return KILN_C99_OUT_OF_MEMORY;
            for (size_t index = 0; index < promise->count; ++index) {
                moved[index].continuation = promise->reactions[index].continuation;
                kiln_c99_heap_store_value(promise, &moved[index].on_fulfilled,
                                          promise->reactions[index].on_fulfilled);
                kiln_c99_heap_store_value(promise, &moved[index].on_rejected,
                                          promise->reactions[index].on_rejected);
                kiln_c99_heap_store_value(promise, &moved[index].derived,
                                          promise->reactions[index].derived);
            }
            /* The promise owns memory outside its block now (`KILN_GC_FINALIZED_AT_ALLOC`). */
            kiln_c99_gc_note_finalizable(promise);
        } else {
            moved = (kiln_reaction *)realloc(promise->reactions, grown * sizeof *moved);
            if (moved == NULL) return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_heap_store_pointer(promise, &promise->reactions, moved);
        promise->capacity = grown;
    }
    promise->reactions[promise->count].continuation = reaction->continuation;
    kiln_c99_heap_store_value(promise, &promise->reactions[promise->count].on_fulfilled,
                              reaction->on_fulfilled);
    kiln_c99_heap_store_value(promise, &promise->reactions[promise->count].on_rejected,
                              reaction->on_rejected);
    kiln_c99_heap_store_value(promise, &promise->reactions[promise->count].derived,
                              reaction->derived);
    promise->count += 1;
    return KILN_C99_OK;
}

/* Queue one reaction against a promise that has already settled. */
static int kiln_c99_schedule(kiln_promise *promise, const kiln_reaction *reaction) {
    int rejected = promise->state == KILN_C99_REJECTED;
    /* The job runs in this checkpoint, and for an `await` its handler is a suspended frame
     * nothing has touched since it suspended: start bringing it in now. A hint only. */
    if (reaction->continuation != 0) {
        const char *frame = (const char *)(uintptr_t)reaction->on_fulfilled.bits;
        size_t line;
        /* The frame's fields and the first of its locals, which follow it in the block. */
        for (line = 0; line < sizeof(kiln_generator) + 64; line += 64) {
            __builtin_prefetch(frame + line);
        }
    }
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
    kiln_c99_heap_store_value(state, &state->value, value);
    if (rejected && !state->handled) {
        status = kiln_c99_rejection_track(promise, value);
        if (status != KILN_C99_OK) return status;
    }
    for (index = 0; index < state->count; ++index) {
        status = kiln_c99_schedule(state, &state->reactions[index]);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    state->count = 0;
    /* A settled promise never takes another reaction into the list -- a later `then` is
     * scheduled at once -- so the list is released now rather than when the promise dies. */
    if (state->reactions != &state->inline_reaction) free(state->reactions);
    kiln_c99_heap_store_pointer(state, &state->reactions, NULL);
    state->capacity = 0;
    return KILN_C99_OK;
}

static int kiln_c99_new_settler(kiln_value *out, kiln_value promise, int rejects) {
    kiln_settler *settler =
        (kiln_settler *)kiln_c99_gc_alloc_hot(KILN_GC_SETTLER, sizeof *settler, 1);
    if (settler == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    kiln_c99_heap_store_value(settler, &settler->promise, promise);
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
    int status = kiln_c99_promise_observe(state);
    if (status == KILN_C99_OK) status = kiln_c99_new_promise(&reaction.derived);
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
    return kiln_c99_promise_add_reaction(state, &reaction);
}

/* `p.then(f, r)` for the runtime's own bookkeeping, where nothing can ever read the promise `then`
 * would return: the reaction runs `f` or `r` and settles nothing after. A combinator's per-element
 * step is exactly that, and a derived promise per element was one more block for every task a
 * `Promise.all` waits on. */
static int kiln_c99_react(kiln_value promise, kiln_value on_fulfilled, kiln_value on_rejected) {
    kiln_promise *state = kiln_c99_promise_of(promise);
    kiln_reaction reaction;
    int status = kiln_c99_promise_observe(state);
    if (status != KILN_C99_OK) return status;
    reaction.on_fulfilled = on_fulfilled;
    reaction.on_rejected = on_rejected;
    reaction.derived = kiln_c99_undefined();
    reaction.continuation = 0;
    if (state->state != KILN_C99_PENDING) {
        return kiln_c99_schedule(state, &reaction);
    }
    return kiln_c99_promise_add_reaction(state, &reaction);
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
    int status = kiln_c99_promise_observe(state);
    if (status != KILN_C99_OK) return status;
    reaction.on_fulfilled = generator;
    reaction.on_rejected = generator;
    reaction.derived = kiln_c99_undefined();
    reaction.continuation = mode;
    if (state->state != KILN_C99_PENDING) {
        return kiln_c99_schedule(state, &reaction);
    }
    return kiln_c99_promise_add_reaction(state, &reaction);
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

static int kiln_c99_async_generator_settle(kiln_host *host, kiln_value step, kiln_value value,
                                           int finished, int depth, kiln_value *thrown);
static int kiln_c99_async_generator_step(kiln_host *host, kiln_value generator, kiln_value sent,
                                         int rejected, int depth, kiln_value *thrown);
static int kiln_c99_async_generator_delegate_result(kiln_host *host, kiln_value generator,
                                                     kiln_value result, int rejected, int depth,
                                                     kiln_value *thrown);
static int kiln_c99_async_generator_delegate_value(kiln_host *host, kiln_value generator,
                                                    kiln_value value, int done, int depth,
                                                    kiln_value *thrown);
static int kiln_c99_async_generator_deliver_yield(kiln_host *host, kiln_value generator,
                                                   kiln_value produced, int depth,
                                                   kiln_value *thrown);
enum {
    KILN_C99_ASYNC_REQUEST_NEXT,
    KILN_C99_ASYNC_REQUEST_RETURN,
    KILN_C99_ASYNC_REQUEST_THROW,
    /* Internal only: the return operand has completed AsyncGenerator Await. */
    KILN_C99_ASYNC_REQUEST_RETURN_READY
};

/* Request kinds are a compact internal discriminator. Keeping it beside each
 * queued promise matters: return and throw are requests on the same serialized generator, not
 * side channels that may mutate its active next(). */
static int kiln_c99_async_generator_queue_push(kiln_generator *frame, kiln_value promise,
                                                kiln_value sent, uint8_t request_kind) {
    if (frame->queue_count >= frame->queue_capacity) {
        size_t new_capacity = frame->queue_capacity == 0 ? 4 : frame->queue_capacity * 2;
        kiln_value *promises;
        kiln_value *values;
        uint8_t *methods;
        kiln_value *old_promises = frame->queue_promises;
        kiln_value *old_values = frame->queue_sent;
        uint8_t *old_methods = frame->queue_methods;
        size_t index;
        if (new_capacity < frame->queue_capacity || new_capacity > SIZE_MAX / sizeof(kiln_value))
            return KILN_C99_OUT_OF_MEMORY;
        promises = (kiln_value *)malloc(new_capacity * sizeof(kiln_value));
        if (promises == NULL) return KILN_C99_OUT_OF_MEMORY;
        values = (kiln_value *)malloc(new_capacity * sizeof(kiln_value));
        methods = (uint8_t *)malloc(new_capacity * sizeof(uint8_t));
        if (values == NULL || methods == NULL) {
            free(promises);
            free(values);
            free(methods);
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < frame->queue_count; ++index) {
            size_t old_slot = (frame->queue_head + index) % frame->queue_capacity;
            promises[index] = frame->queue_promises[old_slot];
            values[index] = frame->queue_sent[old_slot];
            methods[index] = frame->queue_methods[old_slot];
        }
        kiln_c99_heap_store_pointer(frame, &frame->queue_promises, promises);
        kiln_c99_heap_store_pointer(frame, &frame->queue_sent, values);
        kiln_c99_heap_store_pointer(frame, &frame->queue_methods, methods);
        frame->queue_capacity = new_capacity;
        frame->queue_head = 0;
        free(old_promises);
        free(old_values);
        free(old_methods);
        for (index = 0; index < frame->queue_count; ++index) {
            kiln_c99_heap_store_value(frame, &frame->queue_promises[index],
                                      frame->queue_promises[index]);
            kiln_c99_heap_store_value(frame, &frame->queue_sent[index], frame->queue_sent[index]);
        }
        /* The frame owns these malloc arrays from this point onward. */
        kiln_c99_gc_note_finalizable(frame);
    }
    {
        size_t slot = (frame->queue_head + frame->queue_count) % frame->queue_capacity;
        kiln_c99_heap_store_value(frame, &frame->queue_promises[slot], promise);
        kiln_c99_heap_store_value(frame, &frame->queue_sent[slot], sent);
        frame->queue_methods[slot] = request_kind;
    }
    frame->queue_count++;
    return KILN_C99_OK;
}

static void kiln_c99_async_generator_queue_pop(kiln_generator *frame, kiln_value *promise,
                                               kiln_value *sent, uint8_t *request_kind) {
    size_t slot = frame->queue_head;
    *promise = frame->queue_promises[slot];
    *sent = frame->queue_sent[slot];
    *request_kind = frame->queue_methods[slot];
    frame->queue_count--;
    frame->queue_head = frame->queue_count == 0
                            ? 0
                            : (frame->queue_head + 1) % frame->queue_capacity;
}

static int kiln_c99_async_generator_start_request(kiln_host *host, kiln_value generator,
                                                   kiln_value promise, kiln_value sent,
                                                   uint8_t request_kind, int depth,
                                                   kiln_value *thrown);

static int kiln_c99_async_generator_drain_closed(kiln_host *host, kiln_generator *frame,
                                                  int depth, kiln_value *thrown) {
    while (frame->queue_count != 0) {
        kiln_value promise;
        kiln_value sent;
        uint8_t request_kind;
        int status;
        kiln_c99_async_generator_queue_pop(frame, &promise, &sent, &request_kind);
        if (request_kind == KILN_C99_ASYNC_REQUEST_RETURN) {
            /* Return completion values are awaited even after the generator is closed. This
             * request owns the lane until its operand settles; its continuation drains the rest. */
            return kiln_c99_async_generator_start_request(host,
                                                          kiln_c99_value(
                                                              KILN_VALUE_GENERATOR,
                                                              (uint64_t)(uintptr_t)frame, NULL, 0),
                                                          promise, sent, request_kind, depth,
                                                          thrown);
        }
        if (request_kind == KILN_C99_ASYNC_REQUEST_THROW) {
            status = kiln_c99_settle(promise, sent, 1);
        } else {
            status = kiln_c99_async_generator_settle(
                host, promise,
                request_kind == KILN_C99_ASYNC_REQUEST_RETURN ||
                        request_kind == KILN_C99_ASYNC_REQUEST_RETURN_READY
                    ? sent
                    : kiln_c99_undefined(),
                1, depth, thrown);
        }
        if (status != KILN_C99_OK) return status;
    }
    frame->queue_executing = 0;
    return KILN_C99_OK;
}

static int kiln_c99_async_generator_start_request(kiln_host *host, kiln_value generator,
                                                   kiln_value promise, kiln_value sent,
                                                   uint8_t request_kind, int depth,
                                                   kiln_value *thrown) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    int status;
    frame->queue_executing = 1;
    kiln_c99_gc_keep_alive(frame);
    if (kiln_c99_is_reference(sent))
        kiln_c99_gc_keep_alive((const void *)(uintptr_t)sent.bits);
    if (promise.tag == KILN_VALUE_PROMISE)
        kiln_c99_gc_keep_alive((const void *)(uintptr_t)promise.bits);
    kiln_c99_heap_store_value(frame, &frame->promise, promise);
    frame->async_request_kind = request_kind == KILN_C99_ASYNC_REQUEST_RETURN_READY
                                    ? KILN_C99_ASYNC_REQUEST_RETURN
                                    : request_kind;
    if (request_kind == KILN_C99_ASYNC_REQUEST_RETURN) {
        /* AsyncGeneratorResumeNext awaits the return completion's value before it injects the
         * abrupt return into the suspended body (and does so even for an already-closed frame). */
        return kiln_c99_await_suspend(host, generator, sent, 5, depth);
    }
    if (request_kind == KILN_C99_ASYNC_REQUEST_RETURN_READY)
        request_kind = KILN_C99_ASYNC_REQUEST_RETURN;
    if (frame->done) {
        if (request_kind == KILN_C99_ASYNC_REQUEST_THROW)
            status = kiln_c99_settle(promise, sent, 1);
        else
            status = kiln_c99_async_generator_settle(
                host, promise,
                request_kind == KILN_C99_ASYNC_REQUEST_RETURN ? sent : kiln_c99_undefined(),
                1, depth, thrown);
        if (status != KILN_C99_OK) return status;
        return kiln_c99_async_generator_drain_closed(host, frame, depth, thrown);
    }
    frame->queue_executing = 1;
    if (frame->delegate.tag != KILN_VALUE_UNDEFINED &&
        request_kind != KILN_C99_ASYNC_REQUEST_NEXT) {
        kiln_value method = kiln_c99_undefined();
        kiln_value method_error = kiln_c99_undefined();
        const char *name = request_kind == KILN_C99_ASYNC_REQUEST_RETURN ? "return" : "throw";
        size_t name_len = request_kind == KILN_C99_ASYNC_REQUEST_RETURN ? 6 : 5;
        status = kiln_c99_get(host, frame->delegate, name, name_len, &method, depth, &method_error);
        if (status != KILN_C99_OK) {
            kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
            frame->delegate_exit = 0;
            frame->delegate_wrapped = 0;
            return kiln_c99_async_generator_step(host, generator,
                                                  status == KILN_C99_THROWN ? method_error : sent,
                                                  1, depth, thrown);
        }
        if (method.tag == KILN_VALUE_UNDEFINED || method.tag == KILN_VALUE_NULL) {
            if (request_kind == KILN_C99_ASYNC_REQUEST_RETURN) {
                kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
                frame->delegate_exit = 0;
                frame->delegate_wrapped = 0;
                return kiln_c99_async_generator_start_request(
                    host, generator, promise, sent, KILN_C99_ASYNC_REQUEST_RETURN_READY, depth,
                    thrown);
            }
            /* Yield* closes the delegate before throwing when it has no throw method. The
             * continuation validates/awaits the close result before injecting the TypeError. */
            status = kiln_c99_get(host, frame->delegate, "return", 6, &method, depth, &method_error);
            if (status != KILN_C99_OK) {
                kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
                frame->delegate_exit = 0;
                frame->delegate_wrapped = 0;
                return kiln_c99_async_generator_step(host, generator, method_error, 1, depth,
                                                      thrown);
            }
            if (method.tag == KILN_VALUE_UNDEFINED || method.tag == KILN_VALUE_NULL) {
                status = kiln_c99_throw_error(&method_error, KILN_C99_BUILTIN_TYPE_ERROR,
                                              "Iterator does not have a throw method");
                if (status != KILN_C99_THROWN) return status;
                kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
                frame->delegate_exit = 0;
                frame->delegate_wrapped = 0;
                return kiln_c99_async_generator_step(host, generator, method_error, 1, depth,
                                                      thrown);
            }
            if (!kiln_c99_is_callable(method)) {
                status = kiln_c99_throw_error(&method_error, KILN_C99_BUILTIN_TYPE_ERROR,
                                              "Iterator method is not callable");
                if (status != KILN_C99_THROWN) return status;
                kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
                frame->delegate_exit = 0;
                frame->delegate_wrapped = 0;
                return kiln_c99_async_generator_step(host, generator, method_error, 1, depth,
                                                      thrown);
            }
            frame->async_delegate_close = 1;
            {
                kiln_value close_result = kiln_c99_undefined();
                status = kiln_c99_call(host, &close_result, method, frame->delegate, NULL, 0,
                                       depth, &method_error);
                if (status != KILN_C99_OK) {
                    frame->async_delegate_close = 0;
                    kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
                    frame->delegate_exit = 0;
                    frame->delegate_wrapped = 0;
                    return kiln_c99_async_generator_step(
                        host, generator, status == KILN_C99_THROWN ? method_error : sent, 1,
                        depth, thrown);
                }
                return kiln_c99_await_suspend(host, generator, close_result, 3, depth);
            }
        }
        if (!kiln_c99_is_callable(method)) {
            status = kiln_c99_throw_error(&method_error, KILN_C99_BUILTIN_TYPE_ERROR,
                                          "Iterator method is not callable");
            if (status != KILN_C99_THROWN) return status;
            kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
            frame->delegate_exit = 0;
            frame->delegate_wrapped = 0;
            return kiln_c99_async_generator_step(host, generator, method_error, 1, depth, thrown);
        }
        {
            kiln_value delegated_result = kiln_c99_undefined();
            status = kiln_c99_call(host, &delegated_result, method, frame->delegate, &sent, 1,
                                   depth, &method_error);
            if (status != KILN_C99_OK) {
                kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
                frame->delegate_exit = 0;
                frame->delegate_wrapped = 0;
                return kiln_c99_async_generator_step(
                    host, generator, status == KILN_C99_THROWN ? method_error : sent, 1,
                    depth, thrown);
            }
            return kiln_c99_await_suspend(host, generator, delegated_result, 3, depth);
        }
    }
    if (request_kind == KILN_C99_ASYNC_REQUEST_THROW) {
        frame->async_return_pending = 0;
        kiln_c99_heap_store_value(frame, &frame->async_return_value, kiln_c99_undefined());
        return kiln_c99_async_generator_step(host, generator, sent, 1, depth, thrown);
    }
    if (request_kind == KILN_C99_ASYNC_REQUEST_RETURN) {
        frame->async_return_pending = 1;
        kiln_c99_heap_store_value(frame, &frame->async_return_value, sent);
        if (frame->handler_top != 0) {
            kiln_closure *closure = kiln_c99_closure_of(frame->function);
            kiln_val produced = KILN_VAL_UNDEFINED;
            frame->raising = 1;
            /* A return is an abrupt completion, not a thrown undefined value. In particular,
             * catch blocks must not intercept it while finally blocks still run. */
            kiln_c99_heap_store_value(frame, &frame->sent, kiln_c99_return_signal());
            frame->running = 1;
            status = kiln_c99_invoke_closure_body(host, &produced, closure,
                                                  kiln_val_from_boundary(frame->receiver), NULL, 0,
                                                  depth, thrown, frame, 0);
            frame->running = 0;
            frame->raising = 0;
            if (status == KILN_C99_YIELDED) {
                frame->started = 1;
                return kiln_c99_async_generator_deliver_yield(
                    host, generator, kiln_val_to_boundary(produced), depth, thrown);
            }
            if (status == KILN_C99_THROWN) {
                frame->done = 1;
                if (kiln_val_from_boundary(*thrown) == KILN_VAL_RETURN_SIGNAL) {
                    kiln_value return_value = frame->async_return_value;
                    frame->async_return_pending = 0;
                    if (kiln_c99_is_reference(return_value))
                        kiln_c99_gc_keep_alive((const void *)(uintptr_t)return_value.bits);
                    kiln_c99_heap_store_value(frame, &frame->async_return_value,
                                              kiln_c99_undefined());
                    status = kiln_c99_async_generator_settle(
                        host, promise, return_value, 1, depth, thrown);
                } else {
                    frame->async_return_pending = 0;
                    kiln_c99_heap_store_value(frame, &frame->async_return_value,
                                              kiln_c99_undefined());
                    status = kiln_c99_settle(promise, *thrown, 1);
                }
                if (status != KILN_C99_OK) return status;
                kiln_c99_gc_keep_alive(frame);
                kiln_c99_gc_keep_alive((const void *)(uintptr_t)promise.bits);
                return kiln_c99_async_generator_drain_closed(host, frame, depth, thrown);
            }
            if (status != KILN_C99_OK) return status;
            sent = kiln_val_to_boundary(produced);
        }
        frame->done = 1;
        frame->async_return_pending = 0;
        if (kiln_c99_is_reference(sent))
            kiln_c99_gc_keep_alive((const void *)(uintptr_t)sent.bits);
        kiln_c99_heap_store_value(frame, &frame->async_return_value, kiln_c99_undefined());
        status = kiln_c99_async_generator_settle(host, promise, sent, 1, depth, thrown);
        if (status != KILN_C99_OK) return status;
        kiln_c99_gc_keep_alive(frame);
        kiln_c99_gc_keep_alive((const void *)(uintptr_t)promise.bits);
        return kiln_c99_async_generator_drain_closed(host, frame, depth, thrown);
    }
    return kiln_c99_async_generator_step(host, generator, sent, 0, depth, thrown);
}

static int kiln_c99_async_generator_request(kiln_host *host, kiln_value generator, kiln_value promise,
                                     kiln_value sent, uint64_t method, int depth,
                                     kiln_value *thrown) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    uint8_t request_kind;
    int status;
    if (method == KILN_C99_METHOD_GENERATOR_NEXT)
        request_kind = KILN_C99_ASYNC_REQUEST_NEXT;
    else if (method == KILN_C99_METHOD_GENERATOR_RETURN)
        request_kind = KILN_C99_ASYNC_REQUEST_RETURN;
    else if (method == KILN_C99_METHOD_GENERATOR_THROW)
        request_kind = KILN_C99_ASYNC_REQUEST_THROW;
    else
        return KILN_C99_INVALID_ARGUMENT;
    if (frame->queue_executing)
        return kiln_c99_async_generator_queue_push(frame, promise, sent, request_kind);
    if (frame->queue_count != 0) {
        kiln_value ready_promise;
        kiln_value ready_sent;
        uint8_t ready_method;
        /* A reentrant request can arrive while the previous result's `then` getter runs. Put
         * that request behind existing work, then activate the oldest request immediately. */
        status = kiln_c99_async_generator_queue_push(frame, promise, sent, request_kind);
        if (status != KILN_C99_OK) return status;
        kiln_c99_async_generator_queue_pop(frame, &ready_promise, &ready_sent, &ready_method);
        return kiln_c99_async_generator_start_request(host, generator, ready_promise, ready_sent,
                                                       ready_method, depth, thrown);
    }
    status = kiln_c99_async_generator_start_request(host, generator, promise, sent, request_kind,
                                                    depth, thrown);
    if (status != KILN_C99_OK && status != KILN_C99_THROWN && thrown != NULL)
        kiln_c99_publish_turn_refusal(status, kiln_c99_undefined(), thrown);
    return status;
}

/* Finish one yielded value and release the async-generator queue in the same job. This is shared
 * by awaited yields and direct async-delegate values so the latter do not gain an extra turn. */
static int kiln_c99_async_generator_finish_yield(kiln_host *host, kiln_value generator,
                                                 kiln_value value, int depth,
                                                 kiln_value *thrown) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    kiln_value step = frame->promise;
    kiln_value shaped;
    int status = kiln_c99_generator_step(host, value, 0, &shaped, depth, thrown);
    if (status != KILN_C99_OK) return status;
    /* The shaped result is complete and rooted before resolving. A synchronous then getter may
     * reenter the generator, so expose the queued head only at this exact boundary. */
    frame->queue_executing = 0;
    status = kiln_c99_settle(step, shaped, 0);
    kiln_c99_gc_keep_alive((const void *)(uintptr_t)generator.bits);
    if (step.tag == KILN_VALUE_PROMISE)
        kiln_c99_gc_keep_alive((const void *)(uintptr_t)step.bits);
    if (status != KILN_C99_OK) return status;
    /* Let a request made by the result's synchronous `then` getter activate the ready head.
     * If it did not, activate that same FIFO head after settlement returns. */
    if (!frame->queue_executing && frame->queue_count > 0) {
        kiln_value next_p;
        kiln_value next_s;
        uint8_t next_method;
        kiln_c99_async_generator_queue_pop(frame, &next_p, &next_s, &next_method);
        return kiln_c99_async_generator_start_request(host, generator, next_p, next_s,
                                                       next_method, 0, thrown);
    }
    return status;
}

static int kiln_c99_async_generator_deliver_yield(kiln_host *host, kiln_value generator,
                                                   kiln_value produced, int depth,
                                                   kiln_value *thrown) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    if (frame->awaiting) return kiln_c99_await_suspend(host, generator, produced, 1, depth);
    if (frame->delegate.tag != KILN_VALUE_UNDEFINED) {
        kiln_value value = kiln_c99_undefined();
        kiln_value error = kiln_c99_undefined();
        int status = kiln_c99_get(host, produced, "value", 5, &value, depth, &error);
        if (status == KILN_C99_THROWN) {
            kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
            frame->delegate_exit = 0;
            frame->delegate_wrapped = 0;
            return kiln_c99_enqueue(generator, error, kiln_c99_undefined(), 1, 1);
        }
        if (status != KILN_C99_OK) return status;
        if (frame->delegate_wrapped)
            return kiln_c99_await_suspend(host, generator, value, 2, depth);
        return kiln_c99_async_generator_finish_yield(host, generator, value, depth, thrown);
    }
    return kiln_c99_await_suspend(host, generator, produced, 2, depth);
}

/* Run one promise job. `task` is the drain's own copy, not a pointer into the ring: a handler
 * that enqueues can grow the array out from under it. */
static int kiln_c99_run_job(kiln_microtask *task, kiln_host *host, kiln_value *thrown) {
    kiln_value produced = kiln_c99_undefined();
    int status;
    if (task->continuation) {
        if (task->continuation == 3) {
            return kiln_c99_async_generator_delegate_result(host, task->handler, task->argument,
                                                            task->rejected, 0, thrown);
        }
        if (task->continuation == 4) {
            kiln_generator *frame = kiln_c99_generator_of(task->handler);
            kiln_value error = task->argument;
            frame->async_delegate_close = 0;
            kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
            frame->delegate_exit = 0;
            frame->delegate_wrapped = 0;
            if (!task->rejected) {
                status = kiln_c99_throw_error(&error, KILN_C99_BUILTIN_TYPE_ERROR,
                                              "Iterator does not have a throw method");
                if (status != KILN_C99_THROWN) return status;
            }
            return kiln_c99_async_generator_step(host, task->handler, error, 1, 0, thrown);
        }
        if (task->continuation == 5) {
            kiln_generator *frame = kiln_c99_generator_of(task->handler);
            if (task->rejected) {
                /* Awaiting the argument to return can reject before the generator is resumed.
                 * Reject this request, leave the suspended body intact, and continue FIFO. */
                status = kiln_c99_settle(frame->promise, task->argument, 1);
                if (status != KILN_C99_OK) return status;
                frame->queue_executing = 0;
                if (frame->queue_count != 0) {
                    kiln_value next_p, next_s;
                    uint8_t next_method;
                    kiln_c99_async_generator_queue_pop(frame, &next_p, &next_s, &next_method);
                    return kiln_c99_async_generator_start_request(
                        host, task->handler, next_p, next_s, next_method, 0, thrown);
                }
                return KILN_C99_OK;
            }
            return kiln_c99_async_generator_start_request(
                host, task->handler, frame->promise, task->argument,
                KILN_C99_ASYNC_REQUEST_RETURN_READY, 0, thrown);
        }
        if (task->continuation == 6) {
            kiln_generator *frame = kiln_c99_generator_of(task->handler);
            if (task->rejected)
                return kiln_c99_async_generator_step(host, task->handler, task->argument, 1, 0,
                                                     thrown);
            return kiln_c99_async_generator_delegate_value(
                host, task->handler, task->argument, frame->async_delegate_result_done, 0,
                thrown);
        }
        if (task->continuation == 2) {
            if (task->rejected) {
                status = kiln_c99_async_generator_step(host, task->handler, task->argument,
                                                       1, 0, thrown);
                if (status == KILN_C99_THROWN) return KILN_C99_THROWN;
                kiln_c99_publish_turn_refusal(status, kiln_c99_undefined(), thrown);
                return status;
            }
            return kiln_c99_async_generator_finish_yield(host, task->handler, task->argument, 0,
                                                        thrown);
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

/* `__kilnJobsPending()`: what a microtask checkpoint would run now, as bits -- 1 for a
 * `process.nextTick` callback, 2 for a promise job or `queueMicrotask` callback, 4 for a
 * rejection to report (node's `hasRejectionToWarn`) (ADR 0065, representation: it reads the
 * realm's queues). A message port delivers its next message in the
 * same macrotask when this is 0, because a checkpoint between the two would run nothing, and
 * otherwise waits at the back of the checkpoint (`web_events_streams.ts`). */
static int kiln_c99_jobs_pending_raw(kiln_value *out) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    int bits = 0;
    if (heap != NULL) {
        if (heap->next_ticks_queued != 0) bits |= 1;
        if (heap->job_head < heap->job_tail) bits |= 2;
        if (heap->rejections_waiting != 0) bits |= 4;
    }
    *out = kiln_c99_number((double)bits);
    return KILN_C99_OK;
}

/* Does the lane hold a job the realm has not run yet -- or, with no tick left ahead of it, a
 * rejection to report? */
static int kiln_c99_promise_jobs_pending(void *context) {
    kiln_gc_heap *heap = (kiln_gc_heap *)context;
    return heap != NULL &&
           (heap->job_head < heap->job_tail ||
            (heap->rejections_waiting != 0 && heap->next_ticks_queued == 0));
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
        kiln_c99_gc_enter_job();
        status = kiln_c99_run_job(&job, host, thrown);
        kiln_c99_gc_leave_job();
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    /* Node's `processTicksAndRejections`: ticks and promise jobs both run out, then the
     * rejections; the realm's drain comes back around for whatever reporting them queued. */
    if (heap->rejections_waiting != 0 && heap->next_ticks_queued == 0)
        return kiln_c99_process_rejections(heap, host, thrown);
    return KILN_C99_OK;
}

/* The end of an ES module body. Node evaluates one from inside a promise job (the loader awaits
 * `module.evaluate()`), so the checkpoint that follows the body is already in progress and the
 * promise jobs the body queued run before any `process.nextTick` it queued. The launcher calls
 * this after the body and before the turn loop, which puts the tick lane first the way a
 * CommonJS body -- which runs from the top of a macrotask -- has it. Until 2026-09-19 an ES
 * module printed `tick` before `then` where node prints it after. */
int kiln_c99_finish_module_checkpoint(kiln_host *host, kiln_value *thrown) {
    return kiln_c99_run_promise_jobs(kiln_c99_gc_heap(), host, thrown);
}

/* The finaliser frees `args`, so the release below must not free it a second time. */
static void kiln_c99_uncount_next_tick(kiln_next_tick *task) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (task == NULL || !task->counted) return;
    task->counted = 0;
    if (heap != NULL && heap->next_ticks_queued != 0) heap->next_ticks_queued -= 1;
}

/* Called exactly once per queued tick -- after it ran, or when it is discarded unrun. */
static void kiln_c99_drop_next_tick(void *work) {
    kiln_c99_uncount_next_tick((kiln_next_tick *)work);
    kiln_c99_gc_release(work);
}

static int kiln_c99_run_next_tick(void *work, void *host_pointer, void *thrown_pointer) {
    kiln_next_tick *task = (kiln_next_tick *)work;
    kiln_host *host = (kiln_host *)host_pointer;
    kiln_value *thrown = (kiln_value *)thrown_pointer;
    kiln_value produced = kiln_c99_undefined();
    int status;
    if (task == NULL || thrown == NULL) return KILN_C99_INVALID_ARGUMENT;
    kiln_c99_gc_enter_job();
    kiln_c99_uncount_next_tick(task);
    status = kiln_c99_call(host, &produced, task->callback, kiln_c99_undefined(),
                           task->argc == 0 ? NULL : task->args, task->argc, 0, thrown);
    kiln_c99_gc_leave_job();
    kiln_c99_publish_turn_refusal(status, produced, thrown);
    kiln_c99_gc_keep_alive(task);
    return status;
}

/* A timer is its own record, and the record is the realm's work: the block the queue runs and
 * roots, and the one a program holds as a `Timeout` or `Immediate` (`KILN_VALUE_TIMER`). It
 * carries the callback and arguments, so a timeout that already ran can be armed again by a
 * `refresh()` for as long as the program still holds it, and a cleared one has dropped them.
 *
 * The queue's reference is counted in `owners`, because the realm drops its work after running it
 * and a timeout that re-arms itself from inside its own callback has two entries for a moment.
 * A drop that finds the last entry gone before the timer ever ran is a cancellation, which is how
 * a timer cleared by its bare identity -- all the realm has -- still lets go of its callback. The
 * record itself is collected, never released here: the program may still hold it. */
static void kiln_c99_timer_drop_state(kiln_timer *timer) {
    timer->cleared = 1;
    kiln_c99_heap_store_value(timer, &timer->callback, kiln_c99_undefined());
    timer->argc = 0;
}

static void kiln_c99_drop_timer(void *work) {
    kiln_timer *timer = (kiln_timer *)work;
    if (timer->owners > 0) timer->owners -= 1;
    if (timer->owners == 0 && !timer->ran) kiln_c99_timer_drop_state(timer);
}

/* A record for `callback(...args)`, or NULL when memory is short. The arguments are copied into
 * a malloc'd array the collector traces and the finaliser frees. */
static kiln_timer *kiln_c99_timer_new(kiln_value callback, const kiln_value *args, size_t argc,
                                      double delay, int kind) {
    kiln_timer *timer = (kiln_timer *)kiln_c99_gc_calloc(KILN_GC_TIMER, sizeof *timer);
    size_t index;
    if (timer == NULL) return NULL;
    kiln_c99_heap_store_value(timer, &timer->callback, callback);
    timer->delay = delay;
    timer->kind = (uint8_t)kind;
    timer->owners = 1;
    if (argc != 0) {
        timer->args = (kiln_value *)malloc(argc * sizeof *timer->args);
        if (timer->args == NULL) return NULL;
        timer->argc = argc;
        for (index = 0; index < argc; ++index)
            kiln_c99_heap_store_value(timer, &timer->args[index], args[index]);
    }
    return timer;
}

static int kiln_c99_run_timer(void *work, void *host_pointer, void *thrown_pointer) {
    kiln_timer *timer = (kiln_timer *)work;
    kiln_host *host = (kiln_host *)host_pointer;
    kiln_value *thrown = (kiln_value *)thrown_pointer;
    kiln_value produced = kiln_c99_undefined();
    int status;
    if (timer == NULL || thrown == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (timer->cleared) return KILN_C99_OK;
    /* Before the callback, so a `refresh()` inside it sees a timeout that has already run. */
    if (timer->kind != KILN_C99_TIMER_INTERVAL) timer->ran = 1;
    kiln_c99_gc_enter_job();
    status = kiln_c99_call(host, &produced, timer->callback, kiln_c99_undefined(),
                           timer->argc == 0 ? NULL : timer->args, timer->argc, 0, thrown);
    kiln_c99_gc_leave_job();
    kiln_c99_publish_turn_refusal(status, produced, thrown);
    /* The Realm removes one shot work from its queue before entering the callback. Keep the
     * payload live through the callback's final use; otherwise an optimized build can retire
     * this pointer while its callback's captures are still live. */
    kiln_c99_gc_keep_alive(timer);
    return status;
}

/* A timer whose callback is a heap function and that carries no extra arguments -- nearly every
 * `setTimeout(fn, ms)` -- hands the realm the function itself as its work, with no `kiln_timer`
 * block around it: one allocation and one release fewer per timer, and one block fewer for every
 * pending timer the collector marks. The realm roots the work pointer exactly as it rooted the
 * `kiln_timer`, and a function is released by the collector, not by the timer, so there is
 * nothing to drop. One run function per tag, because the pointer alone does not say which it
 * was. */
static int kiln_c99_run_timer_value(kiln_value callback, void *host_pointer,
                                    void *thrown_pointer) {
    kiln_host *host = (kiln_host *)host_pointer;
    kiln_value *thrown = (kiln_value *)thrown_pointer;
    kiln_value produced = kiln_c99_undefined();
    int status;
    if (thrown == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_call(host, &produced, callback, kiln_c99_undefined(), NULL, 0, 0, thrown);
    kiln_c99_publish_turn_refusal(status, produced, thrown);
    kiln_c99_gc_keep_alive((const void *)(uintptr_t)callback.bits);
    return status;
}

static int kiln_c99_run_timer_function(void *work, void *host, void *thrown) {
    return kiln_c99_run_timer_value(
        kiln_c99_value(KILN_VALUE_FUNCTION, (uint64_t)(uintptr_t)work, NULL, 0), host, thrown);
}

/* A promise's own `resolve` or `reject` as the callback -- `new Promise((r) => setTimeout(r, ms))`
 * -- is the commonest timer there is, and calling a settler is exactly settling its promise with
 * no value. So the timer holds the promise itself: the settler is garbage as soon as the
 * executor returns instead of living as long as the timer, and the one object the timer's run
 * touches is the one it settles. Nothing can tell: a settler has no identity a call observes. */
static int kiln_c99_run_timer_resolve(void *work, void *host, void *thrown) {
    int status;
    (void)host;
    if (thrown == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_settle(kiln_c99_value(KILN_VALUE_PROMISE, (uint64_t)(uintptr_t)work, NULL, 0),
                             kiln_c99_undefined(), 0);
    kiln_c99_publish_turn_refusal(status, kiln_c99_undefined(), (kiln_value *)thrown);
    kiln_c99_gc_keep_alive(work);
    return status;
}

static int kiln_c99_run_timer_reject(void *work, void *host, void *thrown) {
    int status;
    (void)host;
    if (thrown == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_settle(kiln_c99_value(KILN_VALUE_PROMISE, (uint64_t)(uintptr_t)work, NULL, 0),
                             kiln_c99_undefined(), 1);
    kiln_c99_publish_turn_refusal(status, kiln_c99_undefined(), (kiln_value *)thrown);
    kiln_c99_gc_keep_alive(work);
    return status;
}

static int kiln_c99_run_timer_bound(void *work, void *host, void *thrown) {
    return kiln_c99_run_timer_value(
        kiln_c99_value(KILN_VALUE_BOUND, (uint64_t)(uintptr_t)work, NULL, 0), host, thrown);
}

static void kiln_c99_drop_timer_value(void *work) {
    (void)work;
}

typedef int (*kiln_c99_timer_run_fn)(void *work, void *host, void *thrown);

/* The direct run function for `callback`, or NULL when it needs a `kiln_timer`; `*work` is the
 * pointer the realm is to hold -- the function itself, or a settler's promise. */
static kiln_c99_timer_run_fn kiln_c99_timer_direct_run(kiln_value callback, void **work) {
    if (callback.bits == 0) return NULL;
    *work = (void *)(uintptr_t)callback.bits;
    switch (callback.tag) {
    case KILN_VALUE_FUNCTION: return kiln_c99_run_timer_function;
    case KILN_VALUE_SETTLER: {
        kiln_settler *settler = (kiln_settler *)(uintptr_t)callback.bits;
        if (settler->promise.tag != KILN_VALUE_PROMISE || settler->promise.bits == 0) return NULL;
        *work = (void *)(uintptr_t)settler->promise.bits;
        return settler->rejects ? kiln_c99_run_timer_reject : kiln_c99_run_timer_resolve;
    }
    case KILN_VALUE_BOUND: return kiln_c99_run_timer_bound;
    default: return NULL;
    }
}

/* Arms `work` in the realm lane the scheduling method `code` names: a timeout or an interval
 * after `delay`, an immediate (node's check phase), or an I/O callback (its poll phase, where
 * `__kilnQueueIo` puts a message port's delivery). */
static int kiln_c99_arm_timer(uint64_t code, double delay, kiln_realm_work_fn run,
                              kiln_realm_drop_fn drop, void *work, uint64_t *id) {
    switch (code) {
    case KILN_C99_METHOD_SET_INTERVAL: return kiln_realm_set_interval(delay, run, drop, work, id);
    case KILN_C99_METHOD_SET_IMMEDIATE: return kiln_realm_set_immediate(run, drop, work, id);
    case KILN_C99_METHOD_QUEUE_IO_RAW: return kiln_realm_queue_io(run, drop, work, id);
    default: return kiln_realm_set_timeout(delay, run, drop, work, id);
    }
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
    int status = kiln_c99_generator_step(host, value, finished, &shaped, depth, thrown);
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
        status = kiln_c99_settle(step, sent, 1);
        if (status != KILN_C99_OK) return status;
        return kiln_c99_async_generator_drain_closed(host, frame, depth, thrown);
    }
    kiln_c99_heap_store_value(frame, &frame->sent, sent);
    frame->raising = rejected;
    closure = kiln_c99_closure_of(frame->function);
    kiln_val vproduced = KILN_VAL_UNDEFINED;
    status = kiln_c99_invoke_closure_body(host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0,
                                          depth, thrown, frame, 0);
    produced = kiln_val_to_boundary(vproduced);
    if (status == KILN_C99_YIELDED) {
        frame->started = 1;
        return kiln_c99_async_generator_deliver_yield(host, generator, produced, depth, thrown);
    }
    if (status == KILN_C99_THROWN) {
        frame->done = 1;
        if (frame->async_return_pending &&
            kiln_val_from_boundary(*thrown) == KILN_VAL_RETURN_SIGNAL) {
            kiln_value return_value = frame->async_return_value;
            frame->async_return_pending = 0;
            if (kiln_c99_is_reference(return_value))
                kiln_c99_gc_keep_alive((const void *)(uintptr_t)return_value.bits);
            kiln_c99_heap_store_value(frame, &frame->async_return_value, kiln_c99_undefined());
            status = kiln_c99_async_generator_settle(host, step, return_value, 1, depth, thrown);
        } else {
            frame->async_return_pending = 0;
            kiln_c99_heap_store_value(frame, &frame->async_return_value, kiln_c99_undefined());
            status = kiln_c99_settle(step, *thrown, 1);
        }
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_async_generator_drain_closed(host, frame, depth, thrown);
        return status;
    }
    if (status != KILN_C99_OK) {
        frame->queue_executing = 0;
        return status;
    }
    frame->done = 1;
    if (frame->async_return_pending) {
        frame->async_return_pending = 0;
        kiln_c99_heap_store_value(frame, &frame->async_return_value, kiln_c99_undefined());
    }
    status = kiln_c99_async_generator_settle(host, step, produced, 1, depth, thrown);
    if (status != KILN_C99_OK) return status;
    kiln_c99_gc_keep_alive(frame);
    kiln_c99_gc_keep_alive((const void *)(uintptr_t)step.bits);
    return kiln_c99_async_generator_drain_closed(host, frame, depth, thrown);
}

static int kiln_c99_async_generator_delegate_value(kiln_host *host, kiln_value generator,
                                                    kiln_value value, int done, int depth,
                                                    kiln_value *thrown) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    if (!done)
        return kiln_c99_async_generator_finish_yield(host, generator, value, depth, thrown);
    kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
    frame->delegate_exit = 0;
    frame->delegate_wrapped = 0;
    if (frame->async_request_kind == KILN_C99_ASYNC_REQUEST_RETURN)
        return kiln_c99_async_generator_start_request(
            host, generator, frame->promise, value, KILN_C99_ASYNC_REQUEST_RETURN_READY, depth,
            thrown);
    return kiln_c99_async_generator_step(host, generator, value, 0, depth, thrown);
}

/* The delegated method's result is awaited before this continuation runs. For an
 * AsyncFromSync delegate, its value is a second await; a real async iterator's value is yielded
 * unchanged. The same routine handles an externally queued return/throw and the delegation
 * completion it produces, keeping getter order and request serialization in one place. */
static int kiln_c99_async_generator_delegate_result(kiln_host *host, kiln_value generator,
                                                     kiln_value result, int rejected, int depth,
                                                     kiln_value *thrown) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    kiln_value error = kiln_c99_undefined();
    kiln_value done_value = kiln_c99_undefined();
    kiln_value value = kiln_c99_undefined();
    int status;
    if (rejected) {
        frame->async_delegate_close = 0;
        kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
        frame->delegate_exit = 0;
        frame->delegate_wrapped = 0;
        return kiln_c99_async_generator_step(host, generator, result, 1, depth, thrown);
    }
    if (!(kiln_c99_is_reference(result) || result.tag == KILN_VALUE_DATE)) {
        status = kiln_c99_throw_error(&error, KILN_C99_BUILTIN_TYPE_ERROR,
                                      "Iterator result is not an object");
        if (status != KILN_C99_THROWN) return status;
        frame->async_delegate_close = 0;
        kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
        frame->delegate_exit = 0;
        frame->delegate_wrapped = 0;
        return kiln_c99_async_generator_step(host, generator, error, 1, depth, thrown);
    }
    if (frame->async_delegate_close) {
        frame->async_delegate_close = 0;
        if (!frame->delegate_wrapped) {
            status = kiln_c99_throw_error(&error, KILN_C99_BUILTIN_TYPE_ERROR,
                                          "Iterator does not have a throw method");
            if (status != KILN_C99_THROWN) return status;
            kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
            frame->delegate_exit = 0;
            frame->delegate_wrapped = 0;
            return kiln_c99_async_generator_step(host, generator, error, 1, depth, thrown);
        }
        status = kiln_c99_get(host, result, "done", 4, &done_value, depth, &error);
        if (status != KILN_C99_OK) {
            if (status != KILN_C99_THROWN) return status;
            kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
            frame->delegate_exit = 0;
            frame->delegate_wrapped = 0;
            return kiln_c99_async_generator_step(host, generator, error, 1, depth, thrown);
        }
        status = kiln_c99_get(host, result, "value", 5, &value, depth, &error);
        if (status != KILN_C99_OK) {
            if (status != KILN_C99_THROWN) return status;
            kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
            frame->delegate_exit = 0;
            frame->delegate_wrapped = 0;
            return kiln_c99_async_generator_step(host, generator, error, 1, depth, thrown);
        }
        return kiln_c99_await_suspend(host, generator, value, 4, depth);
    }
    status = kiln_c99_get(host, result, "done", 4, &done_value, depth, &error);
    if (status != KILN_C99_OK) {
        if (status != KILN_C99_THROWN) return status;
        kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
        frame->delegate_exit = 0;
        frame->delegate_wrapped = 0;
        return kiln_c99_async_generator_step(host, generator, error, 1, depth, thrown);
    }
    status = kiln_c99_get(host, result, "value", 5, &value, depth, &error);
    if (status != KILN_C99_OK) {
        if (status != KILN_C99_THROWN) return status;
        kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
        frame->delegate_exit = 0;
        frame->delegate_wrapped = 0;
        return kiln_c99_async_generator_step(host, generator, error, 1, depth, thrown);
    }
    frame->async_delegate_result_done = (uint8_t)kiln_c99_truthy(done_value);
    if (frame->delegate_wrapped)
        return kiln_c99_await_suspend(host, generator, value, 6, depth);
    return kiln_c99_async_generator_delegate_value(
        host, generator, value, frame->async_delegate_result_done, depth, thrown);
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
    kiln_c99_heap_store_value(frame, &frame->sent, sent);
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
