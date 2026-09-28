        case KILN_C99_METHOD_DATE_NOW:
        {
            double milliseconds;
            status = kiln_c99_wall_clock_milliseconds(host, &milliseconds);
            if (status != KILN_C99_OK) {
                return status;
            }
            /* Date time values are integral milliseconds. The host clock deliberately keeps
             * sub-millisecond precision for performance.now(), but Date.now() must discard it. */
            *result = kiln_c99_number(trunc(milliseconds));
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_COLLECTION_GET:
        case KILN_C99_METHOD_COLLECTION_SET:
        case KILN_C99_METHOD_COLLECTION_ADD:
        case KILN_C99_METHOD_COLLECTION_HAS:
        case KILN_C99_METHOD_COLLECTION_DELETE:
        case KILN_C99_METHOD_COLLECTION_CLEAR:
        case KILN_C99_METHOD_COLLECTION_FOR_EACH:
        case KILN_C99_METHOD_COLLECTION_KEYS:
        case KILN_C99_METHOD_COLLECTION_VALUES:
        case KILN_C99_METHOD_COLLECTION_ENTRIES:
        case KILN_C99_METHOD_COLLECTION_SELF: {
            kiln_collection *collection;
            kiln_value first = argc > 0 ? args[0] : kiln_c99_undefined();
            double at;
            /* `Map.prototype.get.call({})`. A collection method reached through a receiver that
             * has no backing collection is a brand check the language fails with a TypeError, not
             * a method this backend is missing -- it has the method, and the receiver is wrong. */
            if (receiver.tag != KILN_VALUE_COLLECTION) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Method called on an incompatible receiver");
            }
            collection = kiln_c99_collection_of(receiver);
            switch (code) {
            case KILN_C99_METHOD_COLLECTION_GET:
                at = kiln_c99_collection_find(collection, first);
                *result = at < 0.0 ? kiln_c99_undefined() : collection->entries[(size_t)at].value;
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_HAS:
                *result = kiln_c99_boolean(kiln_c99_collection_find(collection, first) >= 0.0);
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_SET:
            case KILN_C99_METHOD_COLLECTION_ADD:
                /* A Set stores each value under itself, which is what makes `has` one lookup
                 * rather than two shapes. Both answer with the collection, so they chain. */
                status = kiln_c99_collection_put(receiver, first,
                                                 code == KILN_C99_METHOD_COLLECTION_SET
                                                     ? (argc > 1 ? args[1] : kiln_c99_undefined())
                                                     : first);
                if (status != KILN_C99_OK) {
                    return status;
                }
                *result = receiver;
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_DELETE:
                at = kiln_c99_collection_find(collection, first);
                if (at < 0.0) {
                    *result = kiln_c99_boolean(0);
                    return KILN_C99_OK;
                }
                /* A hole, not a removal: a walk already past this point must not have what
                 * follows shifted under it, and one not yet there must not see this entry
                 * again. `x,z` rather than `x,y,z` when the loop deletes `y` after visiting `x`. */
                collection->entries[(size_t)at].dead = 1;
                collection->live -= 1;
                kiln_c99_collection_hash_remove(collection, (size_t)at, first);
                *result = kiln_c99_boolean(1);
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_CLEAR:
                collection->count = 0;
                collection->live = 0;
                if (collection->hash_table != NULL) {
                    memset(collection->hash_table, 0, collection->hash_capacity * sizeof(uint32_t));
                }
                *result = kiln_c99_undefined();
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_FOR_EACH: {
                /* The callback takes `(value, key, collection)`. A Set passes its value twice,
                 * which is the specification's way of giving both one signature.
                 * Newly added entries during iteration must also be visited per ES spec. */
                for (index = 0; index < collection->count; ++index) {
                    kiln_value visit[3];
                    kiln_value ignored;
                    if (collection->entries[index].dead) {
                        continue;
                    }
                    visit[0] = collection->entries[index].value;
                    visit[1] = collection->entries[index].key;
                    visit[2] = receiver;
                    status = kiln_c99_call(host, &ignored, first,
                                           argc > 1 ? args[1] : kiln_c99_undefined(), visit, 3,
                                           depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                *result = kiln_c99_undefined();
                return KILN_C99_OK;
            }
            default: {
                /* A *live* iterator, holding the collection rather than a copy of it: a delete
                 * during the walk has to be seen, and a snapshot is exactly what cannot see one.
                 * What each step hands back is decided here, where the collection's kind is
                 * known, so the iterator itself needs no reference back to it. */
                int pairs = code == KILN_C99_METHOD_COLLECTION_ENTRIES ||
                            (code == KILN_C99_METHOD_COLLECTION_SELF && collection->is_map);
                int wants_keys = code == KILN_C99_METHOD_COLLECTION_KEYS;
                kiln_value iterator;
                status = kiln_c99_new_object(&iterator);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "@@iterated", 10, receiver, depth,
                                          thrown);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "@@index", 7, kiln_c99_number(0.0),
                                          depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    /* 0 the value, 1 the key, 2 a `[key, value]` pair. */
                    status = kiln_c99_put(host, iterator, "@@part", 6,
                                          kiln_c99_number(pairs ? 2.0 : (wants_keys ? 1.0 : 0.0)),
                                          depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "next", 4,
                                          kiln_c99_method(KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT),
                                          depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "@@iterator", 10,
                                          kiln_c99_method(KILN_C99_METHOD_ITERATOR_SELF), depth,
                                          thrown);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                *result = iterator;
                return KILN_C99_OK;
            }
            }
        }
        case KILN_C99_METHOD_THEN:
        case KILN_C99_METHOD_CATCH:
        case KILN_C99_METHOD_FINALLY: {
            kiln_value on_fulfilled = kiln_c99_undefined();
            kiln_value on_rejected = kiln_c99_undefined();
            /* `catch(r)` is `Invoke(this, "then", [undefined, r])` -- literally, including the
             * read -- so a thenable that is not a promise is handled by its *own* `then`, and a
             * subclass that overrides `then` has that one run. Only `catch` delegates: `then`
             * itself needs a promise to attach a reaction to, and `finally` needs a pair of
             * closures over the receiver that this backend has no way to make for a thenable. */
            if (code == KILN_C99_METHOD_CATCH && receiver.tag != KILN_VALUE_PROMISE) {
                kiln_value method;
                kiln_value forwarded[2];
                status = kiln_c99_get(host, receiver, "then", 4, &method, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (!kiln_c99_is_callable(method)) {
                    return kiln_c99_throw_not_a(thrown, method, "function");
                }
                forwarded[0] = kiln_c99_undefined();
                forwarded[1] = argc > 0 ? args[0] : kiln_c99_undefined();
                return kiln_c99_call(host, result, method, receiver, forwarded, 2, depth, thrown);
            }
            /* `Promise.prototype.then.call({})` has no promise to attach a reaction to. The
             * interpreter reports this against the receiver, and the two are compared byte for
             * byte, so the same sentence is built here. */
            if (receiver.tag != KILN_VALUE_PROMISE) {
                return kiln_c99_throw_not_a(thrown, receiver, "function");
            }
            if (code == KILN_C99_METHOD_THEN) {
                on_fulfilled = argc > 0 ? args[0] : kiln_c99_undefined();
                on_rejected = argc > 1 ? args[1] : kiln_c99_undefined();
            } else if (code == KILN_C99_METHOD_CATCH) {
                on_rejected = argc > 0 ? args[0] : kiln_c99_undefined();
            } else {
                /* `finally` runs on both sides and passes the outcome through untouched, which is
                 * what separates it from `then(f, f)`. Each side is the handler wrapped in a step
                 * that restores what it was given -- bound to the handler itself, so the pair
                 * needs no state object of its own. */
                kiln_value handler = argc > 0 ? args[0] : kiln_c99_undefined();
                status = kiln_c99_bind(&on_fulfilled,
                                       kiln_c99_method(KILN_C99_METHOD_FINALLY_PASS), handler);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_bind(&on_rejected,
                                           kiln_c99_method(KILN_C99_METHOD_FINALLY_RAISE),
                                           handler);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            return kiln_c99_then(receiver, on_fulfilled, on_rejected, result);
        }
        case KILN_C99_METHOD_PROMISE_RESOLVE:
        case KILN_C99_METHOD_PROMISE_REJECT: {
            kiln_value value = argc > 0 ? args[0] : kiln_c99_undefined();
            /* `Promise.resolve(p)` is `p`, not a promise of a promise -- which is what lets a
             * function take "a value or a promise of one" without asking which it got. */
            if (code == KILN_C99_METHOD_PROMISE_RESOLVE && value.tag == KILN_VALUE_PROMISE &&
                receiver.tag != KILN_VALUE_FUNCTION) {
                *result = value;
                return KILN_C99_OK;
            }
            status = kiln_c99_new_promise(result);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (receiver.tag == KILN_VALUE_FUNCTION) {
                kiln_value proto;
                kiln_value tally_val;
                if (kiln_c99_get(host, receiver, "prototype", 9, &proto, depth, thrown) == KILN_C99_OK) {
                    (void)kiln_c99_set_prototype(*result, proto);
                }
                if (kiln_c99_get(host, receiver, "tally", 5, &tally_val, depth, thrown) == KILN_C99_OK &&
                    tally_val.tag == KILN_VALUE_NUMBER) {
                    (void)kiln_c99_put(host, receiver, "tally", 5,
                                       kiln_c99_number(kiln_c99_as_number(tally_val) + 1.0),
                                       0, thrown);
                }
            }
            return kiln_c99_settle(*result, value, code == KILN_C99_METHOD_PROMISE_REJECT);
        }
        case KILN_C99_METHOD_PROMISE_WITH_RESOLVERS: {
            kiln_value promise;
            kiln_value resolve;
            kiln_value reject;
            status = kiln_c99_new_promise(&promise);
            if (status == KILN_C99_OK)
                status = kiln_c99_new_settler(&resolve, promise, 0);
            if (status == KILN_C99_OK)
                status = kiln_c99_new_settler(&reject, promise, 1);
            if (status == KILN_C99_OK)
                status = kiln_c99_new_object(result);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, *result, "promise", 7, promise, 0, NULL);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, *result, "resolve", 7, resolve, 0, NULL);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, *result, "reject", 6, reject, 0, NULL);
            return status;
        }
        case KILN_C99_METHOD_PROMISE_ALL:
        case KILN_C99_METHOD_PROMISE_RACE:
        case KILN_C99_METHOD_PROMISE_ALL_SETTLED:
        case KILN_C99_METHOD_PROMISE_ANY: {
            kiln_value gathered;
            kiln_value derived;
            kiln_array *given;
            /* All four are one machine differing in what each arrival does, which is why the mode
             * travels in the shared state rather than in four copies of the wiring below. */
            double mode = code == KILN_C99_METHOD_PROMISE_RACE          ? 1.0
                          : code == KILN_C99_METHOD_PROMISE_ALL_SETTLED ? 2.0
                          : code == KILN_C99_METHOD_PROMISE_ANY         ? 3.0
                                                                        : 0.0;
            int racing = mode == 1.0;
            status = kiln_c99_new_array(&gathered);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_iterate(host, gathered, argc > 0 ? args[0] : kiln_c99_undefined(),
                                      depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_new_promise(&derived);
            if (status != KILN_C99_OK) {
                return status;
            }
            given = kiln_c99_array_of(gathered);
            if (!racing && given->count == 0) {
                /* Nothing to wait for, so it is already done. `any` is the exception: with no
                 * promise to succeed, every one of them has failed. */
                if (mode == 3.0) {
                    kiln_value aggregate;
                    status = kiln_c99_aggregate_error(&aggregate, gathered);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_settle(derived, aggregate, 1);
                    }
                } else {
                    status = kiln_c99_settle(derived, gathered, 0);
                }
                *result = derived;
                return status;
            }
            {
                /* One shared state object, and one bound step per element that knows its own
                 * index. The index has to travel with the handler, and a handler is one value,
                 * so the pair is an object the step reads itself out of. */
                kiln_value state;
                kiln_value results;
                kiln_value shared_rejecter = kiln_c99_undefined();
                size_t index;
                status = kiln_c99_new_object(&state);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_new_array(&results);
                }
                for (index = 0; index < given->count && status == KILN_C99_OK; ++index) {
                    status = kiln_c99_array_push(results, kiln_c99_undefined());
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@results", 9, results, 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@remaining", 11,
                                          kiln_c99_number((double)given->count), 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@derived", 9, derived, 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@mode", 6, kiln_c99_number(mode), 0,
                                          NULL);
                }
                for (index = 0; index < given->count && status == KILN_C99_OK; ++index) {
                    kiln_value element;
                    kiln_value position = kiln_c99_number((double)index);
                    kiln_value step;
                    kiln_value rejecter;
                    /* Each element's step is the shared state bound as the receiver with its own
                     * index bound as the first argument, so the step is called as
                     * `(index, arrived)` on the state. A slot object carrying the pair cost an
                     * object and a dictionary per element -- about 500 bytes for each of the
                     * hundred thousand tasks a `Promise.all` over a fan-out waits on. */
                    element = kiln_val_to_boundary(given->elements[index]);
                    if (element.tag != KILN_VALUE_PROMISE) {
                        kiln_value immediate;
                        status = kiln_c99_new_promise(&immediate);
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_settle(immediate, element, 0);
                        }
                        element = immediate;
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_bind_with(&step,
                                                    kiln_c99_method(KILN_C99_METHOD_ALL_STEP),
                                                    state, &position, 1);
                    }
                    if (status == KILN_C99_OK) {
                        /* For `all` and `race` a rejection settles the whole thing, whichever
                         * element it came from, and a settler is exactly that. The other two have
                         * something to record instead, so they get a step of their own. */
                        status = mode == 2.0 || mode == 3.0
                                     ? kiln_c99_bind_with(&rejecter,
                                                          kiln_c99_method(KILN_C99_METHOD_REJECT_STEP),
                                                          state, &position, 1)
                                     : shared_rejecter.tag != KILN_VALUE_UNDEFINED
                                           ? (rejecter = shared_rejecter, KILN_C99_OK)
                                           : kiln_c99_new_settler(&rejecter, derived, 1);
                        /* The rejecting side of `all` and `race` is the same settler for every
                         * element -- it rejects the one derived promise -- so it is made once. */
                        if (status == KILN_C99_OK && !(mode == 2.0 || mode == 3.0)) {
                            shared_rejecter = rejecter;
                        }
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_react(element, step, rejecter);
                    }
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = derived;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_FINALLY_PASS:
        case KILN_C99_METHOD_FINALLY_RAISE: {
            kiln_value passed = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value ignored;
            if (kiln_c99_is_callable(receiver)) {
                /* The handler's own failure wins: a `finally` that throws replaces the outcome
                 * it was passing through, which is the one thing that can change it. */
                status = kiln_c99_call(host, &ignored, receiver, kiln_c99_undefined(), NULL, 0,
                                       depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            {
                /* `Promise.resolve(f()).then(() => v)`, which is the specification's own spelling
                 * and the reason it matters: a handler that returns a promise is *waited for*, and
                 * the extra turn that costs is visible in the order things print. Settling here
                 * would put the outcome a turn early. */
                kiln_value awaited;
                kiln_value restore;
                status = kiln_c99_new_promise(&awaited);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_settle(awaited, ignored, 0);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_bind(
                        &restore,
                        kiln_c99_method(code == KILN_C99_METHOD_FINALLY_RAISE
                                            ? KILN_C99_METHOD_FINALLY_RERAISE
                                            : KILN_C99_METHOD_FINALLY_RESTORE),
                        passed);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                return kiln_c99_then(awaited, restore, kiln_c99_undefined(), result);
            }
        }
        case KILN_C99_METHOD_ADOPT_STEP: {
            /* Two settlers bound to the outer promise: whichever the inner one reaches, the outer
             * one follows. */
            if (receiver.tag != KILN_VALUE_PROMISE || argc == 0) {
                KILN_C99_REFUSE("promise adoption was handed something that is not a pair of "
                                "promises");
            }
            if (argc == 1) {
                kiln_promise *inner;
                kiln_reaction adopt;
                if (args[0].tag != KILN_VALUE_PROMISE) {
                    KILN_C99_REFUSE("promise adoption was handed something that is not a pair of "
                                    "promises");
                }
                inner = kiln_c99_promise_of(args[0]);
                status = kiln_c99_promise_observe(inner);
                if (status == KILN_C99_OK)
                    status = kiln_c99_new_settler(&adopt.on_fulfilled, receiver, 0);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_new_settler(&adopt.on_rejected, receiver, 1);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                adopt.derived = kiln_c99_undefined();
                adopt.continuation = 0;
                *result = kiln_c99_undefined();
                if (inner->state != KILN_C99_PENDING) {
                    return kiln_c99_schedule(inner, &adopt);
                }
                return kiln_c99_promise_add_reaction(inner, &adopt);
            }
            /* argc >= 2: thenable adoption: args[0] is `then`, args[1] is `thenable`. */
            kiln_value resolve, reject;
            kiln_value settler_args[2];
            kiln_value call_result = kiln_c99_undefined();
            status = kiln_c99_new_settler(&resolve, receiver, 0);
            if (status == KILN_C99_OK) {
                status = kiln_c99_new_settler(&reject, receiver, 1);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            settler_args[0] = resolve;
            settler_args[1] = reject;
            status = kiln_c99_call(host, &call_result, args[0], args[1], settler_args, 2, depth, thrown);
            if (status == KILN_C99_THROWN) {
                status = kiln_c99_settle(receiver, *thrown, 1);
                if (status != KILN_C99_OK) return status;
            }
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_FINALLY_RESTORE:
            /* The outcome travelled here as the receiver, because a reaction is handed the value
             * it is reacting to and that is the handler's result rather than the original. */
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_FINALLY_RERAISE:
            /* A rejection travels on, which a reaction says by throwing. */
            *thrown = receiver;
            return KILN_C99_THROWN;
        case KILN_C99_METHOD_ALL_STEP:
        case KILN_C99_METHOD_REJECT_STEP: {
            kiln_value state;
            kiln_value at;
            kiln_value results;
            kiln_value remaining;
            kiln_value derived;
            kiln_value mode;
            /* Bound by `Promise.all` and its siblings: the receiver is the shared state and the
             * first argument this element's index, ahead of the value that arrived. */
            kiln_value arrived = argc > 1 ? args[1] : kiln_c99_undefined();
            int rejected = code == KILN_C99_METHOD_REJECT_STEP;
            if (receiver.tag != KILN_VALUE_OBJECT || argc == 0 || args[0].tag != KILN_VALUE_NUMBER) {
                /* Another microtask with no instruction of its own to blame. The state and the
                 * index are bound by `Promise.all` and its siblings, so anything else means a
                 * combinator's bookkeeping went missing rather than a program did anything. */
                KILN_C99_REFUSE("a promise combinator's per-element step was not bound to the "
                                "state and index it builds");
            }
            state = receiver;
            at = args[0];
            status = kiln_c99_get(host, state, "@@results", 9, &results, depth, thrown);
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@remaining", 11, &remaining, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@derived", 9, &derived, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@mode", 6, &mode, depth, thrown);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_undefined();
            if (!rejected && (kiln_c99_as_number(mode) == 1.0 || kiln_c99_as_number(mode) == 3.0)) {
                /* `race` and `any` are both settled by the first arrival that counts, and the
                 * rest are ignored -- which `settle` already does by refusing a second
                 * settlement. For `any` only a fulfilment counts, which is what puts this on the
                 * fulfilment side alone. */
                return kiln_c99_settle(derived, arrived, 0);
            }
            if (results.tag == KILN_VALUE_ARRAY) {
                kiln_array *array = kiln_c99_array_of(results);
                size_t slot = (size_t)kiln_c99_as_number(at);
                kiln_value recorded = arrived;
                if (kiln_c99_as_number(mode) == 2.0) {
                    /* `allSettled` reports the outcome as well as the value, so every element
                     * gets a record rather than a bare result. */
                    status = kiln_c99_new_object(&recorded);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(host, recorded, "status", 6,
                                              kiln_c99_copy_text(rejected ? "rejected"
                                                                          : "fulfilled",
                                                                 rejected ? 8 : 9),
                                              depth, thrown);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(host, recorded, rejected ? "reason" : "value",
                                              rejected ? 6 : 5, arrived, depth, thrown);
                    }
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                if (slot < array->count) {
                    array->elements[slot] = kiln_val_from_boundary(recorded);
                }
            }
            {
                double left = kiln_c99_as_number(remaining) - 1.0;
                status = kiln_c99_put(host, state, "@@remaining", 11, kiln_c99_number(left), depth,
                                      thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (left <= 0.0) {
                    if (kiln_c99_as_number(mode) == 3.0) {
                        /* Every one of them failed, and the reasons are what the caller gets. */
                        kiln_value aggregate;
                        status = kiln_c99_aggregate_error(&aggregate, results);
                        if (status != KILN_C99_OK) {
                            return status;
                        }
                        return kiln_c99_settle(derived, aggregate, 1);
                    }
                    return kiln_c99_settle(derived, results, 0);
                }
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ASYNC_RESUME:
        case KILN_C99_METHOD_ASYNC_THROW:
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                /* `@@resume` and `@@throw` are bound to the suspended frame that awaited, and
                 * they run a whole turn later -- so this refusal, like the two above, arrives
                 * with no instruction behind it and must say for itself what it wanted. */
                KILN_C99_REFUSE("an await resumed against something that is not a suspended frame");
            }
            *result = kiln_c99_undefined();
            /* The same two handlers serve both drivers: what an arrival means depends on the
             * frame it is arriving at, not on who attached them. */
            return kiln_c99_generator_of(receiver)->async_generator
                       ? kiln_c99_async_generator_step(
                             host, receiver, argc > 0 ? args[0] : kiln_c99_undefined(),
                             code == KILN_C99_METHOD_ASYNC_THROW, depth, thrown)
                       : kiln_c99_async_step(host, receiver,
                                             argc > 0 ? args[0] : kiln_c99_undefined(),
                                             code == KILN_C99_METHOD_ASYNC_THROW, depth, thrown,
                                             NULL, 0);
        case KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT: {
            kiln_value source;
            kiln_value at;
            kiln_value part;
            kiln_value step;
            kiln_value produced = kiln_c99_undefined();
            kiln_collection *collection;
            size_t position;
            int finished;
            if (receiver.tag != KILN_VALUE_OBJECT) {
                KILN_C99_REFUSE("a collection iterator whose state is not the object it builds");
            }
            status = kiln_c99_get(host, receiver, "@@iterated", 10, &source, depth, thrown);
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, receiver, "@@index", 7, &at, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, receiver, "@@part", 6, &part, depth, thrown);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            if (source.tag != KILN_VALUE_COLLECTION) {
                KILN_C99_REFUSE("a collection iterator whose source is not a collection");
            }
            collection = kiln_c99_collection_of(source);
            position = (size_t)kiln_c99_as_number(at);
            /* Past every dead entry: one deleted ahead of the cursor is gone, and one deleted
             * behind it has already been handed over. */
            while (position < collection->count && collection->entries[position].dead) {
                position += 1;
            }
            finished = position >= collection->count;
            if (!finished) {
                double which = kiln_c99_as_number(part);
                if (which == 2.0) {
                    status = kiln_c99_new_array(&produced);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(produced,
                                                     collection->entries[position].key);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(produced,
                                                     collection->entries[position].value);
                    }
                } else {
                    produced = which == 1.0 ? collection->entries[position].key
                                            : collection->entries[position].value;
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, receiver, "@@index", 7,
                                          kiln_c99_number((double)(position + 1)), depth, thrown);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            status = kiln_c99_new_object(&step);
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(host, step, "value", 5, produced, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(host, step, "done", 4, kiln_c99_boolean(finished), depth,
                                      thrown);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = step;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ITERATOR_NEXT: {
            kiln_value items;
            kiln_value at;
            kiln_value step;
            kiln_array *array;
            kiln_typed_array *typed;
            kiln_value part_value;
            size_t position;
            int finished;
            int part = 0;
            if (receiver.tag != KILN_VALUE_OBJECT) {
                KILN_C99_REFUSE("an array iterator whose state is not the object it builds");
            }
            status = kiln_c99_get(host, receiver, "@@iterated", 10, &items, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_get(host, receiver, "@@index", 7, &at, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (items.tag != KILN_VALUE_ARRAY && items.tag != KILN_VALUE_TYPED_ARRAY) {
                KILN_C99_REFUSE("an array iterator whose source is not an array or a typed array");
            }
            position = (size_t)kiln_c99_as_number(at);
            if (items.tag == KILN_VALUE_TYPED_ARRAY) {
                typed = kiln_c99_typed_array_of(items);
                finished = position >= typed->length;
            } else {
                array = kiln_c99_array_of(items);
                finished = position >= array->count;
            }
            if (items.tag == KILN_VALUE_TYPED_ARRAY &&
                kiln_c99_get(host, receiver, "@@iterpart", 10, &part_value, depth, thrown) ==
                    KILN_C99_OK) {
                part = (int)kiln_c99_as_number(part_value);
            }
            if (!finished) {
                status = kiln_c99_put(host, receiver, "@@index", 7,
                                      kiln_c99_number((double)(position + 1)), depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            status = kiln_c99_new_object(&step);
            if (status != KILN_C99_OK) {
                return status;
            }
            {
                kiln_value produced = kiln_c99_undefined();
                if (!finished) {
                    if (items.tag == KILN_VALUE_TYPED_ARRAY) {
                        kiln_value element = kiln_c99_undefined();
                        if (part != 2) status = kiln_c99_typed_element(typed, position, &element);
                        produced = part == 2 ? kiln_c99_number((double)position) : element;
                        if (status == KILN_C99_OK && part == 1) {
                            status = kiln_c99_new_array(&produced);
                            if (status == KILN_C99_OK) {
                                status = kiln_c99_array_push(produced, kiln_c99_number((double)position));
                            }
                            if (status == KILN_C99_OK) {
                                status = kiln_c99_array_push(produced, element);
                            }
                        }
                    } else {
                        produced = kiln_val_to_boundary(array->elements[position]);
                    }
                }
                status = status == KILN_C99_OK
                             ? kiln_c99_put(host, step, "value", 5, produced, depth, thrown)
                             : status;
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_put(host, step, "done", 4, kiln_c99_boolean(finished), depth,
                                  thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = step;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_HAS_OWN_PROPERTY: {
            /* `Object.hasOwn(x, k)` is the same question with the receiver moved into the first
             * argument, and it is answered by the same helper so the two cannot drift apart
             * again -- which they had, in four separate ways. */
            int found = 0;
            /* An absent argument is the one spelling the two engines do not agree about: the
             * language asks after the property named "undefined" and the interpreter asks after
             * the one named "". Neither answer may be given here until that is settled, so this
             * keeps saying so. */
            if (argc == 0) {
                KILN_C99_REFUSE("hasOwnProperty with no argument names a property the two engines "
                                "disagree about");
            }
            status = kiln_c99_own_property_present(host, receiver, args[0], &found, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_boolean(found);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_THROW_TYPE_ERROR:
            /* `%ThrowTypeError%`, in full. It is both halves of an unmapped `arguments`
             * object's `callee`, and the message is the one every engine gives. */
            return kiln_c99_throw_error(
                thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                "'caller', 'callee', and 'arguments' properties may not be accessed on strict "
                "mode functions or the arguments objects for calls to them");
        case KILN_C99_METHOD_OBJECT_TO_STRING: {
            /* `Object.prototype.toString`, the oldest type test there is -- and it reports the
             * *kind* rather than converting: `[object Array]`, not `1,2,3`. A library that has to
             * tell an array from an arguments object still reaches for it. */
            kiln_text built = {NULL, 0, 0};
            kiln_value custom = kiln_c99_undefined();
            const char *tag;
            /* `Symbol.toStringTag` overrides the built-in answer, which is the whole reason the
             * symbol exists: it is how a class says what a library's type test should call it.
             * Read through the ordinary chain walk, so an accessor on a prototype runs -- the
             * usual spelling is a getter on the class rather than a field on each instance.
             *
             * Only for an object, because only an object can carry one. A primitive's tag comes
             * from its kind and asking it for a property would be a conversion the operation does
             * not make. */
            if (receiver.tag == KILN_VALUE_OBJECT) {
                int status = kiln_c99_probe_symbol(host, receiver, "@@toStringTag", 13, &custom, depth,
                                          thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            if (custom.tag == KILN_VALUE_STRING) {
                /* The tag is a string *value*, whose pointer names the header:
                 * appending its bytes would serialize the header's metadata. */
                if (kiln_c99_text_append_c(&built, "[object ") != KILN_C99_OK ||
                    kiln_c99_text_append_string(&built, custom) != KILN_C99_OK ||
                    kiln_c99_text_append_c(&built, "]") != KILN_C99_OK) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                *result = kiln_c99_text_value(&built);
                return KILN_C99_OK;
            }
            switch (receiver.tag) {
            case KILN_VALUE_UNDEFINED: tag = "Undefined"; break;
            case KILN_VALUE_NULL: tag = "Null"; break;
            case KILN_VALUE_BOOLEAN: tag = "Boolean"; break;
            case KILN_VALUE_NUMBER: tag = "Number"; break;
            case KILN_VALUE_STRING: tag = "String"; break;
            case KILN_VALUE_ARRAY: tag = "Array"; break;
            case KILN_VALUE_DATE: tag = "Date"; break;
            case KILN_VALUE_REGEXP: tag = "RegExp"; break;
            case KILN_VALUE_PROMISE: tag = "Promise"; break;
            /* Reached through the `Symbol.toStringTag` a generator's prototype carries rather
             * than through a builtinTag, and lands in the same string. The interpreter answers
             * `[object Generator]`; answering `[object Object]` here was a parity divergence. */
            case KILN_VALUE_GENERATOR: tag = "Generator"; break;
            case KILN_VALUE_SYMBOL: tag = "Symbol"; break;
            case KILN_VALUE_BIGINT: tag = "BigInt"; break;
            case KILN_VALUE_ARRAY_BUFFER:
                tag = kiln_c99_typed_buffer_of(receiver)->shared != NULL ? "SharedArrayBuffer" : "ArrayBuffer";
                break;
            case KILN_VALUE_DATA_VIEW: tag = "DataView"; break;
            case KILN_VALUE_TYPED_ARRAY:
                tag = kiln_c99_typed_name(kiln_c99_typed_array_of(receiver)->kind);
                break;
            /* One tag holds both collections, so which one it is comes from the collection
             * rather than from the value tag. */
            case KILN_VALUE_COLLECTION:
                {
                    kiln_collection *collection = kiln_c99_collection_of(receiver);
                    tag = collection->is_weak
                              ? (collection->is_map ? "WeakMap" : "WeakSet")
                              : (collection->is_map ? "Map" : "Set");
                }
                break;
            /* Each of the eight names itself: `[object Temporal.PlainDate]`, which is the
             * `Symbol.toStringTag` the language gives them and the string every type test built
             * on this operation reads. `[object Object]` was the answer before. */
            case KILN_VALUE_TEMPORAL: tag = kiln_c99_temporal_tag(receiver); break;
            case KILN_VALUE_BUILTIN:
                /* `Math` and `JSON` are the two namespaces the language gives a tag of their own,
                 * and they are not functions -- calling them `Function` was the reason
                 * `Object.prototype.toString.call(Math)` disagreed with every other engine. A
                 * constructor really is a function and keeps that answer. */
                tag = receiver.bits == KILN_C99_BUILTIN_MATH   ? "Math"
                      : receiver.bits == KILN_C99_BUILTIN_JSON ? "JSON"
                      : kiln_c99_is_namespace(receiver.bits)   ? "Object"
                                                               : "Function";
                break;
            case KILN_VALUE_FUNCTION:
            case KILN_VALUE_METHOD:
            case KILN_VALUE_BOUND:
            case KILN_VALUE_COMMONJS_REQUIRE:
            case KILN_VALUE_SETTLER: tag = "Function"; break;
            /* An `arguments` object is ordinary in every respect but this one, and this one is
             * exactly what the type test above is asked to tell apart.
             *
             * So is an error, which carries no C-level brand: `@@error` is the reserved slot
             * `kiln_c99_new_error` attaches, and it is what stands in for the [[ErrorData]] the
             * specification brands on. Every error kind reports `Error`, the constructor not
             * being what is asked -- `toString.call(e) === "[object Error]"` is how a library
             * recognises an error it did not construct, across realms where `instanceof` cannot
             * answer. `[object Object]` was the answer before, in both engines; the interpreter
             * was fixed first and this is the other half. */
            case KILN_VALUE_OBJECT:
                tag = kiln_c99_object_of(receiver)->arguments_brand ? "Arguments"
                      : kiln_c99_object_of(receiver)->error_brand   ? "Error"
                                                                    : "Object";
                break;
            default: tag = "Object"; break;
            }
            if (kiln_c99_text_append_c(&built, "[object ") != KILN_C99_OK ||
                kiln_c99_text_append_c(&built, tag) != KILN_C99_OK ||
                kiln_c99_text_append_c(&built, "]") != KILN_C99_OK) {
                free(built.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            *result = kiln_c99_text_value(&built);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING: {
            /* `Invoke(ToObject(this), "toString")`, not the tag itself: a class that overrides
             * `toString` for display is the whole reason the method exists. It was not in this
             * table at all, so `({}).toLocaleString()` was a TypeError about `undefined` -- a
             * missing name reported as a missing function. */
            kiln_value method;
            if (receiver.tag == KILN_VALUE_UNDEFINED || receiver.tag == KILN_VALUE_NULL) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot convert undefined or null to object");
            }
            /* A number's `toLocaleString` is `Intl.NumberFormat` -- `1,234,567.891` for
             * `en-US` -- and answering `toString` printed `1234567.891`, a plausible wrong
             * string. It refuses until `Intl` links on demand (ADR 0063 rule 5); an object's
             * really is `toString`, per the specification. */
            if (receiver.tag == KILN_VALUE_NUMBER || receiver.tag == KILN_VALUE_BIGINT) {
                KILN_C99_REFUSE("Number.prototype.toLocaleString, whose answer is Intl's");
            }
            status = kiln_c99_get(host, receiver, "toString", 8, &method, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            return kiln_c99_call(host, result, method, receiver, NULL, 0, depth, thrown);
        }
        case KILN_C99_METHOD_OBJECT_VALUE_OF:
            /* The receiver unchanged, which is the identity that makes ToPrimitive fall through
             * to `toString` for a plain object.
             *
             * `ToObject(this)` says a primitive receiver answers with its *box*, and there is no
             * wrapper object in this backend to hand back. It cannot be refused here either: a
             * primitive's own `valueOf` -- `"abc".valueOf()` -- is routed to this code rather
             * than to the string one, so refusing would refuse that too. The only way to reach
             * `Object.prototype.valueOf` with a primitive is through `.call`, which this backend
             * refuses on a method receiver already, so the difference is not reachable here. */
            if (receiver.tag == KILN_VALUE_UNDEFINED || receiver.tag == KILN_VALUE_NULL) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot convert undefined or null to object");
            }
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_PRIMITIVE_TO_STRING:
            *result = kiln_c99_text(receiver);
            return KILN_C99_OK;
        case KILN_C99_METHOD_DATE_PARSE: {
            /* Parsing accepts the ISO forms and rejects the rest, rather than guessing: a date
             * a program did not write is worse than no date at all. */
            kiln_value source = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_undefined();
            size_t source_len = 0;
            const char *source_bytes = source.tag == KILN_VALUE_STRING
                                           ? kiln_c99_string_ascii_bytes(source, &source_len)
                                           : NULL;
            *result = kiln_c99_number(source_bytes != NULL
                                          ? kiln_c99_parse_date(source_bytes, source_len)
                                          : 0.0 / 0.0);
            return KILN_C99_OK;
        }
        /* `Date.UTC`, and the two representation operations the shared `Date.prototype`
         * (`kiln-builtins/src/date.ts`) captures at realm start and builds every other member on
         * (ADR 0060, ADR 0065). */
        case KILN_C99_METHOD_DATE_UTC:
        case KILN_C99_METHOD_GET_TIME:
        case KILN_C99_METHOD_SET_TIME: {
            if (code == KILN_C99_METHOD_DATE_UTC) {
                double fields[7];
                for (index = 0; index < 7; ++index) {
                    /* Day defaults to 1 and everything else to 0, which is what makes
                     * `Date.UTC(2026, 7)` the first of August. */
                    fields[index] = index < argc  ? kiln_c99_as_number(args[index])
                                    : index == 2 ? 1.0
                                                 : 0.0;
                }
                *result = kiln_c99_number(kiln_c99_instant_from_parts(fields));
                return KILN_C99_OK;
            }
            /* `Date.prototype.getTime.call({})` has no instant to read. Both engines and node
             * spell this fault the same way, so it is spelled that way here too. */
            if (receiver.tag != KILN_VALUE_DATE) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "this is not a Date object.");
            }
            if (code == KILN_C99_METHOD_GET_TIME) {
                *result = kiln_c99_number(kiln_c99_date_time(receiver));
                return KILN_C99_OK;
            }
            {
                double written = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0 / 0.0;
                *(double *)(uintptr_t)receiver.bits = written;
                *result = kiln_c99_number(written);
                return KILN_C99_OK;
            }
        }
        case KILN_C99_METHOD_ITERATOR_SELF:
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_ARRAY_VALUES: {
            /* Over a snapshot, so a `for...of` that appends to the array it is walking
             * terminates -- which is what the length being read once means. */
            kiln_value snapshot;
            kiln_array *array;
            if (receiver.tag == KILN_VALUE_TYPED_ARRAY) {
                return kiln_c99_new_typed_iterator(result, receiver, 0);
            }
            /* An ordinary object with a `length` -- an `arguments` object above all, whose own
             * `@@iterator` is this very method -- iterates as the array-like it is. Refusing
             * here is what made `[...arguments]` stop compiling the moment `arguments` stopped
             * being an array, and the interpreter has walked an array-like receiver all along. */
            if (receiver.tag == KILN_VALUE_OBJECT) {
                kiln_value length;
                double count;
                status = kiln_c99_get(host, receiver, "length", 6, &length, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                count = kiln_c99_as_number(length);
                if (!(count > 0.0)) {
                    count = 0.0;
                }
                if (count > 4294967295.0) {
                    count = 4294967295.0;
                }
                status = kiln_c99_new_array(&snapshot);
                for (index = 0; index < (size_t)count && status == KILN_C99_OK; ++index) {
                    kiln_value element;
                    /* The index as a value, not as bytes: a boundary string's
                     * pointer names its header, and reading the header as key
                     * bytes misses every element. */
                    status = kiln_c99_get_computed(host, receiver,
                                                   kiln_c99_number((double)index),
                                                   &element, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                    status = kiln_c99_array_push(snapshot, element);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                return kiln_c99_new_iterator(result, snapshot);
            }
            /* A string's `@@iterator` is this same method, and it walks *characters*: an astral
             * pair is one step, which is the difference between iterating a string and indexing
             * it. Without this a string receiver fell to the refusal below, so
             * `for (const c of text)` refused where `[...text]` worked -- the two differ only in
             * that spread reaches `kiln_c99_iterate` directly while `for...of` is compiled as
             * `text[@@iterator]()` and so arrives here. A bundled `marked` refuses on exactly
             * that, tokenising a list item. The walk is `kiln_c99_iterate`'s rather than a second
             * one written out here, so the two spellings of one loop cannot come to disagree
             * about where a surrogate pair ends. */
            if (receiver.tag == KILN_VALUE_STRING) {
                status = kiln_c99_new_array(&snapshot);
                if (status != KILN_C99_OK) {
                    return status;
                }
                status = kiln_c99_iterate(host, snapshot, receiver, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                return kiln_c99_new_iterator(result, snapshot);
            }
            if (receiver.tag != KILN_VALUE_ARRAY) {
                KILN_C99_REFUSE("an array iteration method on a receiver that is not an array");
            }
            array = kiln_c99_array_of(receiver);
            status = kiln_c99_new_array(&snapshot);
            for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
                /* A hole reads as undefined here: `for...of` visits every position, unlike
                 * `forEach`, which skips one. */
                status = kiln_c99_array_push(snapshot,
                                             array->elements[index] == KILN_VAL_HOLE
                                                 ? kiln_c99_undefined()
                                                 : kiln_val_to_boundary(array->elements[index]));
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            return kiln_c99_new_iterator(result, snapshot);
        }
        case KILN_C99_METHOD_GENERATOR_SELF:
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                KILN_C99_REFUSE("a generator method on a receiver that is not a generator");
            }
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_GENERATOR_THROW: {
            kiln_generator *frame;
            kiln_closure *closure;
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                KILN_C99_REFUSE("a generator method on a receiver that is not a generator");
            }
            frame = kiln_c99_generator_of(receiver);
            if (frame->running) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Generator is already running");
            }
            if (frame->delegate.tag != KILN_VALUE_UNDEFINED && !frame->async_generator) {
                kiln_value thrown_val = argc > 0 ? args[0] : kiln_c99_undefined();
                return kiln_c99_generator_delegate_throw(host, frame, thrown_val, result, depth, thrown);
            }
            /* A suspended body with a handler over the suspension gets the value thrown *at*
             * the `yield`, so its `catch` or `finally` runs; it may yield again, finish, or
             * let the throw out. A body that has not started, is done, or has nothing to
             * catch with, is finished and the throw goes straight to the caller. */
            if (frame->started && !frame->done && frame->handler_top != 0 && !frame->async_generator) {
                kiln_val vproduced = KILN_VAL_UNDEFINED;
                frame->raising = 1;
                frame->sent = argc > 0 ? args[0] : kiln_c99_undefined();
                closure = kiln_c99_closure_of(frame->function);
                frame->running = 1;
                status = kiln_c99_invoke_closure_body(
                    host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0, depth, thrown, frame, 0);
                frame->running = 0;
                frame->raising = 0;
                if (status == KILN_C99_YIELDED) {
                    return kiln_c99_generator_step(host, kiln_val_to_boundary(vproduced), 0, result, depth, thrown);
                }
                frame->done = 1;
                if (status != KILN_C99_OK) {
                    return status;
                }
                return kiln_c99_generator_step(host, kiln_val_to_boundary(vproduced), 1, result, depth, thrown);
            }
            frame->done = 1;
            if (thrown != NULL) {
                *thrown = argc > 0 ? args[0] : kiln_c99_undefined();
            }
            return KILN_C99_THROWN;
        }
        case KILN_C99_METHOD_GENERATOR_NEXT:
        case KILN_C99_METHOD_GENERATOR_RETURN: {
            kiln_generator *frame;
            kiln_closure *closure;
            kiln_value produced = kiln_c99_undefined();
            int finished = 1;
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                KILN_C99_REFUSE("a generator method on a receiver that is not a generator");
            }
            frame = kiln_c99_generator_of(receiver);
            /* One frame, and it is on the C stack: resuming it from inside itself would overwrite
             * the locals the running body is standing on. The language says so by name. */
            if (frame->running) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Generator is already running");
            }
            if (frame->async_generator) {
                kiln_value step_promise;
                status = kiln_c99_new_promise(&step_promise);
                if (status != KILN_C99_OK) {
                    return status;
                }
                *result = step_promise;
                kiln_value sent_val = argc > 0 ? args[0] : kiln_c99_undefined();
                if (frame->done) {
                    return kiln_c99_async_generator_settle(
                        host, step_promise,
                        code == KILN_C99_METHOD_GENERATOR_RETURN ? sent_val : kiln_c99_undefined(),
                        1, depth, thrown);
                }
                if (code == KILN_C99_METHOD_GENERATOR_RETURN) {
                    if (frame->handler_top != 0 && !frame->done) {
                        frame->raising = 1;
                        frame->sent = kiln_c99_undefined();
                        closure = kiln_c99_closure_of(frame->function);
                        frame->running = 1;
                        kiln_val vproduced = KILN_VAL_UNDEFINED;
                        (void)kiln_c99_invoke_closure_body(
                            host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0, depth, thrown, frame, 0);
                        produced = kiln_val_to_boundary(vproduced);
                        frame->running = 0;
                    }
                    frame->done = 1;
                    return kiln_c99_async_generator_settle(host, step_promise, sent_val, 1, depth, thrown);
                }
                if (frame->queue_executing) {
                    if (frame->queue_count >= frame->queue_capacity) {
                        size_t new_cap = frame->queue_capacity == 0 ? 4 : frame->queue_capacity * 2;
                        frame->queue_promises = (kiln_value *)realloc(frame->queue_promises, new_cap * sizeof(kiln_value));
                        frame->queue_sent = (kiln_value *)realloc(frame->queue_sent, new_cap * sizeof(kiln_value));
                        frame->queue_capacity = new_cap;
                        /* The frame owns memory outside its block now (`KILN_GC_FINALIZED_AT_ALLOC`). */
                        kiln_c99_gc_note_finalizable(frame);
                    }
                    frame->queue_promises[frame->queue_count] = step_promise;
                    frame->queue_sent[frame->queue_count] = sent_val;
                    frame->queue_count++;
                    return KILN_C99_OK;
                }
                frame->queue_executing = 1;
                frame->promise = step_promise;
                return kiln_c99_async_generator_step(
                    host, receiver, sent_val, 0, depth, thrown);
            }
            if (code == KILN_C99_METHOD_GENERATOR_RETURN) {
                if (frame->delegate.tag != KILN_VALUE_UNDEFINED && !frame->async_generator) {
                    kiln_value sent_val = argc > 0 ? args[0] : kiln_c99_undefined();
                    return kiln_c99_generator_delegate_return(host, frame, sent_val, result, depth, thrown);
                }
                if (frame->handler_top != 0 && !frame->done) {
                    frame->raising = 1;
                    frame->sent = kiln_c99_return_signal();
                    closure = kiln_c99_closure_of(frame->function);
                    frame->running = 1;
                    kiln_val vproduced = KILN_VAL_UNDEFINED;
                    int status = kiln_c99_invoke_closure_body(
                        host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0, depth, thrown, frame, 0);
                    produced = kiln_val_to_boundary(vproduced);
                    frame->running = 0;
                    if (status != KILN_C99_OK && thrown != NULL && kiln_val_from_boundary(*thrown) != KILN_VAL_RETURN_SIGNAL) {
                        return status;
                    }
                }
                frame->done = 1;
                produced = argc > 0 ? args[0] : kiln_c99_undefined();
            } else if (!frame->done) {
                frame->sent = argc > 0 ? args[0] : kiln_c99_undefined();
                frame->raising = 0;
                closure = kiln_c99_closure_of(frame->function);
                frame->running = 1;
                kiln_val vproduced = KILN_VAL_UNDEFINED;
                status = kiln_c99_invoke_closure_body(
                    host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0, depth, thrown, frame, 0);
                produced = kiln_val_to_boundary(vproduced);
                frame->running = 0;
                if (status == KILN_C99_YIELDED) {
                    frame->started = 1;
                    finished = 0;
                } else if (status != KILN_C99_OK) {
                    frame->done = 1;
                    return status;
                } else {
                    frame->done = 1;
                }
            }
            if (status == KILN_C99_YIELDED && frame->delegate.tag != KILN_VALUE_UNDEFINED) {
                *result = produced;
                return KILN_C99_OK;
            }
            return kiln_c99_generator_step(host, produced, finished || frame->done, result, depth, thrown);
        }
        case KILN_C99_METHOD_JSON_STRINGIFY: {
            kiln_text text = {NULL, 0, 0};
            kiln_text indent = {NULL, 0, 0};
            int wrote;
            /* A replacer is a function or an array of keys, and honouring half of it would give
             * a document missing exactly what the program asked to control. */
            if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED &&
                args[1].tag != KILN_VALUE_NULL) {
                KILN_C99_REFUSE("JSON.stringify with a replacer");
            }
            if (argc > 2) {
                if (args[2].tag == KILN_VALUE_NUMBER) {
                    /* A number is that many spaces, capped at ten, which is what the
                     * specification says and what stops a silly argument from allocating. */
                    double count = kiln_c99_as_number(args[2]);
                    size_t spaces = !(count > 0.0) ? 0 : count > 10.0 ? 10 : (size_t)count;
                    while (spaces-- > 0) {
                        if (kiln_c99_text_append_c(&indent, " ") != KILN_C99_OK) {
                            free(indent.bytes);
                            return KILN_C99_OUT_OF_MEMORY;
                        }
                    }
                } else if (args[2].tag == KILN_VALUE_STRING) {
                    status = kiln_c99_json_indent_string(args[2], &indent);
                    if (status == KILN_C99_INVALID_ARGUMENT) {
                        free(indent.bytes);
                        KILN_C99_REFUSE("JSON.stringify indentation with ill-formed UTF-16");
                    }
                    if (status != KILN_C99_OK) {
                        free(indent.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
            }
            status = kiln_c99_json_value(host, &text, argc > 0 ? args[0] : kiln_c99_undefined(),
                                         indent.bytes, indent.len, "", 0, &wrote, depth, NULL,
                                         thrown);
            free(indent.bytes);
            if (status != KILN_C99_OK) {
                free(text.bytes);
                return status;
            }
            /* `JSON.stringify(undefined)` is `undefined`, not the string. */
            if (wrote) {
                *result = kiln_c99_text_value(&text);
            } else {
                free(text.bytes);
                *result = kiln_c99_undefined();
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_JSON_PARSE: {
            kiln_value source = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_undefined();
            kiln_val parsed;
            if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
                /* A reviver rewrites the tree bottom-up; ignoring it would hand back a document
                 * the program never asked for. */
                KILN_C99_REFUSE("JSON.parse with a reviver");
            }
            if (source.tag != KILN_VALUE_STRING) {
                KILN_C99_REFUSE("JSON.parse of a value with no string form");
            }
            status = kiln_c99_json_document((const kiln_string *)source.ptr, &parsed);
            /* Malformed input is a `SyntaxError` the program catches, not a fault that stops it:
             * `try { JSON.parse(input) } catch {}` around text that came from outside is the most
             * ordinary code there is, and it was unwritable while this ended the process. Only the
             * reader's own refusal becomes the throw -- running out of memory is still that. */
            if (status == KILN_C99_CONTROL_ERROR) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                            "Unexpected token in JSON");
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_val_to_boundary(parsed);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            if (subject.tag == KILN_VALUE_SHARED)
                return kiln_c99_shared_prototype(subject, result);
            if (subject.tag == KILN_VALUE_PROXY) {
                kiln_value trap;
                kiln_proxy *data = kiln_c99_proxy_of(subject);
                int trap_st = kiln_c99_proxy_trap(host, subject, "getPrototypeOf", &trap, depth, thrown);
                if (trap_st != KILN_C99_OK) return trap_st;
                if (trap.tag != KILN_VALUE_UNDEFINED) {
                    kiln_value trap_args[1] = { data->target };
                    return kiln_c99_call(host, result, trap, data->handler, trap_args, 1, depth, thrown);
                }
                return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                                              kiln_c99_undefined(), &data->target, 1, depth, thrown);
            }
            if (subject.tag == KILN_VALUE_OBJECT) {
                kiln_value linked = kiln_c99_object_of(subject)->prototype;
                /* Undefined and null are different answers: one means nothing was recorded, and
                 * an object with nothing recorded still inherits `Object.prototype`. The other
                 * means nothing was wanted, which is what `Object.create(null)` asks for. */
                *result = linked.tag == KILN_VALUE_UNDEFINED
                              ? kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE)
                              : linked;
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_ARRAY || subject.tag == KILN_VALUE_COLLECTION) {
                /* A subclass instance, or one a `setPrototypeOf` relinked, answers with the
                 * link its store carries. */
                kiln_value *slot = kiln_c99_exotic_store(subject);
                kiln_value linked = slot->tag == KILN_VALUE_OBJECT
                                        ? kiln_c99_object_of(*slot)->prototype
                                        : kiln_c99_undefined();
                if (linked.tag != KILN_VALUE_UNDEFINED) {
                    *result = linked;
                    return KILN_C99_OK;
                }
                if (subject.tag == KILN_VALUE_ARRAY) {
                    *result = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_PROTOTYPE);
                } else {
                    kiln_collection *collection = kiln_c99_collection_of(subject);
                    *result = kiln_c99_builtin(
                        collection->is_weak
                            ? (collection->is_map ? KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE
                                                  : KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE)
                            : (collection->is_map ? KILN_C99_BUILTIN_MAP_PROTOTYPE
                                                  : KILN_C99_BUILTIN_SET_PROTOTYPE));
                }
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_STRING) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_STRING_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_TYPED_ARRAY) {
                if (kiln_c99_typed_array_of(subject)->is_buffer) {
                    kiln_value proto;
                    int proto_status = kiln_c99_buffer_prototype(host, &proto, depth, thrown);
                    if (proto_status != KILN_C99_OK) return proto_status;
                    if (proto.tag == KILN_VALUE_OBJECT) {
                        *result = proto;
                        return KILN_C99_OK;
                    }
                }
                *result = kiln_c99_builtin(
                    kiln_c99_typed_prototype(kiln_c99_typed_array_of(subject)->kind));
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_DATA_VIEW) {
                /* One table for every view, because a DataView has no kind to pick one by. */
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_ARRAY_BUFFER) {
                *result = kiln_c99_builtin(kiln_c99_typed_buffer_of(subject)->shared != NULL
                                               ? KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER_PROTOTYPE
                                               : KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_FUNCTION || subject.tag == KILN_VALUE_METHOD ||
                subject.tag == KILN_VALUE_BOUND) {
                /* A function keeps a relinked prototype on its store, which is where
                 * `Object.setPrototypeOf(f, o)` puts it and where the read walk looks for
                 * inherited statics -- so this has to report the same link or the two disagree
                 * about a chain they are both walking. */
                kiln_value linked =
                    subject.tag == KILN_VALUE_FUNCTION &&
                            kiln_c99_closure_store(kiln_c99_closure_of(subject)).tag == KILN_VALUE_OBJECT
                        ? kiln_c99_object_of(kiln_c99_closure_store(kiln_c99_closure_of(subject)))->prototype
                        : kiln_c99_undefined();
                *result = linked.tag == KILN_VALUE_UNDEFINED
                              ? kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE)
                              : linked;
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_NUMBER) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_NUMBER_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_REGEXP) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_REGEXP_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_DATE) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_DATE_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_BUILTIN) {
                kiln_typed_kind typed_kind;
                kiln_value store;
                int stored = kiln_c99_callable_store(subject, &store, 0);
                if (stored != KILN_C99_OK) return stored;
                if (store.tag == KILN_VALUE_OBJECT &&
                    kiln_c99_object_of(store)->prototype.tag != KILN_VALUE_UNDEFINED) {
                    *result = kiln_c99_object_of(store)->prototype;
                    return KILN_C99_OK;
                }
                if (kiln_c99_typed_prototype_kind(subject.bits, &typed_kind)) {
                    *result = kiln_c99_builtin(KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE);
                    return KILN_C99_OK;
                }
                if (subject.bits == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE) {
                    *result = kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
                    return KILN_C99_OK;
                }
                if (subject.bits == KILN_C99_BUILTIN_PROMISE_PROTOTYPE) {
                    *result = kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
                    return KILN_C99_OK;
                }
                /* `TypeError` and the other native error constructors inherit from `Error`, not
                 * from `Function.prototype` directly; WPT's `assert_throws_js` walks that chain
                 * looking for a function named "Error" and answered "is not an Error subtype"
                 * for every one of them until 2026-09-19. `AggregateError` too. */
                if (kiln_c99_error_name(subject.bits) != NULL && subject.bits != KILN_C99_BUILTIN_ERROR) {
                    *result = kiln_c99_builtin(KILN_C99_BUILTIN_ERROR);
                    return KILN_C99_OK;
                }
                /* A prototype table's own prototype is `Object.prototype`, which is where the
                 * chain from any of them ends. */
                *result = kiln_c99_is_namespace(subject.bits) &&
                                  subject.bits != KILN_C99_BUILTIN_OBJECT_PROTOTYPE
                              ? kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE)
                          : subject.bits == KILN_C99_BUILTIN_OBJECT_PROTOTYPE
                              ? kiln_c99_null()
                              : kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE);
                return KILN_C99_OK;
            }
            KILN_C99_REFUSE("Object.getPrototypeOf of a value whose chain this backend does not "
                            "model");
        }
        case KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            if (subject.tag == KILN_VALUE_SHARED) {
                kiln_value current;
                kiln_value wanted = argc > 1 ? args[1] : kiln_c99_undefined();
                status = kiln_c99_shared_prototype(subject, &current);
                if (status != KILN_C99_OK) return status;
                if (!kiln_c99_strict_equal(current, wanted))
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Cannot set prototype of a non-extensible object");
                *result = subject;
                return KILN_C99_OK;
            }
            if (subject.tag != KILN_VALUE_OBJECT && subject.tag != KILN_VALUE_FUNCTION &&
                kiln_c99_exotic_store(subject) == NULL &&
                !(subject.tag == KILN_VALUE_BUILTIN && kiln_c99_is_namespace(subject.bits))) {
                KILN_C99_REFUSE("Object.setPrototypeOf on a value with no prototype slot");
            }
            {
                /* Where the literal's `__proto__` ignores a primitive, this one throws: a
                 * program that asked for a prototype in so many words is told the request could
                 * not be met rather than left to find out later. */
                kiln_value wanted = argc > 1 ? args[1] : kiln_c99_undefined();
                if (!kiln_c99_can_be_prototype(wanted)) {
                    return kiln_c99_throw_error(
                        thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                        "Object prototype may only be an Object or null");
                }
            }
            status = kiln_c99_set_prototype(subject, argc > 1 ? args[1] : kiln_c99_undefined());
            /* The object, so a `setPrototypeOf` can be inlined into the expression that built
             * it. */
            *result = subject;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_SYMBOLS: {
            /* The other half of `getOwnPropertyNames`, off the very same walk -- `ownKeys` is
             * both, and each of the two exists in order to be one of them. Its absence refused a
             * bundled `lodash` at the point it asked an object for its symbol keys, a question
             * the walk beneath it could already answer. */
            kiln_value listed;
            kiln_value symbols;
            status = kiln_c99_all_own_keys(host, argc > 0 ? args[0] : kiln_c99_undefined(),
                                           &listed, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_new_array(&symbols);
            for (index = 0; index < kiln_c99_array_of(listed)->count && status == KILN_C99_OK;
                 ++index) {
                kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(listed)->elements[index]);
                if (key.tag != KILN_VALUE_SYMBOL) {
                    continue;
                }
                status = kiln_c99_array_push(symbols, key);
            }
            *result = symbols;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES: {
            /* Names, so the symbol keys `Reflect.ownKeys` reports are dropped again here. The
             * two operations exist precisely to be different: `getOwnPropertySymbols` is the
             * other half, and `ownKeys` is both. */
            kiln_value listed;
            kiln_value named;
            status = kiln_c99_all_own_keys(host, argc > 0 ? args[0] : kiln_c99_undefined(),
                                           &listed, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_new_array(&named);
            for (index = 0; index < kiln_c99_array_of(listed)->count && status == KILN_C99_OK;
                 ++index) {
                kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(listed)->elements[index]);
                if (key.tag == KILN_VALUE_SYMBOL) {
                    continue;
                }
                status = kiln_c99_array_push(named, key);
            }
            *result = named;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY: {
            kiln_value name = argc > 1 ? kiln_c99_property_key(args[1]) : kiln_c99_undefined();
            const kiln_string *name_key =
                name.tag == KILN_VALUE_STRING ? (const kiln_string *)name.ptr : NULL;
            if (argc < 3 || name_key == NULL) {
                KILN_C99_REFUSE("Object.defineProperty invalid key or arity");
            }
            /* The headed form: the byte adapter would read the header's words as key bytes. */
            status = kiln_c99_define_property_key(host, args[0], name_key, NULL, args[2],
                                                  depth, thrown);
            if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_detail == NULL)
                kiln_c99_diagnostic_detail = "Object.defineProperty rejected descriptor or target";
            /* The target, so a definition can be inlined into the expression that built it. */
            *result = args[0];
            return status;
        }
        case KILN_C99_METHOD_OBJECT_DEFINE_PROPERTIES: {
            if (argc < 2) {
                KILN_C99_REFUSE("Object.defineProperties needs a target and a map of descriptors");
            }
            status = kiln_c99_define_properties(host, args[0], args[1], depth, thrown);
            *result = args[0];
            return status;
        }
        case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR: {
            kiln_value name = argc > 1 ? kiln_c99_property_key(args[1]) : kiln_c99_undefined();
            const kiln_string *name_key =
                name.tag == KILN_VALUE_STRING ? (const kiln_string *)name.ptr : NULL;
            int found;
            if (argc < 2 || name_key == NULL) {
                KILN_C99_REFUSE("Object.getOwnPropertyDescriptor invalid key or arity");
            }
            return kiln_c99_describe_property_key(host, args[0], name_key, NULL, result,
                                                  &found, depth, thrown);
        }
        case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTORS: {
            /* One object holding every own property's descriptor, which is what a structural
             * clone reaches for -- accessors and all, where a spread would have called them. */
            kiln_value built;
            kiln_object *object;
            if (argc == 0 || args[0].tag != KILN_VALUE_OBJECT) {
                KILN_C99_REFUSE("Object.getOwnPropertyDescriptors of something that is not an "
                                "object");
            }
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            object = kiln_c99_object_of(args[0]);
            if (object->shape != NULL) {
                kiln_c99_ensure_dictionary(object);
            }
            for (index = 0; index < object->count; ++index) {
                kiln_value described;
                int found;
                kiln_value symbol;
                const kiln_string *key = object->properties[index].key;
                /* A symbol-keyed property is described too -- this is how a faithful copy is
                 * made, and one that quietly leaves them out is the bug this family had. It
                 * lands in the result under the same reserved key it came from, so nothing here
                 * needs to know it is a symbol beyond deciding to keep it. */
                if (kiln_c99_key_is_reserved(key) &&
                    !kiln_c99_symbol_of_key_view(key, &symbol)) {
                    continue;
                }
                status = kiln_c99_describe_property_key(host, args[0], key, key, &described,
                                                        &found, depth, thrown);
                if (status == KILN_C99_OK && found) {
                    status = kiln_c99_put_mode_key(NULL, built, key, key, described, 0, 0, NULL);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                object = kiln_c99_object_of(args[0]);
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_PROXY_REVOCABLE: {
            /* A pair: the proxy, and the one function that can retire it. Handing the second out
             * separately is the whole point -- a caller can be given the proxy without being
             * given the power to withdraw it, and the other way round. */
            kiln_value proxy;
            kiln_value revoke;
            kiln_value pair;
            status = kiln_c99_new_proxy(&proxy, argc > 0 ? args[0] : kiln_c99_undefined(),
                                        argc > 1 ? args[1] : kiln_c99_undefined());
            if (status == KILN_C99_OK) {
                status = kiln_c99_bind(&revoke, kiln_c99_method(KILN_C99_METHOD_PROXY_REVOKE),
                                       proxy);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_new_object(&pair);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, pair, "proxy", 5, proxy, 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, pair, "revoke", 6, revoke, 0, NULL);
            }
            *result = pair;
            return status;
        }
        case KILN_C99_METHOD_REFLECT_OWN_KEYS:
        case KILN_C99_METHOD_REFLECT_GET:
        case KILN_C99_METHOD_REFLECT_SET:
        case KILN_C99_METHOD_REFLECT_HAS:
        case KILN_C99_METHOD_REFLECT_DELETE_PROPERTY: {
            /* Every one of these refuses a primitive target rather than coercing it, which is
             * the difference from the operator forms: `Reflect.get(1, "x")` is a TypeError where
             * `(1).x` reads off the string prototype. A library that reaches for `Reflect`
             * usually does so precisely to get the strict form. */
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value key = argc > 1 ? args[1] : kiln_c99_undefined();
            int answer;
            if (!kiln_c99_is_reference(subject) && subject.tag != KILN_VALUE_DATE &&
                subject.tag != KILN_VALUE_REGEXP && subject.tag != KILN_VALUE_PROMISE) {
                return kiln_c99_throw_not_a(thrown, subject, "object");
            }
            switch (code) {
            case KILN_C99_METHOD_REFLECT_OWN_KEYS:
                return kiln_c99_all_own_keys(host, subject, result, depth, thrown);
            case KILN_C99_METHOD_REFLECT_GET:
                /* The third argument is what an accessor sees as `this` and what a `get` trap is
                 * handed; without one it is the target itself. */
                return kiln_c99_get_with_receiver(host, subject, key,
                                                  argc > 2 ? args[2] : subject, result, depth,
                                                  thrown);
            case KILN_C99_METHOD_REFLECT_SET:
                status = kiln_c99_reflect_set(host, subject, key,
                                               argc > 2 ? args[2] : kiln_c99_undefined(), &answer,
                                               depth, thrown);
                *result = kiln_c99_boolean(answer);
                return status;
            case KILN_C99_METHOD_REFLECT_HAS:
                status = kiln_c99_has_computed(host, subject, key, &answer, depth, thrown);
                *result = kiln_c99_boolean(answer);
                return status;
            default:
                status = kiln_c99_delete_computed(host, subject, key, &answer, depth, thrown);
                *result = kiln_c99_boolean(answer);
                return status;
            }
        }
        case KILN_C99_METHOD_REFLECT_DEFINE_PROPERTY: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value key = argc > 1 ? args[1] : kiln_c99_undefined();
            kiln_value descriptor = argc > 2 ? args[2] : kiln_c99_undefined();
            int answer = 0;
            if (!kiln_c99_is_reference(subject) && subject.tag != KILN_VALUE_DATE &&
                subject.tag != KILN_VALUE_REGEXP && subject.tag != KILN_VALUE_PROMISE) {
                return kiln_c99_throw_not_a(thrown, subject, "object");
            }
            if (descriptor.tag != KILN_VALUE_OBJECT) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Property description must be an object");
            }
            status = kiln_c99_reflect_define_property(host, subject, key, descriptor, &answer,
                                                      depth, thrown);
            *result = kiln_c99_boolean(answer);
            return status;
        }
        case KILN_C99_METHOD_REFLECT_GET_PROTOTYPE_OF: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            if (!kiln_c99_is_reference(subject) && subject.tag != KILN_VALUE_DATE &&
                subject.tag != KILN_VALUE_REGEXP && subject.tag != KILN_VALUE_PROMISE) {
                return kiln_c99_throw_not_a(thrown, subject, "object");
            }
            return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                                          kiln_c99_undefined(), &subject, 1, depth, thrown);
        }
        case KILN_C99_METHOD_REFLECT_APPLY: {
            /* `Reflect.apply(f, this, args)` is `f.apply(this, args)` without going through the
             * function's own `apply`, which a proxy or an override could have replaced. */
            kiln_array *list;
            if (argc < 3 || args[2].tag != KILN_VALUE_ARRAY) {
                KILN_C99_REFUSE("Reflect.apply needs a callee, a receiver and an array of "
                                "arguments");
            }
            list = kiln_c99_array_of(args[2]);
            kiln_val call_result_val = KILN_VAL_UNDEFINED;
            status = kiln_c99_call_val(host, &call_result_val, kiln_val_from_boundary(args[0]),
                                       kiln_val_from_boundary(args[1]),
                                       list->count == 0 ? NULL : list->elements, list->count, depth,
                                       thrown);
            if (status == KILN_C99_OK) *result = kiln_val_to_boundary(call_result_val);
            return status;
        }
        case KILN_C99_METHOD_REFLECT_CONSTRUCT: {
            kiln_array *list;
            if (argc < 2 || args[1].tag != KILN_VALUE_ARRAY) {
                KILN_C99_REFUSE("Reflect.construct needs a callee and an array of arguments");
            }
            if (argc > 2) {
                /* A third argument names a `new.target` other than the callee, which changes
                 * both what the body reads and which prototype the instance gets. */
                KILN_C99_REFUSE("Reflect.construct with a new.target other than the callee");
            }
            list = kiln_c99_array_of(args[1]);
            kiln_val built_val = KILN_VAL_UNDEFINED;
            status = kiln_c99_construct_val(host, &built_val, kiln_val_from_boundary(args[0]),
                                            list->count == 0 ? NULL : list->elements, list->count,
                                            depth, thrown);
            if (status == KILN_C99_OK) *result = kiln_val_to_boundary(built_val);
            return status;
        }
        case KILN_C99_METHOD_ERROR_CAPTURE_STACK_TRACE:
            /* A release build strips unwind tables, so there are no frames to read: the
             * property this would write is one this engine cannot fill in. Answering undefined
             * and leaving the error's own `stack` header alone is what the interpreter does. */
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        case KILN_C99_METHOD_OBJECT_IS: {
            /* SameValue: `===` except that it separates the two zeroes and finds NaN. The
             * collections already compare with SameValueZero, which differs from this in
             * exactly the zero case, so the shared half is reused and only that case is added. */
            kiln_value left = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value right = argc > 1 ? args[1] : kiln_c99_undefined();
            if (left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_NUMBER &&
                kiln_c99_as_number(left) == 0.0 && kiln_c99_as_number(right) == 0.0) {
                *result = kiln_c99_boolean(kiln_c99_signbit(kiln_c99_as_number(left)) ==
                                           kiln_c99_signbit(kiln_c99_as_number(right)));
                return KILN_C99_OK;
            }
            *result = kiln_c99_boolean(kiln_c99_same_value_zero(left, right));
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_STRING_FROM_CHAR_CODE: {
            /* Each argument is one UTF-16 *code unit*, so a surrogate pair spelled across two
             * arguments becomes the one character it stands for and a lone surrogate becomes
             * U+FFFD. `kiln_c99_from_units` already draws both of those lines, and draws them
             * where `String::from_utf16_lossy` does, which is what the interpreter builds the
             * string with.
             *
             * The narrowing is the interpreter's `as u16` rather than the specification's
             * `ToUint16`, and the difference is visible: the specification *wraps*, so
             * `fromCharCode(65601)` is "A" in node, where a saturating cast answers U+FFFF.
             * Matching the specification here would be a wrong answer against the oracle this
             * corpus measures against -- the two engines disagreeing is the failure being
             * fixed, so the deviation stays in one place and is shared rather than split. */
            uint16_t *units = NULL;
            size_t at;
            if (argc > 0) {
                units = (uint16_t *)malloc(argc * sizeof *units);
                if (units == NULL) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            for (at = 0; at < argc; ++at) {
                double number = kiln_c99_as_number(args[at]);
                units[at] = isnan(number)            ? (uint16_t)0
                            : number <= 0.0          ? (uint16_t)0
                            : number >= 65535.0      ? (uint16_t)65535
                                                     : (uint16_t)number;
            }
            *result = kiln_c99_from_units(units, argc);
            free(units);
            return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
        }
        case KILN_C99_METHOD_STRING_FROM_CODE_POINT: {
            uint16_t *units = NULL;
            size_t out_count = 0;
            size_t at;
            if (argc > 0) {
                units = (uint16_t *)malloc(argc * 2 * sizeof *units);
                if (units == NULL) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            for (at = 0; at < argc; ++at) {
                double number = kiln_c99_as_number(args[at]);
                if (isnan(number) || floor(number) != number || number < 0.0 || number > 1114111.0) {
                    free(units);
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                "Invalid code point");
                }
                uint32_t cp = (uint32_t)number;
                if (cp <= 0xFFFF) {
                    units[out_count++] = (uint16_t)cp;
                } else {
                    cp -= 0x10000;
                    units[out_count++] = (uint16_t)(0xD800 + (cp >> 10));
                    units[out_count++] = (uint16_t)(0xDC00 + (cp & 0x3FF));
                }
            }
            *result = kiln_c99_from_units(units, out_count);
            free(units);
            return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
        }
        case KILN_C99_METHOD_STRING_RAW: {
            /* `String.raw` reads the *raw* half of the template object -- the text as written,
             * before escapes were interpreted -- and rejoins it around the substitutions. The
             * substitutions arrive as the remaining arguments, one between each pair of
             * segments and none after the last, which is what makes a trailing `${}` add
             * nothing. */
            kiln_value strings = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value raw;
            kiln_array *segments;
            kiln_text built = {NULL, 0, 0};
            size_t at;
            status = kiln_c99_get(host, strings, "raw", 3, &raw, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (raw.tag != KILN_VALUE_ARRAY) {
                /* A hand-built array-like would walk here too; only the array a template site
                 * produces is understood so far. */
                KILN_C99_REFUSE("a tagged template whose raw strings are not an array");
            }
            segments = kiln_c99_array_of(raw);
            for (at = 0; at < segments->count; ++at) {
                kiln_value piece = segments->elements[at] == KILN_VAL_HOLE
                                       ? kiln_c99_undefined()
                                       : kiln_val_to_boundary(segments->elements[at]);
                kiln_value shown = kiln_c99_text(piece);
                if (shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_text_append_string(&built, shown) != KILN_C99_OK) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                if (at + 1 < segments->count && at + 1 < argc) {
                    kiln_value between = kiln_c99_text(args[at + 1]);
                    if (between.tag != KILN_VALUE_STRING ||
                        kiln_c99_text_append_string(&built, between) != KILN_C99_OK) {
                        free(built.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
            }
            *result = kiln_c99_text_value(&built);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_FROM_ENTRIES: {
            kiln_value built;
            kiln_value source = argc > 0 ? args[0] : kiln_c99_undefined();
            if (source.tag == KILN_VALUE_UNDEFINED || source.tag == KILN_VALUE_NULL) {
                return kiln_c99_throw_not_a(thrown, source, "iterable");
            }
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (source.tag == KILN_VALUE_COLLECTION && kiln_c99_collection_of(source)->is_map) {
                kiln_collection *col = kiln_c99_collection_of(source);
                for (index = 0; index < col->count; ++index) {
                    if (col->entries[index].dead) continue;
                    status = kiln_c99_set_computed(host, built, col->entries[index].key,
                                                   col->entries[index].value, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
            } else {
                kiln_value gathered;
                kiln_array *pairs;
                if (source.tag == KILN_VALUE_ARRAY) {
                    gathered = source;
                } else {
                    status = kiln_c99_new_array(&gathered);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                    status = kiln_c99_iterate(host, gathered, source, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                pairs = kiln_c99_array_of(gathered);
                for (index = 0; index < pairs->count; ++index) {
                    kiln_value entry = pairs->elements[index] == KILN_VAL_HOLE
                                           ? kiln_c99_undefined()
                                           : kiln_val_to_boundary(pairs->elements[index]);
                    kiln_value key = kiln_c99_undefined();
                    kiln_value val = kiln_c99_undefined();
                    if (entry.tag == KILN_VALUE_ARRAY) {
                        kiln_array *pair = kiln_c99_array_of(entry);
                        if (pair->count > 0 && pair->elements[0] != KILN_VAL_HOLE) {
                            key = kiln_val_to_boundary(pair->elements[0]);
                        }
                        if (pair->count > 1 && pair->elements[1] != KILN_VAL_HOLE) {
                            val = kiln_val_to_boundary(pair->elements[1]);
                        }
                    } else if (entry.tag == KILN_VALUE_OBJECT) {
                        status = kiln_c99_get(host, entry, "0", 1, &key, depth, thrown);
                        if (status != KILN_C99_OK) return status;
                        status = kiln_c99_get(host, entry, "1", 1, &val, depth, thrown);
                        if (status != KILN_C99_OK) return status;
                    } else {
                        return kiln_c99_throw_not_a(thrown, entry, "object");
                    }
                    status = kiln_c99_set_computed(host, built, key, val, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ARRAY_FROM: {
            /* Anything iterable, then the optional mapper -- which is one walk rather than a
             * `map` afterwards, because `Array.from({length: 3}, f)` has nothing to map. */
            kiln_value built;
            kiln_value mapper = argc > 1 ? args[1] : kiln_c99_undefined();
            kiln_array *collected;
            kiln_value source = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value iterator_method;
            if (mapper.tag != KILN_VALUE_UNDEFINED && !kiln_c99_is_callable(mapper)) {
                return kiln_c99_throw_not_a(thrown, mapper, "function");
            }
            if (source.tag == KILN_VALUE_UNDEFINED || source.tag == KILN_VALUE_NULL) {
                return kiln_c99_throw_not_a(thrown, source, "iterable or array-like object");
            }
            status = kiln_c99_new_array(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            iterator_method = kiln_c99_undefined();
            /* A generator is iterable and is not array-like: it has no `length`, so falling to
             * the array-like path below read `undefined`, coerced it to zero, and answered `[]`
             * for a generator that yields. That is the silent kind of wrong -- `Array.from(g())`
             * is how a program drains one, and an empty array is a plausible answer a caller
             * carries forward. It used to refuse here, so the answer got worse, not better. */
            if (source.tag == KILN_VALUE_ARRAY || source.tag == KILN_VALUE_STRING ||
                source.tag == KILN_VALUE_TYPED_ARRAY || source.tag == KILN_VALUE_COLLECTION ||
                source.tag == KILN_VALUE_GENERATOR) {
                status = kiln_c99_probe_symbol(host, source, "@@iterator", 10, &iterator_method, depth,
                                      thrown);
                if (status != KILN_C99_OK) return status;
            } else if (source.tag == KILN_VALUE_OBJECT) {
                /* A plain object is array-like unless it explicitly supplies the iterator
                 * protocol. Unknown ordinary names on objects read as undefined here. */
                status = kiln_c99_probe_symbol(host, source, "@@iterator", 10, &iterator_method, depth,
                                      thrown);
                if (status == KILN_C99_CONTROL_ERROR) {
                    /* Answered rather than refused, so the refusal's detail must go with it --
                     * see the `@@` conversion in `kiln_c99_get` for what a left-behind one does. */
                    kiln_c99_diagnostic_detail = NULL;
                    iterator_method = kiln_c99_undefined();
                    status = KILN_C99_OK;
                }
                if (status != KILN_C99_OK) return status;
            }
            if (iterator_method.tag == KILN_VALUE_UNDEFINED ||
                iterator_method.tag == KILN_VALUE_NULL) {
                /* `LengthOfArrayLike` is `ToLength(Get(items, "length"))`, so a length that is
                 * not already a number is coerced rather than read as zero, and one no array can
                 * have is the `ArrayCreate` step's `RangeError`. Clamping `Infinity` to
                 * `(size_t)-1` and then *running* was the other answer, and it reached 6 GB
                 * before anything stopped it. The interpreter refuses at the same bound. */
                kiln_value length_value;
                double wanted;
                size_t length = 0;
                status = kiln_c99_get(host, source, "length", 6, &length_value, depth, thrown);
                if (status != KILN_C99_OK) return status;
                status = kiln_c99_to_number_checked(host, length_value, &wanted, depth, thrown);
                if (status != KILN_C99_OK) return status;
                if (wanted > 0.0) {
                    wanted = floor(wanted);
                    if (wanted > 4294967295.0) {
                        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                    "Invalid array length");
                    }
                    length = (size_t)wanted;
                }
                for (index = 0; index < length; ++index) {
                    char key[32];
                    size_t key_len = kiln_c99_double_to_js((double)index, key);
                    kiln_value value;
                    kiln_value produced;
                    status = kiln_c99_get(host, source, key, key_len, &value, depth, thrown);
                    if (status != KILN_C99_OK) return status;
                    if (mapper.tag != KILN_VALUE_UNDEFINED) {
                        kiln_value visit[2] = {value, kiln_c99_number((double)index)};
                        kiln_value this_arg = argc > 2 ? args[2] : kiln_c99_undefined();
                        status = kiln_c99_call(host, &produced, mapper, this_arg, visit, 2, depth,
                                               thrown);
                        if (status != KILN_C99_OK) return status;
                        value = produced;
                    }
                    status = kiln_c99_array_push(built, value);
                    if (status != KILN_C99_OK) return status;
                }
                *result = built;
                return KILN_C99_OK;
            }
            if (!kiln_c99_is_callable(iterator_method)) {
                return kiln_c99_throw_not_a(thrown, iterator_method, "function");
            }
            status = kiln_c99_iterate(host, built, source, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (!kiln_c99_is_callable(mapper)) {
                *result = built;
                return KILN_C99_OK;
            }
            collected = kiln_c99_array_of(built);
            for (index = 0; index < collected->count; ++index) {
                kiln_value visit[2];
                kiln_value produced;
                visit[0] = kiln_val_to_boundary(collected->elements[index]);
                visit[1] = kiln_c99_number((double)index);
                status = kiln_c99_call(host, &produced, mapper,
                                       argc > 2 ? args[2] : kiln_c99_undefined(), visit, 2, depth,
                                       thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                collected->elements[index] = kiln_val_from_boundary(produced);
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE: {
            /* Own *and* enumerable, which is the pair `Object.keys` reports and `hasOwnProperty`
             * only half of. */
            kiln_value name = kiln_c99_property_key(argc > 0 ? args[0] : kiln_c99_undefined());
            kiln_object *object;
            size_t slot;
            *result = kiln_c99_boolean(0);
            if (receiver.tag == KILN_VALUE_TYPED_ARRAY && name.tag == KILN_VALUE_STRING) {
                int found;
                int enumerable;
                kiln_c99_typed_own_key(receiver, (const kiln_string *)name.ptr, &found,
                                       &enumerable);
                *result = kiln_c99_boolean(found && enumerable);
                return KILN_C99_OK;
            }
            if (receiver.tag != KILN_VALUE_OBJECT || name.tag != KILN_VALUE_STRING) {
                return KILN_C99_OK;
            }
            object = kiln_c99_object_of(receiver);
            if (object->shape != NULL) {
                kiln_c99_ensure_dictionary(object);
            }
            for (slot = 0; slot < object->count; ++slot) {
                if (kiln_c99_same_key_view(&object->properties[slot],
                                           (const kiln_string *)name.ptr)) {
                    *result = kiln_c99_boolean(object->properties[slot].enumerable);
                    break;
                }
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_IS_PROTOTYPE_OF: {
            /* Walks the chain rather than comparing one link, which is what makes it different
             * from `Object.getPrototypeOf(x) === this`. */
            kiln_value walked = argc > 0 ? args[0] : kiln_c99_undefined();
            size_t steps;
            *result = kiln_c99_boolean(0);
            for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
                if (walked.tag != KILN_VALUE_OBJECT) {
                    break;
                }
                walked = kiln_c99_object_of(walked)->prototype;
                if (kiln_c99_strict_equal(walked, receiver)) {
                    *result = kiln_c99_boolean(1);
                    break;
                }
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS:
        case KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE: {
            /* Weaker than freezing, and the difference is the point: a non-extensible object still
             * takes writes to what it already has. A library asks `isExtensible` before deciding
             * it may cache or wrap a value, so answering `isFrozen` for it would be a wrong answer
             * rather than a missing one.
             *
             * A primitive is not an error: the specification sends a non-object through unchanged
             * and reports it as non-extensible, which is what `Object.isExtensible(1)` is. */
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            int *flag = NULL;
            int frozen = 0;
            if (subject.tag == KILN_VALUE_SHARED) {
                *result = code == KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE
                              ? kiln_c99_boolean(0) : subject;
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_OBJECT) {
                kiln_object *object = kiln_c99_object_of(subject);
                /* A module namespace is born non-extensible and cannot be made otherwise. */
                if (object->module_namespace) {
                    *result = code == KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE
                                  ? kiln_c99_boolean(0)
                                  : subject;
                    return KILN_C99_OK;
                }
                flag = &object->not_extensible;
                frozen = object->frozen;
            } else if (subject.tag == KILN_VALUE_ARRAY) {
                flag = &kiln_c99_array_of(subject)->not_extensible;
                frozen = kiln_c99_array_of(subject)->frozen;
            } else if (subject.tag == KILN_VALUE_FUNCTION) {
                kiln_value store;
                status = kiln_c99_function_store(subject, &store);
                if (status != KILN_C99_OK) {
                    return status;
                }
                flag = &kiln_c99_object_of(store)->not_extensible;
                frozen = kiln_c99_object_of(store)->frozen;
            }
            if (flag == NULL) {
                /* A primitive keeps the specification's answer: it is not extensible, and
                 * `preventExtensions` hands it back unchanged. */
                if (!kiln_c99_is_reference(subject) && subject.tag != KILN_VALUE_DATE) {
                    *result = code == KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE ? kiln_c99_boolean(0)
                                                                          : subject;
                    return KILN_C99_OK;
                }
                /* An object-like value this backend keeps no extensibility slot for -- a
                 * collection, a date, a promise, a regexp, a typed array, a bound function.
                 * Answering `false` for one was a wrong answer rather than a missing feature:
                 * nothing has prevented its extensions, so the language says true. It cost the
                 * whole of vue's collection reactivity, because `getTargetType` asks
                 * `isExtensible` before it will wrap a Map and reads false as "do not wrap": a
                 * reactive Map and Set came back unwrapped, no trap ever ran, and every effect
                 * over one ran exactly once.
                 *
                 * `preventExtensions` is the half that stays a refusal -- there is nowhere to
                 * record it, and one that quietly did nothing would make the answer above lie in
                 * the other direction. A proxy refuses both: either is a trap call. */
                if (subject.tag == KILN_VALUE_PROXY ||
                    code == KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS) {
                    KILN_C99_REFUSE("Object.preventExtensions, or an extensibility question about "
                                    "a Proxy");
                }
                *result = kiln_c99_boolean(1);
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE) {
                /* Freezing implies non-extensible, so it has to be read here too: an object
                 * frozen without ever being told to prevent extensions is still not extensible. */
                *result = kiln_c99_boolean(!*flag && !frozen);
                return KILN_C99_OK;
            }
            *flag = 1;
            /* The target, so a call can be inlined into the expression that built it. */
            *result = subject;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_FREEZE:
        case KILN_C99_METHOD_OBJECT_IS_FROZEN: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            int *flag = NULL;
            if (subject.tag == KILN_VALUE_SHARED) {
                *result = code == KILN_C99_METHOD_OBJECT_FREEZE
                              ? subject : kiln_c99_boolean(1);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_OBJECT) {
                if (kiln_c99_object_of(subject)->module_namespace) {
                    kiln_object *namespace_object = kiln_c99_object_of(subject);
                    size_t namespace_index;
                    int has_writable_export = 0;
                    /* A namespace is already non-extensible. A non-empty one cannot make its
                     * export descriptors non-writable without violating the exotic contract;
                     * an empty namespace has only the fixed toStringTag slot and is frozen. */
                    for (namespace_index = 0; namespace_index < namespace_object->count;
                         ++namespace_index) {
                        if (namespace_object->properties[namespace_index].writable) {
                            has_writable_export = 1;
                            break;
                        }
                    }
                    if (code == KILN_C99_METHOD_OBJECT_FREEZE) {
                        if (has_writable_export)
                            return kiln_c99_throw_module_namespace_freeze(thrown);
                        namespace_object->frozen = 1;
                        *result = subject;
                        return KILN_C99_OK;
                    }
                    *result = kiln_c99_boolean(!has_writable_export);
                    return KILN_C99_OK;
                }
                flag = &kiln_c99_object_of(subject)->frozen;
            } else if (subject.tag == KILN_VALUE_ARRAY) {
                /* An array is not a `kiln_object` here, so freezing one has to reach the flag on
                 * the array itself -- and its elements are not properties of a store, which is
                 * why they need their own check rather than inheriting the object one. */
                flag = &kiln_c99_array_of(subject)->frozen;
            } else if (subject.tag == KILN_VALUE_FUNCTION) {
                /* A function's own properties live in an object it makes on first use. Freezing
                 * the function is freezing that store, so it has to exist first. */
                kiln_value store;
                int status = kiln_c99_function_store(subject, &store);
                if (status != KILN_C99_OK) {
                    return status;
                }
                flag = &kiln_c99_object_of(store)->frozen;
            }
            if (subject.tag == KILN_VALUE_PROXY) {
                kiln_value inner = kiln_c99_proxy_inner(subject);
                return kiln_c99_invoke_method(host, result, code, kiln_c99_undefined(), &inner, 1,
                                              depth, thrown);
            }
            if (flag == NULL) {
                /* A built-in -- `Object`, `Math`, `JSON`, `Array.prototype.push` -- is an object
                 * whose properties are writable and configurable, so it is not frozen and there
                 * is nowhere to record that it became so. It shares this arm with the primitives
                 * only because neither keeps a store; answering `true` for it said the realm's
                 * own namespaces could not be changed, which every one of them can. */
                int is_object = subject.tag == KILN_VALUE_BUILTIN ||
                                subject.tag == KILN_VALUE_METHOD ||
                                subject.tag == KILN_VALUE_GENERATOR ||
                                subject.tag == KILN_VALUE_SETTLER ||
                                subject.tag == KILN_VALUE_TEMPORAL ||
                                subject.tag == KILN_VALUE_COMMONJS_REQUIRE ||
                                subject.tag == KILN_VALUE_COLLECTION ||
                                subject.tag == KILN_VALUE_PROMISE;
                /* A primitive has nothing to freeze and is already as frozen as it gets. */
                *result = code == KILN_C99_METHOD_OBJECT_FREEZE ? subject
                                                                : kiln_c99_boolean(!is_object);
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_OBJECT_FREEZE) {
                *flag = 1;
                if (subject.tag == KILN_VALUE_OBJECT) {
                    kiln_object *obj = kiln_c99_object_of(subject);
                    size_t i;
                    if (obj->shape != NULL) {
                        kiln_c99_ensure_dictionary(obj);
                        obj->shape = NULL;
                    }
                    for (i = 0; i < obj->count; ++i) {
                        if (!obj->properties[i].accessor) {
                            obj->properties[i].writable = 0;
                        }
                        obj->properties[i].configurable = 0;
                    }
                } else if (subject.tag == KILN_VALUE_FUNCTION) {
                    kiln_closure *closure = kiln_c99_closure_of(subject);
                    if (kiln_c99_closure_store(closure).tag == KILN_VALUE_OBJECT) {
                        kiln_object *obj = kiln_c99_object_of(kiln_c99_closure_store(closure));
                        size_t i;
                        if (obj->shape != NULL) {
                            kiln_c99_ensure_dictionary(obj);
                            obj->shape = NULL;
                        }
                        for (i = 0; i < obj->count; ++i) {
                            if (!obj->properties[i].accessor) {
                                obj->properties[i].writable = 0;
                            }
                            obj->properties[i].configurable = 0;
                        }
                    }
                } else if (subject.tag == KILN_VALUE_ARRAY) {
                    kiln_array *arr = kiln_c99_array_of(subject);
                    if (arr->properties.tag == KILN_VALUE_OBJECT) {
                        kiln_object *obj = kiln_c99_object_of(arr->properties);
                        size_t i;
                        if (obj->shape != NULL) {
                            kiln_c99_ensure_dictionary(obj);
                            obj->shape = NULL;
                        }
                        for (i = 0; i < obj->count; ++i) {
                            if (!obj->properties[i].accessor) {
                                obj->properties[i].writable = 0;
                            }
                            obj->properties[i].configurable = 0;
                        }
                    }
                }
                /* The object itself, so `Object.freeze(o) === o` and a freeze can be inlined into
                 * the expression that built it. */
                *result = subject;
            } else {
                *result = kiln_c99_boolean(*flag);
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_CREATE: {
            kiln_value built;
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            /* `Object.create(null)` records the absence, which is not the same as recording
             * nothing: an object with no link at all inherits `Object.prototype`. */
            if (argc > 0 &&
                (args[0].tag == KILN_VALUE_NULL || kiln_c99_can_be_prototype(args[0]))) {
                kiln_c99_object_of(built)->prototype = args[0];
            } else if (argc > 0) {
                KILN_C99_REFUSE("Object.create given a prototype that cannot be one");
            }
            /* The second argument is the same map of descriptors `defineProperties` takes, and
             * it goes through the same code -- a definition, not an assignment, so an accessor
             * arrives as an accessor and the prototype's setters are not consulted. */
            if (argc > 1) {
                status = kiln_c99_define_properties(host, built, args[1], depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_HAS_OWN: {
            /* The static spelling of `hasOwnProperty`, and the same helper answers both. */
            int found = 0;
            /* Both arguments, for the reason `hasOwnProperty` gives above. */
            if (argc < 2) {
                KILN_C99_REFUSE("Object.hasOwn needs both a target and a key");
            }
            status = kiln_c99_own_property_present(host, args[0], args[1], &found, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_boolean(found);
            return KILN_C99_OK;
        }
        default:
            KILN_C99_REFUSE("an Object or Reflect static this backend has not implemented");
        }
    }
    if ((groups & KILN_C99_GROUP_STRING_DISPATCH)) {
        if (receiver.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a String.prototype method on a receiver that is not a string");
        }
        return kiln_c99_invoke_string_method(host, result, code, receiver, args, argc, depth,
                                             thrown);
    }
    if (receiver.tag == KILN_VALUE_ARRAY_BUFFER) {
        if (code == KILN_C99_METHOD_SLICE) {
            kiln_typed_buffer *buffer = kiln_c99_typed_buffer_of(receiver);
            if (buffer->detached) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot perform ArrayBuffer.prototype.slice on a detached ArrayBuffer");
            }
            size_t start = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                                   buffer->byte_length, 0);
            size_t end = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                                 buffer->byte_length, buffer->byte_length);
            size_t count = end >= start ? end - start : 0;
            status = kiln_c99_new_typed_buffer(result, count);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (count != 0 && buffer->bytes != NULL) {
                kiln_typed_buffer *copy = kiln_c99_typed_buffer_of(*result);
                memcpy(copy->bytes, buffer->bytes + start, count);
            }
            return KILN_C99_OK;
        }
        KILN_C99_REFUSE("an ArrayBuffer method this backend has not implemented");
    }
    if (receiver.tag == KILN_VALUE_TYPED_ARRAY) {
        kiln_typed_array *view = kiln_c99_typed_array_of(receiver);
        if (code == KILN_C99_METHOD_ARRAY_GENERIC_MAP ||
            code == KILN_C99_METHOD_ARRAY_GENERIC_FILTER) {
            /* A method borrowed from Array.prototype is generic: its result is an ordinary
             * Array, even when the receiver is a typed view. Keep the original receiver in the
             * callback's third argument as well; materialising a snapshot here would make
             * `Array.prototype.map.call(view, (value, index, seen) => seen === view)` observe
             * the implementation detail instead of the language value. */
            kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
            kiln_value built;
            if (!kiln_c99_is_callable(callback)) {
                return kiln_c99_throw_not_a(thrown, callback, "function");
            }
            status = kiln_c99_new_array(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            for (index = 0; index < view->length; ++index) {
                kiln_value visit[3];
                kiln_value produced;
                status = kiln_c99_typed_element(view, index, &visit[0]);
                if (status != KILN_C99_OK) {
                    return status;
                }
                visit[1] = kiln_c99_number((double)index);
                visit[2] = receiver;
                status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth,
                                       thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (code != KILN_C99_METHOD_ARRAY_GENERIC_FILTER) {
                    status = kiln_c99_array_push(built, produced);
                } else if (kiln_c99_truthy(produced)) {
                    status = kiln_c99_array_push(built, visit[0]);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = built;
            return KILN_C99_OK;
        }
    }
    if (receiver.tag != KILN_VALUE_ARRAY) {
        /* Redirected onto something that is not an array. The array methods are generic: they
         * walk whatever has a `length` and indexed properties, which is what makes
         * `Array.prototype.slice.call(arguments)` and `[].map.call("abc", f)` work. A snapshot
         * of that walk is taken here and the method runs against it.
         *
         * Only the readers come this way. A mutator would have to write its result back through
         * the array-like, and a snapshot cannot carry that, so those still refuse -- an honest
         * "not implemented" rather than a silent no-op on the caller's object. */
        kiln_value length;
        kiln_value snapshot;
        double count;
        size_t at;
        if (!kiln_c99_reads_only(code)) {
            KILN_C99_REFUSE("an array method that writes back, reached through an array-like");
        }
        /* A number or a boolean has no `length` and no indices, so the walk is empty -- which is
         * why `[].slice.call(7).length` is 0. Asking for the property instead would be answered
         * with "this backend has no such member", and that is a different sentence: the language
         * says the property is absent, not unknown. */
        if (receiver.tag == KILN_VALUE_NUMBER || receiver.tag == KILN_VALUE_BOOLEAN ||
            receiver.tag == KILN_VALUE_SYMBOL || receiver.tag == KILN_VALUE_BIGINT) {
            length = kiln_c99_undefined();
        } else {
            status = kiln_c99_get(host, receiver, "length", 6, &length, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        /* No `length` is an empty walk rather than an error, which is why
         * `[].slice.call(7).length` is 0 and not a throw. */
        count = kiln_c99_as_number(length);
        if (!(count >= 1.0)) {
            count = 0.0;
        } else if (count > 4294967295.0) {
            count = 4294967295.0;
        } else {
            count = floor(count);
        }
        status = kiln_c99_new_array(&snapshot);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (at = 0; at < (size_t)count; ++at) {
            kiln_value element;
            status = kiln_c99_get_computed(host, receiver, kiln_c99_number((double)at), &element,
                                           depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_array_push(snapshot, element);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_invoke_method(host, result, code, snapshot, args, argc, depth, thrown);
    }
    array = kiln_c99_array_of(receiver);
    if (array->frozen && kiln_c99_changes_length(code)) {
        /* A TypeError rather than a silent no-op: an element write has an answer to give without
         * doing anything -- the value it was handed -- and `push` does not. Its answer is the new
         * length, and there is no honest one. */
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot add or remove an element of a frozen array");
    }
    switch (code) {
    case KILN_C99_METHOD_PUSH:
        for (index = 0; index < argc; ++index) {
            status = kiln_c99_array_push(receiver, args[index]);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        *result = kiln_c99_number((double)array->count);
        return KILN_C99_OK;
    case KILN_C99_METHOD_POP:
        if (array->count == 0) {
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        array->count -= 1;
        *result = kiln_val_to_boundary(array->elements[array->count]);
        return KILN_C99_OK;
    case KILN_C99_METHOD_JOIN: {
        kiln_text joined = {NULL, 0, 0};
        size_t length;
        kiln_value separator = argc > 0 ? args[0] : kiln_c99_undefined();
        int has_separator = 0;
        if (separator.tag != KILN_VALUE_UNDEFINED) {
            kiln_value shown = kiln_c99_text(separator);
            if (shown.tag != KILN_VALUE_STRING) {
                KILN_C99_REFUSE("join given a separator with no text form");
            }
            separator = shown;
            has_separator = 1;
        }
        /* The length is read once, as `LengthOfArrayLike` does: an element's `toString` can
         * push or pop, and the walk goes on to the length it started with, reading a vanished
         * index as undefined. */
        length = array->count;
        for (index = 0; index < length; ++index) {
            kiln_val element = index < array->count ? array->elements[index] : KILN_VAL_UNDEFINED;
            if (index != 0 &&
                (has_separator ? kiln_c99_text_append_string(&joined, separator)
                               : kiln_c99_text_append(&joined, ",", 1)) != KILN_C99_OK) {
                free(joined.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            /* A nullish element, and a hole, contribute nothing -- which is what makes
             * `[1, , 3].join()` produce "1,,3" rather than "1,undefined,3". */
            if (element == KILN_VAL_UNDEFINED || element == KILN_VAL_NULL ||
                element == KILN_VAL_HOLE) {
                continue;
            }
            if (KILN_VAL_IS_STRING(element)) {
                const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(element);
                if (s != NULL && s->unit_len > 0) {
                    /* The builder holds UTF-8, so only ASCII bytes can be copied as they are:
                     * a Latin-1 `é` is one byte 0xE9 here and two in UTF-8, and copying it raw
                     * put U+FFFD in every `["é"].join("")` until 2026-09-19. */
                    if (kiln_string_is_utf16(s) ||
                        !kiln_c99_string_is_ascii(kiln_val_to_boundary(element))) {
                        kiln_value shown = kiln_c99_text(kiln_val_to_boundary(element));
                        if (shown.tag != KILN_VALUE_STRING ||
                            kiln_c99_text_append_string(&joined, shown) != KILN_C99_OK) {
                            free(joined.bytes);
                            return KILN_C99_OUT_OF_MEMORY;
                        }
                    } else if (kiln_c99_text_append(&joined, (const char *)kiln_string_latin1(s), s->unit_len) != KILN_C99_OK) {
                        free(joined.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
                continue;
            }
            {
                kiln_value boundary = kiln_val_to_boundary(element);
                kiln_value shown;
                /* Each element is `ToString`ed, which for an object is a call: its own
                 * `toString`, `@@toPrimitive` or `valueOf` -- a Buffer's decoder among them.
                 * Converting one without the call answered "[object Object]" for
                 * `[{ toString() { return "y"; } }].join()` and for every `String(array)`,
                 * `${array}` and `array + ""` that reaches this through `toString`. A symbol
                 * element is the `TypeError` `ToString` gives it. */
                int shown_status = kiln_c99_to_text_argument(host, boundary, &shown, depth, thrown);
                if (shown_status != KILN_C99_OK) {
                    free(joined.bytes);
                    return shown_status;
                }
                if (kiln_c99_text_append_string(&joined, shown) != KILN_C99_OK) {
                    free(joined.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
        }
        *result = kiln_c99_text_value(&joined);
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_INDEX_OF:
    case KILN_C99_METHOD_INCLUDES: {
        kiln_value needle = argc > 0 ? args[0] : kiln_c99_undefined();
        /* Where the walk starts, counting from the end when negative. Ignoring it answered
         * `[1,2,3,4].indexOf(3, 3)` with 2 -- a position before the one the caller asked to
         * start from, which reads as a find rather than as a miss. */
        size_t start = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                               array->count, 0);
        int found = 0;
        for (index = start; index < array->count; ++index) {
            if (array->elements[index] == KILN_VAL_HOLE) {
                /* The only place `indexOf` and `includes` disagree, and the reason `includes`
                 * exists: one skips a hole and the other reads it as undefined. */
                found = code == KILN_C99_METHOD_INCLUDES && needle.tag == KILN_VALUE_UNDEFINED;
                if (found) {
                    break;
                }
                continue;
            }
            kiln_value elem_val = kiln_val_to_boundary(array->elements[index]);
            found = code == KILN_C99_METHOD_INCLUDES
                        ? kiln_c99_same_value_zero(elem_val, needle)
                        : kiln_c99_strict_equal(elem_val, needle);
            if (found) {
                break;
            }
        }
        if (code == KILN_C99_METHOD_INCLUDES) {
            *result = kiln_c99_boolean(found);
        } else {
            *result = kiln_c99_number(found ? (double)index : -1.0);
        }
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_SLICE: {
        size_t start = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t end = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                             array->count, array->count);
        kiln_value built;
        size_t slice_len = (end > start && start < array->count) ? (end < array->count ? end - start : array->count - start) : 0;
        status = kiln_c99_new_array_with_capacity(&built, slice_len, array->is_numeric);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (slice_len > 0) {
            kiln_array *b_arr = kiln_c99_array_of(built);
            memcpy(b_arr->elements, array->elements + start, slice_len * sizeof(kiln_val));
            b_arr->count = slice_len;
        }
        *result = built;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_CONCAT: {
        kiln_value built;
        size_t argument;
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < array->count; ++index) {
            status = kiln_c99_array_push(built, kiln_val_to_boundary(array->elements[index]));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        for (argument = 0; argument < argc; ++argument) {
            /* An array argument contributes its elements, one level deep; anything else
             * contributes itself. */
            if (args[argument].tag == KILN_VALUE_ARRAY) {
                kiln_array *other = kiln_c99_array_of(args[argument]);
                for (index = 0; index < other->count; ++index) {
                    status = kiln_c99_array_push(built, kiln_val_to_boundary(other->elements[index]));
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
            } else {
                status = kiln_c99_array_push(built, args[argument]);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
        }
        *result = built;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_TO_STRING:
        /* `join` with the separator fixed rather than defaulted -- a program can pass one to
         * `join` and cannot to `toString`, which is why `String([1, 2])` is `"1,2"`. */
        return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_JOIN, receiver, NULL, 0, depth,
                                      thrown);
    case KILN_C99_METHOD_ARRAY_FIND_LAST:
    case KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
        size_t at = array->count;
        *result = code == KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX ? kiln_c99_number(-1.0)
                                                                : kiln_c99_undefined();
        while (at > 0) {
            kiln_value visit[3];
            kiln_value produced;
            at -= 1;
            if (at >= array->count) {
                /* The callback shortened the array out from under the walk. */
                continue;
            }
            /* A hole is visited as undefined here, unlike `find`: the language added the
             * last-first pair after it stopped skipping them. */
            visit[0] = array->elements[at] == KILN_VAL_HOLE ? kiln_c99_undefined()
                                                            : kiln_val_to_boundary(array->elements[at]);
            visit[1] = kiln_c99_number((double)at);
            visit[2] = receiver;
            status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (kiln_c99_truthy(produced)) {
                *result = code == KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX
                              ? kiln_c99_number((double)at)
                              : visit[0];
                break;
            }
        }
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_SPLICE: {
        kiln_value removed;
        size_t start = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t removing;
        size_t inserting = argc > 2 ? argc - 2 : 0;
        size_t tail;
        /* A missing count removes everything from `start`, where an explicit `undefined` removes
         * nothing: `splice(1)` and `splice(1, undefined)` are different calls. */
        if (argc < 2) {
            removing = array->count - start;
        } else {
            double wanted = kiln_c99_as_number(args[1]);
            removing = isnan(wanted) || wanted < 0.0        ? 0
                       : wanted > (double)(array->count - start) ? array->count - start
                                                                 : (size_t)wanted;
        }
        status = kiln_c99_new_array(&removed);
        for (index = 0; index < removing && status == KILN_C99_OK; ++index) {
            status = kiln_c99_array_push(removed, kiln_val_to_boundary(array->elements[start + index]));
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        status = kiln_c99_array_reserve(array, array->count - removing + inserting);
        if (status != KILN_C99_OK) {
            return status;
        }
        tail = array->count - start - removing;
        /* An array that has never grown has `elements == NULL`, and then `start`, `removing` and
         * `tail` are all zero -- so this moved nothing, from `NULL + 0` to `NULL + 0`. Computing
         * that offset is undefined even though nothing is read, and `memmove` declares both
         * pointers non-null however small the count. gcc emits code that happens to work, which
         * is why the corpus passed; `zig cc` traps, and UBSan calls it "applying zero offset to
         * null pointer". Guarding on the count says what the code already meant. */
        if (tail != 0) {
            memmove(array->elements + start + inserting, array->elements + start + removing,
                    tail * sizeof *array->elements);
        }
        /* The gap is filled *under* the final count, not before it. `kiln_val_from_boundary`
         * allocates for any string of more than one byte, and the collector traces an array's
         * first `count` elements and no further -- so a value written past `count` is invisible
         * to it, and the very next allocation in this loop collects the string just stored and
         * hands its memory to something else. Publishing the count first requires the gap to
         * hold a traceable value already, hence the fill. */
        for (index = 0; index < inserting; ++index) {
            array->elements[start + index] = KILN_VAL_UNDEFINED;
        }
        array->count = array->count - removing + inserting;
        for (index = 0; index < inserting; ++index) {
            array->elements[start + index] = kiln_val_from_boundary(args[index + 2]);
        }
        /* The removed elements, which is what makes `splice` usable as a queue read. */
        *result = removed;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_COPY_WITHIN: {
        size_t target = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                                array->count, 0);
        size_t start = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t end = kiln_c99_relative_index(argc > 2 ? args[2] : kiln_c99_undefined(),
                                             array->count, array->count);
        size_t moving = end > start ? end - start : 0;
        if (moving > array->count - target) {
            moving = array->count - target;
        }
        /* `memmove` rather than a loop, because the source and the destination overlap by design
         * -- that overlap is the whole reason the method exists. */
        memmove(array->elements + target, array->elements + start,
                moving * sizeof *array->elements);
        *result = receiver;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_REDUCE_RIGHT: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value accumulator;
        size_t at = array->count;
        if (argc > 1) {
            accumulator = args[1];
        } else {
            /* The seed is the last element that is *there*: an array of nothing but holes has no
             * answer to give, exactly as an empty one does not. */
            while (at > 0 && array->elements[at - 1] == KILN_VAL_HOLE) {
                at -= 1;
            }
            if (at == 0) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Reduce of empty array with no initial value");
            }
            at -= 1;
            accumulator = kiln_val_to_boundary(array->elements[at]);
        }
        while (at > 0) {
            kiln_value visit[4];
            at -= 1;
            if (at >= array->count || array->elements[at] == KILN_VAL_HOLE) {
                continue;
            }
            visit[0] = accumulator;
            visit[1] = kiln_val_to_boundary(array->elements[at]);
            visit[2] = kiln_c99_number((double)at);
            visit[3] = receiver;
            status = kiln_c99_call(host, &accumulator, callback, kiln_c99_undefined(), visit, 4,
                                   depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        *result = accumulator;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FLAT_MAP:
    case KILN_C99_METHOD_MAP:
    case KILN_C99_METHOD_ARRAY_GENERIC_MAP:
    case KILN_C99_METHOD_ARRAY_PARALLEL_MAP:
    case KILN_C99_METHOD_FILTER:
    case KILN_C99_METHOD_ARRAY_GENERIC_FILTER:
    case KILN_C99_METHOD_FOR_EACH: {
        kiln_value built = kiln_c99_undefined();
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_val vcallee = kiln_val_from_boundary(callback);
        if ((code == KILN_C99_METHOD_MAP || code == KILN_C99_METHOD_ARRAY_PARALLEL_MAP) &&
            KILN_VAL_IS_CLOSURE(vcallee) && self_value.tag == KILN_VALUE_UNDEFINED) {
            kiln_value built_val;
            status = kiln_c99_new_array_with_capacity(&built_val, array->count, array->is_numeric);
            if (status != KILN_C99_OK) return status;
            kiln_array *built_arr = kiln_c99_array_of(built_val);
            kiln_val vrecv = kiln_val_from_boundary(receiver);
            size_t length = array->count;
            for (index = 0; index < length && index < array->count; ++index) {
                if (array->elements[index] == KILN_VAL_HOLE) {
                    built_arr->elements[built_arr->count++] = KILN_VAL_HOLE;
                    built_arr->is_numeric = 0;
                    continue;
                }
                kiln_val vvisit[3];
                vvisit[0] = array->elements[index];
                vvisit[1] = kiln_c99_val_from_double((double)index);
                vvisit[2] = vrecv;
                kiln_val prod = KILN_VAL_UNDEFINED;
                status = kiln_c99_val_call(host, vcallee, vvisit, 3, depth, thrown, NULL, 0, NULL, &prod);
                if (status != KILN_C99_OK) return status;
                if (built_arr->is_numeric && !KILN_VAL_IS_NUMBER(prod)) {
                    built_arr->is_numeric = 0;
                }
                built_arr->elements[built_arr->count++] = prod;
            }
            *result = built_val;
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_FILTER && KILN_VAL_IS_CLOSURE(vcallee) && self_value.tag == KILN_VALUE_UNDEFINED) {
            kiln_value built_val;
            status = kiln_c99_new_array_with_capacity(&built_val, array->count, array->is_numeric);
            if (status != KILN_C99_OK) return status;
            kiln_array *built_arr = kiln_c99_array_of(built_val);
            kiln_val vrecv = kiln_val_from_boundary(receiver);
            size_t length = array->count;
            for (index = 0; index < length && index < array->count; ++index) {
                if (array->elements[index] == KILN_VAL_HOLE) {
                    continue;
                }
                kiln_val elem = array->elements[index];
                kiln_val vvisit[3];
                vvisit[0] = elem;
                vvisit[1] = kiln_c99_val_from_double((double)index);
                vvisit[2] = vrecv;
                kiln_val prod = KILN_VAL_UNDEFINED;
                status = kiln_c99_val_call(host, vcallee, vvisit, 3, depth, thrown, NULL, 0, NULL, &prod);
                if (status != KILN_C99_OK) return status;
                if (kiln_c99_val_truthy(prod)) {
                    built_arr->elements[built_arr->count++] = elem;
                }
            }
            *result = built_val;
            return KILN_C99_OK;
        }
        if (code != KILN_C99_METHOD_FOR_EACH) {
            status = kiln_c99_new_array(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        /* The length is read once, so an element the callback appends is not visited -- which is
         * what the specification says and what stops an appending callback from looping. */
        {
            size_t length = array->count;
            for (index = 0; index < length && index < array->count; ++index) {
                kiln_value visit[3];
                kiln_value produced;
                /* Every element walk skips a hole and visits an undefined. `map` still has to
                 * *leave* one: its result has an entry per input position, and skipping would
                 * shift everything after it. */
                if (array->elements[index] == KILN_VAL_HOLE) {
                    if (code == KILN_C99_METHOD_MAP ||
                        code == KILN_C99_METHOD_ARRAY_GENERIC_MAP ||
                        code == KILN_C99_METHOD_ARRAY_PARALLEL_MAP) {
                        status = kiln_c99_array_push(built, kiln_c99_hole());
                        if (status != KILN_C99_OK) {
                            return status;
                        }
                    }
                    continue;
                }
                visit[0] = kiln_val_to_boundary(array->elements[index]);
                visit[1] = kiln_c99_number((double)index);
                visit[2] = receiver;
                status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (code == KILN_C99_METHOD_MAP ||
                    code == KILN_C99_METHOD_ARRAY_GENERIC_MAP ||
                    code == KILN_C99_METHOD_ARRAY_PARALLEL_MAP ||
                    code == KILN_C99_METHOD_ARRAY_FLAT_MAP) {
                    status = kiln_c99_array_push(built, produced);
                } else if ((code == KILN_C99_METHOD_FILTER ||
                            code == KILN_C99_METHOD_ARRAY_GENERIC_FILTER) &&
                           kiln_c99_truthy(produced)) {
                    status = kiln_c99_array_push(built, visit[0]);
                } else {
                    status = KILN_C99_OK;
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
        }
        if (code == KILN_C99_METHOD_ARRAY_FLAT_MAP) {
            /* One level, and only of what the callback produced -- `flatMap` does not flatten an
             * array that was already an element. */
            kiln_value one = kiln_c99_number(1.0);
            return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_ARRAY_FLAT, built, &one, 1,
                                          depth, thrown);
        }
        *result = built;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_AT: {
        /* A negative index counts from the end, which is the whole reason `at` exists alongside
         * indexing; out of range is undefined rather than clamped. */
        double relative = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        double position;
        /* `ToIntegerOrInfinity` first, so a NaN index is zero and not out of range: `[1].at(NaN)`
         * is `[1].at(0)`. Without it every comparison below is false for a NaN and the answer
         * came back undefined -- the spelling of "there is nothing there", for an index that
         * names the first element. The string `at` beside this one already had the check. */
        if (relative != relative) {
            relative = 0.0;
        }
        relative = trunc(relative);
        position = relative < 0.0 ? (double)array->count + relative : relative;
        *result = position >= 0.0 && position < (double)array->count &&
                          array->elements[(size_t)position] != KILN_VAL_HOLE
                      ? kiln_val_to_boundary(array->elements[(size_t)position])
                      : kiln_c99_undefined();
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_LAST_INDEX_OF: {
        kiln_value needle = argc > 0 ? args[0] : kiln_c99_undefined();
        double found = -1.0;
        /* `fromIndex` is where the *backwards* walk starts, counting from the end when
         * negative -- and a negative that runs off the front leaves nowhere to start, which is
         * -1 rather than the whole array. That last case is why this cannot use
         * `kiln_c99_relative_index`, which clamps such a position to zero and would go on to
         * report a match at the very element the caller excluded. */
        size_t limit = array->count;
        if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
            double from = kiln_c99_as_number(args[1]);
            if (from != from) {
                from = 0.0;
            }
            from = trunc(from);
            if (from < 0.0) {
                from += (double)array->count;
            }
            if (from < 0.0) {
                *result = kiln_c99_number(-1.0);
                return KILN_C99_OK;
            }
            limit = from >= (double)array->count ? array->count : (size_t)from + 1;
        }
        for (index = limit; index > 0; --index) {
            if (array->elements[index - 1] == KILN_VAL_HOLE) {
                continue;
            }
            if (kiln_c99_strict_equal(kiln_val_to_boundary(array->elements[index - 1]), needle)) {
                found = (double)(index - 1);
                break;
            }
        }
        *result = kiln_c99_number(found);
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FIND:
    case KILN_C99_METHOD_ARRAY_FIND_INDEX:
    case KILN_C99_METHOD_ARRAY_SOME:
    case KILN_C99_METHOD_ARRAY_EVERY: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
        size_t length = array->count;
        int matched = 0;
        /* `every` is the only one that answers true for an empty array, and the only one that
         * stops on a *falsy* result. Everything else about the four is the same walk. */
        for (index = 0; index < length && index < array->count; ++index) {
            kiln_value visit[3];
            kiln_value produced;
            /* `find` and `findIndex` visit a hole as undefined, where `some` and `every` skip
             * it -- the same split `includes` and `indexOf` make. */
            if (array->elements[index] == KILN_VAL_HOLE) {
                if (code == KILN_C99_METHOD_ARRAY_SOME || code == KILN_C99_METHOD_ARRAY_EVERY) {
                    continue;
                }
                visit[0] = kiln_c99_undefined();
            } else {
                visit[0] = kiln_val_to_boundary(array->elements[index]);
            }
            visit[1] = kiln_c99_number((double)index);
            visit[2] = receiver;
            status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            matched = kiln_c99_truthy(produced);
            if (code == KILN_C99_METHOD_ARRAY_EVERY ? !matched : matched) {
                break;
            }
        }
        switch (code) {
        case KILN_C99_METHOD_ARRAY_FIND:
            *result = matched && index < array->count ? kiln_val_to_boundary(array->elements[index])
                                                      : kiln_c99_undefined();
            if (result->tag == KILN_VALUE_HOLE) {
                *result = kiln_c99_undefined();
            }
            break;
        case KILN_C99_METHOD_ARRAY_FIND_INDEX:
            *result = kiln_c99_number(matched ? (double)index : -1.0);
            break;
        case KILN_C99_METHOD_ARRAY_SOME: *result = kiln_c99_boolean(matched); break;
        default: *result = kiln_c99_boolean(index >= length || matched); break;
        }
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_SORT: {
        /* An insertion sort, which is stable -- and stability is program-visible, so it is part
         * of the answer rather than an implementation detail. The comparator can call back into
         * the program, so every comparison has to be able to fail. */
        kiln_value comparator = argc > 0 ? args[0] : kiln_c99_undefined();
        size_t sorted;
        /* Holes and undefined sort to the end, ahead of the comparator ever seeing them. */
        size_t live = 0;
        /* `undefined` means the default ordering; anything else has to be callable. The check
         * runs before a single element moves, so a refused sort leaves the array exactly as it
         * was -- and it lives here rather than in `toSorted` because both need it and only one
         * of them should own it. Falling through to the text comparison instead was a silent
         * wrong answer: `a.sort("nope")` reordered the array where every other engine throws. */
        if (comparator.tag != KILN_VALUE_UNDEFINED && !kiln_c99_is_callable(comparator)) {
            return kiln_c99_throw_error(
                thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                "The comparison function must be either a function or undefined");
        }
        for (index = 0; index < array->count; ++index) {
            if (array->elements[index] != KILN_VAL_HOLE &&
                array->elements[index] != KILN_VAL_UNDEFINED) {
                array->elements[live++] = array->elements[index];
            }
        }
        for (index = live; index < array->count; ++index) {
            array->elements[index] = KILN_VAL_UNDEFINED;
        }
        if (live > 1) {
            kiln_c99_sort_context ctx;
            ctx.host = host;
            ctx.comparator = comparator;
            ctx.depth = depth;
            ctx.thrown = thrown;
            status = kiln_c99_timsort_val(&ctx, array->elements, live);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        /* The array itself, so `a.sort()` can be chained and `a.sort() === a` holds. */
        *result = receiver;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_REVERSE: {
        size_t half;
        for (half = 0; half * 2 + 1 < array->count; ++half) {
            kiln_val swap = array->elements[half];
            array->elements[half] = array->elements[array->count - 1 - half];
            array->elements[array->count - 1 - half] = swap;
        }
        *result = receiver;
        return KILN_C99_OK;
    }
    /* The four ES2023 change-by-copy methods, each the counterpart of one that mutates: `with`
     * to an index assignment, `toReversed` to `reverse`, `toSorted` to `sort`, `toSpliced` to
     * `splice`. They share a case because they share a shape -- read the receiver into a fresh
     * array, then let the counterpart rearrange *that* -- and sharing it is what keeps them
     * honest: one ordering and one splice, so a copy and its mutating twin can never disagree
     * about where a comparator's ties land or how a count is clamped.
     *
     * Every hole becomes `undefined` on the way in, which is the other half of what these
     * methods promise: the result is dense however sparse the receiver was.
     *
     * The receiver is read *before* the counterpart runs, so a comparator that reaches back and
     * changes it cannot change the answer -- which is the property that made the language add
     * these in the first place. */
    case KILN_C99_METHOD_ARRAY_TO_SORTED:
    case KILN_C99_METHOD_ARRAY_TO_REVERSED:
    case KILN_C99_METHOD_ARRAY_TO_SPLICED:
    case KILN_C99_METHOD_ARRAY_WITH: {
        kiln_value copy;
        kiln_value discarded;
        size_t replacing = 0;
        if (code == KILN_C99_METHOD_ARRAY_WITH) {
            /* `ToIntegerOrInfinity`, and then a negative index counts from the end. An index
             * that lands outside the array is a `RangeError` rather than an appended element:
             * `with` answers a copy of the *same* length, and there is nowhere to put the value.
             *
             * `kiln_c99_relative_index` cannot be borrowed for this. It clamps, which is the
             * answer `slice` and `at` want and is precisely the one this has to refuse -- a
             * clamp would quietly write over the first or last element instead of throwing. */
            double relative = kiln_c99_as_number(argc > 0 ? args[0] : kiln_c99_undefined());
            double at;
            relative = isnan(relative) ? 0.0 : trunc(relative);
            at = relative >= 0.0 ? relative : (double)array->count + relative;
            if (!(at >= 0.0) || at >= (double)array->count) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "Invalid index");
            }
            replacing = (size_t)at;
        }
        status = kiln_c99_new_array(&copy);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < array->count; ++index) {
            kiln_value element = array->elements[index] == KILN_VAL_HOLE
                                     ? kiln_c99_undefined()
                                     : kiln_val_to_boundary(array->elements[index]);
            if (code == KILN_C99_METHOD_ARRAY_WITH && index == replacing) {
                /* Stored as it was handed over: the replacement is the one argument of these
                 * four that must not be converted to anything. */
                element = argc > 1 ? args[1] : kiln_c99_undefined();
            }
            status = kiln_c99_array_push(copy, element);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        /* The copy is the answer in every case, including the three that hand it on: `sort`,
         * `reverse` and `splice` all rearrange it in place, and only `splice` has a return value
         * of its own -- the elements it removed, which is exactly what `toSpliced` must *not*
         * answer. */
        *result = copy;
        if (code == KILN_C99_METHOD_ARRAY_WITH) {
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_ARRAY_TO_SPLICED) {
            /* `toSpliced()` with no arguments at all is a plain copy -- an absent `start` skips
             * nothing -- where `toSpliced(1)` has a start and no count and so runs to the end.
             * `splice` reads a missing second argument as "to the end" for both, which is right
             * for the one-argument form and wrong for the empty call it shares that branch with,
             * so the empty call is answered here rather than handed down. */
            if (argc == 0) {
                return KILN_C99_OK;
            }
            return kiln_c99_invoke_method(host, &discarded, KILN_C99_METHOD_ARRAY_SPLICE, copy,
                                          args, argc, depth, thrown);
        }
        return kiln_c99_invoke_method(host, &discarded,
                                      code == KILN_C99_METHOD_ARRAY_TO_SORTED
                                          ? KILN_C99_METHOD_ARRAY_SORT
                                          : KILN_C99_METHOD_ARRAY_REVERSE,
                                      copy, args, argc, depth, thrown);
    }
    case KILN_C99_METHOD_ARRAY_SHIFT: {
        if (array->count == 0) {
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        *result = array->elements[0] == KILN_VAL_HOLE ? kiln_c99_undefined()
                                                       : kiln_val_to_boundary(array->elements[0]);
        memmove(array->elements, array->elements + 1,
                (array->count - 1) * sizeof *array->elements);
        array->count -= 1;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_UNSHIFT: {
        size_t added;
        status = kiln_c99_array_reserve(array, array->count + argc);
        if (status != KILN_C99_OK) {
            return status;
        }
        memmove(array->elements + argc, array->elements, array->count * sizeof *array->elements);
        /* Same ordering as `splice` above, and for the same reason: the opened gap is made
         * traceable and the count published before any conversion allocates into it. */
        for (added = 0; added < argc; ++added) {
            array->elements[added] = KILN_VAL_UNDEFINED;
        }
        array->count += argc;
        for (added = 0; added < argc; ++added) {
            array->elements[added] = kiln_val_from_boundary(args[added]);
        }
        *result = kiln_c99_number((double)array->count);
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FILL: {
        kiln_value filler = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_val filler_val = kiln_val_from_boundary(filler);
        size_t start = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t end = kiln_c99_relative_index(argc > 2 ? args[2] : kiln_c99_undefined(),
                                             array->count, array->count);
        for (index = start; index < end; ++index) {
            array->elements[index] = filler_val;
        }
        *result = receiver;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FLAT: {
        /* One level unless told otherwise, and a hole disappears rather than flattening to
         * undefined. */
        double depth_wanted = argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED
                                  ? kiln_c99_as_number(args[0])
                                  : 1.0;
        kiln_value built;
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
            kiln_val elem_v = array->elements[index];
            if (elem_v == KILN_VAL_HOLE) {
                continue;
            }
            kiln_value element = kiln_val_to_boundary(elem_v);
            if (element.tag == KILN_VALUE_ARRAY && depth_wanted >= 1.0) {
                kiln_value flattened;
                kiln_value inner_depth = kiln_c99_number(depth_wanted - 1.0);
                kiln_array *nested;
                size_t inner;
                status = kiln_c99_invoke_method(host, &flattened, KILN_C99_METHOD_ARRAY_FLAT,
                                                element, &inner_depth, 1, depth + 1, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                nested = kiln_c99_array_of(flattened);
                for (inner = 0; inner < nested->count && status == KILN_C99_OK; ++inner) {
                    status = kiln_c99_array_push(built, kiln_val_to_boundary(nested->elements[inner]));
                }
                continue;
            }
            status = kiln_c99_array_push(built, element);
        }
        *result = built;
        return status;
    }
    case KILN_C99_METHOD_ARRAY_ENTRIES:
    case KILN_C99_METHOD_ARRAY_KEYS: {
        kiln_value snapshot;
        status = kiln_c99_new_array(&snapshot);
        for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
            kiln_value entry = kiln_c99_number((double)index);
            if (code == KILN_C99_METHOD_ARRAY_ENTRIES) {
                kiln_value pair;
                status = kiln_c99_new_array(&pair);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(pair, entry);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(pair, array->elements[index] == KILN_VAL_HOLE
                                                           ? kiln_c99_undefined()
                                                           : kiln_val_to_boundary(array->elements[index]));
                }
                entry = pair;
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_array_push(snapshot, entry);
            }
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_new_iterator(result, snapshot);
    }
    case KILN_C99_METHOD_REDUCE: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value accumulator;
        size_t length = array->count;
        index = 0;
        if (argc > 1) {
            accumulator = args[1];
        } else {
            /* Without a seed the accumulator is the first element that is *there*: folding a
             * hole in would turn every sum over a sparse array into NaN. An array of nothing but
             * holes has no answer to give, exactly as an empty one does not. */
            while (index < length && array->elements[index] == KILN_VAL_HOLE) {
                index += 1;
            }
            if (index >= length) {
                /* A TypeError the program can catch, not a refusal: `[].reduce(f)` is ordinary
                 * code with an ordinary error, and a fatal status would make it unwritable. */
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Reduce of empty array with no initial value");
            }
            accumulator = kiln_val_to_boundary(array->elements[index]);
            index += 1;
        }
        kiln_val vcallee = kiln_val_from_boundary(callback);
        if (KILN_VAL_IS_CLOSURE(vcallee)) {
            kiln_val vacc = kiln_val_from_boundary(accumulator);
            kiln_val vrecv = kiln_val_from_boundary(receiver);
            for (; index < length && index < array->count; ++index) {
                if (array->elements[index] == KILN_VAL_HOLE) {
                    continue;
                }
                kiln_val vvisit[4];
                vvisit[0] = vacc;
                vvisit[1] = array->elements[index];
                vvisit[2] = kiln_c99_val_from_double((double)index);
                vvisit[3] = vrecv;
                status = kiln_c99_val_call(host, vcallee, vvisit, 4, depth, thrown, NULL, 0, NULL, &vacc);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = kiln_val_to_boundary(vacc);
            return KILN_C99_OK;
        }
        for (; index < length && index < array->count; ++index) {
            kiln_value visit[4];
            if (array->elements[index] == KILN_VAL_HOLE) {
                continue;
            }
            visit[0] = accumulator;
            visit[1] = kiln_val_to_boundary(array->elements[index]);
            visit[2] = kiln_c99_number((double)index);
            visit[3] = receiver;
            status = kiln_c99_call(host, &accumulator, callback, kiln_c99_undefined(), visit, 4,
                                   depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        *result = accumulator;
        return KILN_C99_OK;
    }
    default:
        KILN_C99_REFUSE("an array method this backend has not implemented");
    }
}
