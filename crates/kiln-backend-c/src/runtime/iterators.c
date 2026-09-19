
/* An iterator over a snapshot array.
 *
 * `for...of` asks for an iterator and then drives `next`, so a collection's `@@iterator` cannot
 * hand back the array itself -- an array has no `next`. The state is two reserved properties on
 * an ordinary object, which is what the interpreter does and for the same reason: an iterator is
 * an object with a method, and inventing a third kind of value for it would buy nothing. */
static int kiln_c99_new_iterator(kiln_value *out, kiln_value items) {
    kiln_value iterator;
    int status = kiln_c99_new_object(&iterator);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_put(NULL, iterator, "@@iterated", 10, items, 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_put(NULL, iterator, "@@index", 7, kiln_c99_number(0.0), 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_put(NULL, iterator, "next", 4,
                          kiln_c99_method(KILN_C99_METHOD_ITERATOR_NEXT), 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* An iterator is its own iterable, which is what lets `[...m.keys()]` work: spread asks for
     * an iterator, and asking one for another has to answer with itself. */
    status = kiln_c99_put(NULL, iterator, "@@iterator", 10,
                          kiln_c99_method(KILN_C99_METHOD_ITERATOR_SELF), 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    *out = iterator;
    return KILN_C99_OK;
}

static int kiln_c99_new_typed_iterator(kiln_value *out, kiln_value source, int part) {
    int status = kiln_c99_new_iterator(out, source);
    if (status != KILN_C99_OK) {
        return status;
    }
    return kiln_c99_put(NULL, *out, "@@iterpart", 10, kiln_c99_number((double)part), 0, NULL);
}

/* THE ITERATOR HELPERS (ES2025).
 *
 * `Iterator.from(x)` and the lazy family on `Iterator.prototype`. Lazy is the whole contract and
 * not a nicety: `map` and `filter` hand back another iterator that pulls one element when it is
 * asked, so `endless.map(f).take(3)` terminates. Building an array here would hang on the first
 * infinite source a program wrote, which is worse than refusing.
 *
 * A helper is an ordinary object carrying its state under reserved `@@iter*` keys -- reserved so
 * `kiln_c99_own_slot` marks them non-enumerable and nothing that walks own string keys can see
 * them -- and linked to `Iterator.prototype`, which is where its own `map` and `take` come from.
 * `@@iterhelper` holds the method code that built it, so one `next` serves the whole family and
 * the wrapper `Iterator.from` builds is the same object with a kind of zero.
 *
 * The interpreter is the contract: every one of these mirrors the `Native::IteratorLazy` and
 * `Native::IteratorTerminal` arms of `kiln-interpreter`, including which failures close the
 * source. `chunks`, `windows` and `join` are in the interpreter and not here; they refuse by
 * name through `kiln_c99_iterator_prototype_gap` rather than reading as undefined. */
static kiln_value kiln_c99_iterator_state(kiln_value target, const char *key, size_t key_len) {
    kiln_property *slot = kiln_c99_own_private(target, key, key_len);
    return slot == NULL ? kiln_c99_undefined() : slot->value;
}

/* Whether this object is one of ours, and which method built it.
 *
 * By the reserved slot rather than by shape: a program can build an object with a `next` and a
 * `return` of its own, and it is not a helper -- stepping it as one would read state that is not
 * there. A wrapper from `Iterator.from` answers kind zero, which is a code no method has. */
static int kiln_c99_is_iterator_helper(kiln_value value, uint64_t *kind) {
    kiln_property *slot;
    if (value.tag != KILN_VALUE_OBJECT) {
        return 0;
    }
    slot = kiln_c99_own_private(value, "@@iterhelper", 12);
    if (slot == NULL || slot->value.tag != KILN_VALUE_NUMBER) {
        return 0;
    }
    if (kind != NULL) {
        *kind = (uint64_t)kiln_c99_as_number(slot->value);
    }
    return 1;
}

/* Whether the iterator protocol will accept this value where it requires an object.
 *
 * Asked of a receiver, of what a `flatMap` mapper answered, and of a source about to be closed.
 * `GetIteratorDirect` asks only that the value be a *reference*; it is the `next` read off it
 * that decides whether the thing can actually be stepped, so a value with no `next` fails on
 * `next` and only a primitive fails here.
 *
 * A LIST OF TAGS RATHER THAN A TEST FOR "NOT A PRIMITIVE", and transcribed from
 * `is_iterator_object` in `kiln-interpreter` variant for variant rather than written from an
 * idea of what an object is. That is not caution: the first version of this function was written
 * from the idea, listed four tags, and left out `KILN_VALUE_ARRAY` -- so
 * `it.flatMap(x => [x, x])` refused, which is the commonest thing a `flatMap` callback returns
 * in any real program. Getting it wrong in the other direction is worse still and is why the
 * machine's own tags stay out: a primitive accepted as an iterator reads `done` as `undefined`
 * and then loops forever.
 *
 * `KILN_VALUE_COLLECTION` is in because the interpreter has no Map or Set variant -- both are
 * `Value::Object` there, so both are iterator objects there. `KILN_VALUE_SETTLER` and
 * `KILN_VALUE_TEMPORAL` are out because the interpreter has variants for both and lists
 * neither. */
static int kiln_c99_is_iterator_receiver(kiln_value value) {
    switch (value.tag) {
    case KILN_VALUE_OBJECT:
    case KILN_VALUE_ARRAY:
    case KILN_VALUE_FUNCTION:
    case KILN_VALUE_GENERATOR:
    case KILN_VALUE_PROXY:
    case KILN_VALUE_COLLECTION:
    case KILN_VALUE_TYPED_ARRAY:
    case KILN_VALUE_ARRAY_BUFFER:
    case KILN_VALUE_DATE:
    case KILN_VALUE_REGEXP:
    case KILN_VALUE_PROMISE:
    case KILN_VALUE_BOUND:
    /* The interpreter's one `Value::Native` arm; this backend spells the same thing two ways,
     * and `require` is a third callable it carries that the interpreter models as a native. */
    case KILN_VALUE_METHOD:
    case KILN_VALUE_BUILTIN:
    case KILN_VALUE_COMMONJS_REQUIRE:
        return 1;
    default:
        return 0;
    }
}

/* Whether `Iterator.prototype` is on this value's chain.
 *
 * What makes `Iterator.from(x) === x` true for an iterator this realm made: one that already has
 * the helpers needs no wrapper, and wrapping it would hand back a second object with the same
 * behaviour and a different identity. */
static int kiln_c99_iterator_on_prototype(kiln_value value) {
    kiln_value current = value;
    size_t steps;
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        if (current.tag == KILN_VALUE_BUILTIN) {
            return current.bits == KILN_C99_BUILTIN_ITERATOR_PROTOTYPE;
        }
        if (current.tag != KILN_VALUE_OBJECT) {
            return 0;
        }
        current = kiln_c99_object_of(current)->prototype;
    }
    return 0;
}

/* `GetIteratorDirect`: the `next` this iterator will be stepped with, read once.
 *
 * Once, and kept. A helper that looked `next` up again on every step would follow a program that
 * replaced it midway, which is neither what the specification describes nor what a caller
 * building a pipeline expects -- and it is what the interpreter does. */
static int kiln_c99_iterator_direct(kiln_host *host, kiln_value source, kiln_value *next,
                                    int depth, kiln_value *thrown) {
    if (!kiln_c99_is_iterator_receiver(source)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Method called on an incompatible receiver");
    }
    return kiln_c99_get(host, source, "next", 4, next, depth, thrown);
}

/* One step of `iterator` through the `next` it was captured with. */
static int kiln_c99_iterator_advance(kiln_host *host, kiln_value iterator, kiln_value next,
                                     kiln_value *value, int *finished, int depth,
                                     kiln_value *thrown) {
    kiln_value step;
    kiln_value done;
    int status;
    if (!kiln_c99_is_callable(next)) {
        return kiln_c99_throw_not_a(thrown, next, "function");
    }
    status = kiln_c99_call(host, &step, next, iterator, NULL, 0, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* `{ value, done }` has to be an object. A `next` answering a primitive has broken the
     * protocol, and the language reports that rather than reading `done` off a number. */
    if (step.tag != KILN_VALUE_OBJECT && step.tag != KILN_VALUE_PROXY &&
        step.tag != KILN_VALUE_ARRAY) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Iterator result is not an object");
    }
    status = kiln_c99_get(host, step, "done", 4, &done, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (kiln_c99_truthy(done)) {
        /* The value that comes back with `done` is the iterator's *return* value, which none of
         * the helpers passes on. */
        *finished = 1;
        *value = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    *finished = 0;
    return kiln_c99_get(host, step, "value", 5, value, depth, thrown);
}

/* `IteratorClose`: give the source its chance to release whatever it was holding. */
static int kiln_c99_iterator_close(kiln_host *host, kiln_value iterator, int depth,
                                   kiln_value *thrown) {
    kiln_value method;
    kiln_value ignored;
    int status;
    if (!kiln_c99_is_iterator_receiver(iterator)) {
        return KILN_C99_OK;
    }
    status = kiln_c99_get(host, iterator, "return", 6, &method, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (method.tag == KILN_VALUE_UNDEFINED || method.tag == KILN_VALUE_NULL) {
        return KILN_C99_OK;
    }
    if (!kiln_c99_is_callable(method)) {
        return kiln_c99_throw_not_a(thrown, method, "function");
    }
    return kiln_c99_call(host, &ignored, method, iterator, NULL, 0, depth, thrown);
}

/* Close a source when the close is not the interesting failure.
 *
 * Either a throw is already in flight or one is about to be raised, and the specification keeps
 * that one: what `return` complains about while being told the iteration is over says less than
 * the reason it ended. The value in flight is parked for the duration, because closing is itself
 * a call and a throw from inside `return` would otherwise take the one slot the original failure
 * is riding in. */
static void kiln_c99_iterator_close_discarding(kiln_host *host, kiln_value iterator, int depth,
                                               kiln_value *thrown) {
    kiln_value pending;
    if (thrown == NULL) {
        (void)kiln_c99_iterator_close(host, iterator, depth, NULL);
        return;
    }
    pending = *thrown;
    (void)kiln_c99_iterator_close(host, iterator, depth, thrown);
    *thrown = pending;
}

/* `{ value, done }`, which is the only thing any of this hands back to a program. */
static int kiln_c99_iteration_result(kiln_host *host, kiln_value *out, kiln_value value,
                                     int finished, int depth, kiln_value *thrown) {
    kiln_value step;
    int status = kiln_c99_new_object(&step);
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, step, "value", 5, value, depth, thrown);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, step, "done", 4, kiln_c99_boolean(finished), depth, thrown);
    }
    if (status == KILN_C99_OK) {
        *out = step;
    }
    return status;
}

/* The object every lazy helper is, and the wrapper `Iterator.from` builds -- one shape, told
 * apart by the code in `@@iterhelper`. Nothing is walked here: building a helper must not advance
 * its source by a single element, which is what `drop(3)` deferring its skip to the first `next`
 * is about. */
static int kiln_c99_iterator_new_helper(kiln_value *out, uint64_t kind, kiln_value source,
                                        kiln_value next, kiln_value argument) {
    kiln_value helper;
    int status = kiln_c99_new_object(&helper);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_own_data(helper, "@@iterhelper", 12, kiln_c99_number((double)kind));
    if (status == KILN_C99_OK) {
        status = kiln_c99_own_data(helper, "@@itersource", 12, source);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_own_data(helper, "@@iternext", 10, next);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_own_data(helper, "@@iterarg", 9, argument);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_own_data(helper, "@@itercount", 11, kiln_c99_number(0.0));
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_own_data(helper, "@@iterdone", 10, kiln_c99_boolean(0));
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_own_data(helper, "@@iterinner", 11, kiln_c99_undefined());
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_own_data(helper, "@@iterinnernext", 15, kiln_c99_undefined());
    }
    /* Own and non-enumerable, exactly as the interpreter leaves them: `next` and `return` are the
     * helper's, and `@@iterator` is what lets one be spread or walked by `for...of`. */
    if (status == KILN_C99_OK) {
        status = kiln_c99_define_method(helper, "next", 4,
                                        kiln_c99_method(KILN_C99_METHOD_ITERATOR_HELPER_NEXT));
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_define_method(helper, "return", 6,
                                        kiln_c99_method(KILN_C99_METHOD_ITERATOR_HELPER_RETURN));
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_define_method(helper, "@@iterator", 10,
                                        kiln_c99_method(KILN_C99_METHOD_ITERATOR_SELF));
    }
    /* Linked last, so the writes above never walk a chain that ends in a method table. */
    if (status == KILN_C99_OK) {
        status = kiln_c99_set_prototype(helper,
                                        kiln_c99_builtin(KILN_C99_BUILTIN_ITERATOR_PROTOTYPE));
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    *out = helper;
    return KILN_C99_OK;
}

/* One step of a helper's source, latching the helper done at either kind of ending.
 *
 * Either kind: a helper whose source threw is as finished as one that ran out, and a second
 * `next` on it must answer done rather than reach back into an iterator that is broken. */
static int kiln_c99_iterator_helper_step(kiln_host *host, kiln_value helper, kiln_value *value,
                                         int *finished, int depth, kiln_value *thrown) {
    kiln_value source = kiln_c99_iterator_state(helper, "@@itersource", 12);
    kiln_value next = kiln_c99_iterator_state(helper, "@@iternext", 10);
    int status = kiln_c99_iterator_advance(host, source, next, value, finished, depth, thrown);
    if (status != KILN_C99_OK || *finished) {
        (void)kiln_c99_own_data(helper, "@@iterdone", 10, kiln_c99_boolean(1));
    }
    return status;
}

/* A helper giving up on its source, whatever went wrong.
 *
 * The helper has already taken an element out of the source, so it owns what happens to it: the
 * specification closes the underlying iterator before letting the failure out. */
static void kiln_c99_iterator_helper_abandon(kiln_host *host, kiln_value helper, int depth,
                                             kiln_value *thrown) {
    (void)kiln_c99_own_data(helper, "@@iterdone", 10, kiln_c99_boolean(1));
    kiln_c99_iterator_close_discarding(host, kiln_c99_iterator_state(helper, "@@itersource", 12),
                                       depth, thrown);
}

/* `Iterator.from(value)`.
 *
 * Two things it is not. It is not `Array.from`: nothing is walked, and an endless iterator passes
 * through unchanged. And it is not a wrapper in the common case -- an iterator that already
 * inherits the helpers is handed straight back.
 *
 * A value with no `@@iterator` is taken to *be* an iterator if it has a `next`, which is the
 * whole point of the function: it is how an object that only ever implemented the iterator
 * protocol gets the helper methods. It is the member a bundled `immer` reads. */
static int kiln_c99_iterator_from(kiln_host *host, kiln_value *result, kiln_value value,
                                  int depth, kiln_value *thrown) {
    kiln_value method;
    kiln_value iterator;
    kiln_value next;
    int status;
    if (value.tag == KILN_VALUE_STRING) {
        /* The one primitive the specification lets through, walked as its *characters*. This
         * backend walks a string through `kiln_c99_iterate` rather than through an `@@iterator`
         * that could be stepped, so the characters are collected once and handed on as an
         * ordinary array iterator -- a string is finite by construction, so laziness buys
         * nothing here and the shape a program sees is the same. */
        kiln_value characters;
        status = kiln_c99_new_array(&characters);
        if (status == KILN_C99_OK) {
            status = kiln_c99_iterate(host, characters, value, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_new_iterator(&iterator, characters);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_get(host, iterator, "next", 4, &next, depth, thrown);
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_iterator_new_helper(result, 0, iterator, next, kiln_c99_undefined());
    }
    if (value.tag == KILN_VALUE_UNDEFINED || value.tag == KILN_VALUE_NULL ||
        value.tag == KILN_VALUE_BOOLEAN || value.tag == KILN_VALUE_NUMBER ||
        value.tag == KILN_VALUE_BIGINT || value.tag == KILN_VALUE_SYMBOL) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Iterator.from called on a value that is not iterable");
    }
    status = kiln_c99_probe_symbol(host, value, "@@iterator", 10, &method, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (method.tag == KILN_VALUE_UNDEFINED || method.tag == KILN_VALUE_NULL) {
        iterator = value;
    } else if (kiln_c99_is_callable(method)) {
        status = kiln_c99_call(host, &iterator, method, value, NULL, 0, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    } else {
        return kiln_c99_throw_not_a(thrown, method, "function");
    }
    status = kiln_c99_iterator_direct(host, iterator, &next, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (kiln_c99_iterator_on_prototype(iterator)) {
        *result = iterator;
        return KILN_C99_OK;
    }
    return kiln_c99_iterator_new_helper(result, 0, iterator, next, kiln_c99_undefined());
}

/* `next` on a helper, and on the wrapper `Iterator.from` builds.
 *
 * One function for the whole family, because the shapes differ only in what happens between
 * taking an element from the source and handing one on -- and because a helper's kind is a value
 * in the object rather than a code of its own, which is what keeps the family inside one method
 * ID. */
static int kiln_c99_iterator_helper_next(kiln_host *host, kiln_value *result, kiln_value receiver,
                                         int depth, kiln_value *thrown) {
    uint64_t kind = 0;
    kiln_value element = kiln_c99_undefined();
    kiln_value produced = kiln_c99_undefined();
    int finished = 0;
    int status;
    if (!kiln_c99_is_iterator_helper(receiver, &kind)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Method called on an incompatible receiver");
    }
    if (kiln_c99_truthy(kiln_c99_iterator_state(receiver, "@@iterdone", 10))) {
        return kiln_c99_iteration_result(host, result, kiln_c99_undefined(), 1, depth, thrown);
    }
    if (kind == KILN_C99_METHOD_ITERATOR_TAKE) {
        /* The count is spent *before* the source is asked, so `take(0)` never touches it at all
         * -- and once it runs out the source is closed rather than abandoned. */
        double remaining = kiln_c99_as_number(kiln_c99_iterator_state(receiver, "@@iterarg", 9));
        if (!(remaining > 0.0)) {
            status = kiln_c99_own_data(receiver, "@@iterdone", 10, kiln_c99_boolean(1));
            if (status != KILN_C99_OK) {
                return status;
            }
            /* Propagated, not discarded: nothing failed here, so a `return` that objects is the
             * only thing that went wrong and the caller has to hear it. */
            status = kiln_c99_iterator_close(
                host, kiln_c99_iterator_state(receiver, "@@itersource", 12), depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            return kiln_c99_iteration_result(host, result, kiln_c99_undefined(), 1, depth, thrown);
        }
        status = kiln_c99_own_data(receiver, "@@iterarg", 9, kiln_c99_number(remaining - 1.0));
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    if (kind == KILN_C99_METHOD_ITERATOR_DROP) {
        /* The skipped elements are pulled on the first `next` rather than at construction:
         * `drop` is lazy too, and building one must not advance anything. */
        for (;;) {
            double remaining =
                kiln_c99_as_number(kiln_c99_iterator_state(receiver, "@@iterarg", 9));
            if (!(remaining > 0.0)) {
                break;
            }
            status = kiln_c99_own_data(receiver, "@@iterarg", 9, kiln_c99_number(remaining - 1.0));
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_iterator_helper_step(host, receiver, &element, &finished, depth,
                                                   thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (finished) {
                return kiln_c99_iteration_result(host, result, kiln_c99_undefined(), 1, depth,
                                                 thrown);
            }
        }
    }
    for (;;) {
        double position;
        kiln_value callback;
        if (kind == KILN_C99_METHOD_ITERATOR_FLAT_MAP) {
            /* An inner iterator in flight is drained before the outer one is asked again, which
             * is what makes `flatMap` lazy in both directions: neither the outer source nor an
             * inner one is walked further than the element being handed on. */
            kiln_value inner = kiln_c99_iterator_state(receiver, "@@iterinner", 11);
            if (inner.tag != KILN_VALUE_UNDEFINED) {
                kiln_value drained =
                    kiln_c99_iterator_state(receiver, "@@iterinnernext", 15);
                status = kiln_c99_iterator_advance(host, inner, drained, &produced, &finished,
                                                   depth, thrown);
                if (status != KILN_C99_OK) {
                    kiln_c99_iterator_helper_abandon(host, receiver, depth, thrown);
                    return status;
                }
                if (!finished) {
                    return kiln_c99_iteration_result(host, result, produced, 0, depth, thrown);
                }
                status = kiln_c99_own_data(receiver, "@@iterinner", 11, kiln_c99_undefined());
                if (status == KILN_C99_OK) {
                    status = kiln_c99_own_data(receiver, "@@iterinnernext", 15,
                                               kiln_c99_undefined());
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
        }
        status = kiln_c99_iterator_helper_step(host, receiver, &element, &finished, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (finished) {
            return kiln_c99_iteration_result(host, result, kiln_c99_undefined(), 1, depth, thrown);
        }
        /* The three that pass an element through untouched: the `Iterator.from` wrapper, and
         * `take` and `drop` once their counts have been settled above. */
        if (kind == 0 || kind == KILN_C99_METHOD_ITERATOR_TAKE ||
            kind == KILN_C99_METHOD_ITERATOR_DROP) {
            return kiln_c99_iteration_result(host, result, element, 0, depth, thrown);
        }
        position = kiln_c99_as_number(kiln_c99_iterator_state(receiver, "@@itercount", 11));
        status = kiln_c99_own_data(receiver, "@@itercount", 11, kiln_c99_number(position + 1.0));
        if (status != KILN_C99_OK) {
            return status;
        }
        callback = kiln_c99_iterator_state(receiver, "@@iterarg", 9);
        {
            kiln_value visit[2];
            visit[0] = element;
            visit[1] = kiln_c99_number(position);
            status = kiln_c99_call(host, &produced, callback, kiln_c99_undefined(), visit, 2,
                                   depth, thrown);
        }
        if (status != KILN_C99_OK) {
            kiln_c99_iterator_helper_abandon(host, receiver, depth, thrown);
            return status;
        }
        if (kind == KILN_C99_METHOD_ITERATOR_MAP) {
            return kiln_c99_iteration_result(host, result, produced, 0, depth, thrown);
        }
        if (kind == KILN_C99_METHOD_ITERATOR_FILTER) {
            if (kiln_c99_truthy(produced)) {
                /* The element, not the predicate's answer. */
                return kiln_c99_iteration_result(host, result, element, 0, depth, thrown);
            }
            continue;
        }
        {
            /* `GetIteratorFlattenable`: what `flatMap`'s mapper is allowed to answer with.
             *
             * A primitive is refused rather than wrapped, and a *string* is the case the rule
             * exists for: a string is iterable, so accepting one would silently flatten a
             * returned word into its characters. */
            kiln_value inner = kiln_c99_undefined();
            kiln_value inner_next = kiln_c99_undefined();
            kiln_value inner_method = kiln_c99_undefined();
            if (!kiln_c99_is_iterator_receiver(produced)) {
                kiln_c99_iterator_helper_abandon(host, receiver, depth, thrown);
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Method called on an incompatible receiver");
            }
            status = kiln_c99_probe_symbol(host, produced, "@@iterator", 10, &inner_method, depth, thrown);
            if (status == KILN_C99_OK) {
                if (inner_method.tag == KILN_VALUE_UNDEFINED ||
                    inner_method.tag == KILN_VALUE_NULL) {
                    /* No `@@iterator`, so the value is taken to *be* an iterator if it has a
                     * `next`; `kiln_c99_iterator_direct` below is what decides. */
                    inner = produced;
                } else if (kiln_c99_is_callable(inner_method)) {
                    status = kiln_c99_call(host, &inner, inner_method, produced, NULL, 0, depth,
                                           thrown);
                } else {
                    status = kiln_c99_throw_not_a(thrown, inner_method, "function");
                }
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_iterator_direct(host, inner, &inner_next, depth, thrown);
            }
            if (status != KILN_C99_OK) {
                kiln_c99_iterator_helper_abandon(host, receiver, depth, thrown);
                return status;
            }
            status = kiln_c99_own_data(receiver, "@@iterinner", 11, inner);
            if (status == KILN_C99_OK) {
                status = kiln_c99_own_data(receiver, "@@iterinnernext", 15, inner_next);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
}

/* `return` on a helper: an early `break` out of a `for...of` reaches this, and it is what makes
 * the close travel down the pipeline to the source that actually holds something. */
static int kiln_c99_iterator_helper_return(kiln_host *host, kiln_value *result,
                                           kiln_value receiver, int depth, kiln_value *thrown) {
    int already;
    int status;
    if (!kiln_c99_is_iterator_helper(receiver, NULL)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Method called on an incompatible receiver");
    }
    already = kiln_c99_truthy(kiln_c99_iterator_state(receiver, "@@iterdone", 10));
    status = kiln_c99_own_data(receiver, "@@iterdone", 10, kiln_c99_boolean(1));
    if (status != KILN_C99_OK) {
        return status;
    }
    /* A helper that already finished does not reach back into its source: the source closed
     * itself when it ran out, and calling `return` on it again is a second close.
     *
     * ONLY THE SOURCE, not a `flatMap` inner still in flight. The specification closes both, and
     * `kiln-interpreter`'s `iterator_helper_return` closes only the source -- so closing both
     * here would make the two engines disagree on an inner whose `return` is observable, which
     * is worse than the gap. The fix belongs in the interpreter first; this follows it. */
    if (!already) {
        status = kiln_c99_iterator_close(
            host, kiln_c99_iterator_state(receiver, "@@itersource", 12), depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    return kiln_c99_iteration_result(host, result, kiln_c99_undefined(), 1, depth, thrown);
}

/* `map`, `filter`, `take`, `drop` and `flatMap`.
 *
 * Nothing is walked. The result is a new iterator holding on to this one, which it pulls a single
 * element from each time it is stepped -- the whole point of the family, and the reason
 * `endless.map(f).take(3)` terminates. */
static int kiln_c99_iterator_lazy(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown) {
    kiln_value argument = argc > 0 ? args[0] : kiln_c99_undefined();
    kiln_value stored = kiln_c99_undefined();
    kiln_value next = kiln_c99_undefined();
    int status;
    if (!kiln_c99_is_iterator_receiver(receiver)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Method called on an incompatible receiver");
    }
    /* The argument is settled *before* `next` is so much as read, and a bad one closes the
     * iterator on the way out. Both halves are observable: a test watching the `next` getter
     * asserts it never ran, and one watching `return` asserts it did. */
    if (code == KILN_C99_METHOD_ITERATOR_TAKE || code == KILN_C99_METHOD_ITERATOR_DROP) {
        double limit = 0.0;
        status = kiln_c99_to_number_checked(host, argument, &limit, depth, thrown);
        if (status != KILN_C99_OK) {
            kiln_c99_iterator_close_discarding(host, receiver, depth, thrown);
            return status;
        }
        /* Written as `!(limit >= 0.0)` so NaN falls in: it is out of range rather than rounded
         * towards something plausible, which is what the interpreter answers too. */
        if (!(limit >= 0.0)) {
            kiln_c99_iterator_close_discarding(host, receiver, depth, thrown);
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "Invalid iterator limit");
        }
        /* `take(Infinity)` survives truncation and means "never runs out", which is the answer a
         * program asking for it wants. */
        stored = kiln_c99_number(trunc(limit));
    } else if (kiln_c99_is_callable(argument)) {
        stored = argument;
    } else {
        kiln_c99_iterator_close_discarding(host, receiver, depth, thrown);
        return kiln_c99_throw_not_a(thrown, argument, "function");
    }
    status = kiln_c99_iterator_direct(host, receiver, &next, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    return kiln_c99_iterator_new_helper(result, code, receiver, next, stored);
}

/* `Iterator.prototype.includes(searched, skippedElements?)`.
 *
 * Its own function rather than an arm of the walk below, for two reasons the interpreter draws
 * the same line on: it compares a *value* where every other terminal calls a function, and it
 * takes a second argument. That second argument is why this is not a three-line arm -- ignoring
 * it would make `it.includes(x, 2)` answer about the wrong elements, silently, which is the one
 * kind of wrong answer this backend exists to avoid. */
static int kiln_c99_iterator_includes(kiln_host *host, kiln_value *result, kiln_value receiver,
                                      const kiln_value *args, size_t argc, int depth,
                                      kiln_value *thrown) {
    kiln_value searched = argc > 0 ? args[0] : kiln_c99_undefined();
    kiln_value skip_argument = argc > 1 ? args[1] : kiln_c99_undefined();
    kiln_value next = kiln_c99_undefined();
    double to_skip = 0.0;
    double skipped = 0.0;
    int status;
    /* Settled before `next` is read, and a bad one closes the source on the way out -- the same
     * order every argument in this family is checked in. No coercion: the count names a position,
     * and a position read out of whatever the program happened to pass would be a guess. */
    if (skip_argument.tag != KILN_VALUE_UNDEFINED) {
        double asked = kiln_c99_as_number(skip_argument);
        /* Both infinities are admitted where an integral check alone would refuse them: "skip
         * everything" is a coherent instruction, and the negative one is refused one step below
         * as a count rather than as a type. */
        if (skip_argument.tag != KILN_VALUE_NUMBER ||
            !(isinf(asked) || asked == trunc(asked))) {
            kiln_c99_iterator_close_discarding(host, receiver, depth, thrown);
            return kiln_c99_throw_error(
                thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                "skippedElements must be an integral Number or an infinity");
        }
        to_skip = asked;
    }
    if (to_skip < 0.0 || (isfinite(to_skip) && to_skip > 9007199254740991.0)) {
        kiln_c99_iterator_close_discarding(host, receiver, depth, thrown);
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Invalid iterator limit");
    }
    status = kiln_c99_iterator_direct(host, receiver, &next, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* Counted rather than looped `to_skip` times: the count may be 2**53 or an infinity, and the
     * source running out is what actually ends the skip. */
    while (skipped < to_skip) {
        kiln_value element = kiln_c99_undefined();
        int finished = 0;
        status = kiln_c99_iterator_advance(host, receiver, next, &element, &finished, depth,
                                           thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (finished) {
            *result = kiln_c99_boolean(0);
            return KILN_C99_OK;
        }
        skipped += 1.0;
    }
    for (;;) {
        kiln_value element = kiln_c99_undefined();
        int finished = 0;
        status = kiln_c99_iterator_advance(host, receiver, next, &element, &finished, depth,
                                           thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (finished) {
            /* Exhaustion is not an early exit, so there is nothing to close: the source ended on
             * its own and `return` is not even looked up. */
            *result = kiln_c99_boolean(0);
            return KILN_C99_OK;
        }
        /* SameValueZero, so `NaN` is found and `-0` matches `0`. */
        if (kiln_c99_same_value_zero(element, searched)) {
            status = kiln_c99_iterator_close(host, receiver, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_boolean(1);
            return KILN_C99_OK;
        }
    }
}

/* The methods that *consume* an iterator: `reduce`, `toArray`, `forEach`, `some`, `every`, `find`
 * -- `includes` is answered above.
 *
 * The division is the specification's and it is not cosmetic: these answer a value rather than
 * another iterator, so each drives the source to its end -- or to the element that answers the
 * question, at which point it closes it. Handed an endless source, every one of them is meant to
 * run forever, which is why they are the only place in this file that loops without a bound. */
static int kiln_c99_iterator_terminal(kiln_host *host, kiln_value *result, uint64_t code,
                                      kiln_value receiver, const kiln_value *args, size_t argc,
                                      int depth, kiln_value *thrown) {
    kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
    kiln_value accumulator = kiln_c99_undefined();
    kiln_value collected = kiln_c99_undefined();
    kiln_value next = kiln_c99_undefined();
    double position = 0.0;
    int has_accumulator = 0;
    int status;
    if (!kiln_c99_is_iterator_receiver(receiver)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Method called on an incompatible receiver");
    }
    if (code != KILN_C99_METHOD_ITERATOR_TO_ARRAY && !kiln_c99_is_callable(callback)) {
        kiln_c99_iterator_close_discarding(host, receiver, depth, thrown);
        return kiln_c99_throw_not_a(thrown, callback, "function");
    }
    status = kiln_c99_iterator_direct(host, receiver, &next, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (code == KILN_C99_METHOD_ITERATOR_TO_ARRAY) {
        status = kiln_c99_new_array(&collected);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    if (code == KILN_C99_METHOD_ITERATOR_REDUCE && argc > 1) {
        accumulator = args[1];
        has_accumulator = 1;
    }
    for (;;) {
        kiln_value element = kiln_c99_undefined();
        kiln_value answered = kiln_c99_undefined();
        int finished = 0;
        status = kiln_c99_iterator_advance(host, receiver, next, &element, &finished, depth,
                                           thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (finished) {
            break;
        }
        if (code == KILN_C99_METHOD_ITERATOR_TO_ARRAY) {
            status = kiln_c99_array_push(collected, element);
            if (status != KILN_C99_OK) {
                return status;
            }
            position += 1.0;
            continue;
        }
        if (code == KILN_C99_METHOD_ITERATOR_REDUCE && !has_accumulator) {
            /* No seed, so the first element *is* the seed and the callback never sees it. */
            accumulator = element;
            has_accumulator = 1;
            position += 1.0;
            continue;
        }
        {
            kiln_value visit[3];
            size_t count = 2;
            if (code == KILN_C99_METHOD_ITERATOR_REDUCE) {
                visit[0] = accumulator;
                visit[1] = element;
                visit[2] = kiln_c99_number(position);
                count = 3;
            } else {
                visit[0] = element;
                visit[1] = kiln_c99_number(position);
            }
            status = kiln_c99_call(host, &answered, callback, kiln_c99_undefined(), visit, count,
                                   depth, thrown);
        }
        if (status != KILN_C99_OK) {
            /* The element was already taken from the source, so the source is closed before the
             * callback's failure is let out -- and the failure in flight is the one kept. */
            kiln_c99_iterator_close_discarding(host, receiver, depth, thrown);
            return status;
        }
        position += 1.0;
        if (code == KILN_C99_METHOD_ITERATOR_REDUCE) {
            accumulator = answered;
            continue;
        }
        if (code == KILN_C99_METHOD_ITERATOR_FOR_EACH) {
            continue;
        }
        /* `some`, `every` and `find` each stop at the element that answers the question, and
         * closing the source is part of stopping: the walk ends early, so the iterator is owed
         * the notice it would have got from an early `break`. */
        if ((code == KILN_C99_METHOD_ITERATOR_SOME && kiln_c99_truthy(answered)) ||
            (code == KILN_C99_METHOD_ITERATOR_EVERY && !kiln_c99_truthy(answered)) ||
            (code == KILN_C99_METHOD_ITERATOR_FIND && kiln_c99_truthy(answered))) {
            status = kiln_c99_iterator_close(host, receiver, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = code == KILN_C99_METHOD_ITERATOR_FIND
                          ? element
                          : kiln_c99_boolean(code == KILN_C99_METHOD_ITERATOR_SOME);
            return KILN_C99_OK;
        }
    }
    switch (code) {
    case KILN_C99_METHOD_ITERATOR_TO_ARRAY: *result = collected; return KILN_C99_OK;
    case KILN_C99_METHOD_ITERATOR_SOME: *result = kiln_c99_boolean(0); return KILN_C99_OK;
    case KILN_C99_METHOD_ITERATOR_EVERY: *result = kiln_c99_boolean(1); return KILN_C99_OK;
    case KILN_C99_METHOD_ITERATOR_FIND:
    case KILN_C99_METHOD_ITERATOR_FOR_EACH: *result = kiln_c99_undefined(); return KILN_C99_OK;
    default: break;
    }
    /* `reduce` over an empty source with no seed has nothing to answer with, and the language
     * says so rather than answering undefined. */
    if (!has_accumulator) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Reduce of empty array with no initial value");
    }
    *result = accumulator;
    return KILN_C99_OK;
}

/* The whole 470..484 range, routed by shape rather than by receiver: the dispatcher hands every
 * one of these codes here, because none of them is answered from a tag. */
static int kiln_c99_iterator_method(kiln_host *host, kiln_value *result, uint64_t code,
                                    kiln_value receiver, const kiln_value *args, size_t argc,
                                    int depth, kiln_value *thrown) {
    switch (code) {
    case KILN_C99_METHOD_ITERATOR_FROM:
        return kiln_c99_iterator_from(host, result, argc > 0 ? args[0] : kiln_c99_undefined(),
                                      depth, thrown);
    case KILN_C99_METHOD_ITERATOR_HELPER_NEXT:
        return kiln_c99_iterator_helper_next(host, result, receiver, depth, thrown);
    case KILN_C99_METHOD_ITERATOR_HELPER_RETURN:
        return kiln_c99_iterator_helper_return(host, result, receiver, depth, thrown);
    case KILN_C99_METHOD_ITERATOR_INCLUDES:
        return kiln_c99_iterator_includes(host, result, receiver, args, argc, depth, thrown);
    case KILN_C99_METHOD_ITERATOR_MAP:
    case KILN_C99_METHOD_ITERATOR_FILTER:
    case KILN_C99_METHOD_ITERATOR_TAKE:
    case KILN_C99_METHOD_ITERATOR_DROP:
    case KILN_C99_METHOD_ITERATOR_FLAT_MAP:
        return kiln_c99_iterator_lazy(host, result, code, receiver, args, argc, depth, thrown);
    default:
        return kiln_c99_iterator_terminal(host, result, code, receiver, args, argc, depth,
                                          thrown);
    }
}
