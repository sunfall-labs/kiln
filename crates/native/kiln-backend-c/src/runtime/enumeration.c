
/* Append everything `source` yields to `target`, which is what array spread, `for...of` and
 * destructuring all reduce to.
 *
 * An array and a string are walked directly rather than through their iterators. That is a
 * shortcut the specification does not take, and it is visible to a program that replaces
 * `Array.prototype[Symbol.iterator]` -- which this backend has no way to do, so the shortcut
 * cannot be observed here. Anything else must hand back an iterator and is driven by `next`. */
static int kiln_c99_iterate(kiln_host *host, kiln_value target, kiln_value source, int depth,
                            kiln_value *thrown) {
    kiln_value iterator;
    size_t index;
    int status;
    if (source.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(source);
        size_t length = array->count;
        for (index = 0; index < length && index < array->count; ++index) {
            /* Iteration *reads* each position, so a hole arrives as the undefined it reads as.
             * That is what makes `[...[1, , 3]]` a dense three-element array where `map` over
             * the same input leaves the hole alone. */
            status = kiln_c99_array_push(target, array->elements[index] == KILN_VAL_HOLE
                                                     ? kiln_c99_undefined()
                                                     : kiln_val_to_boundary(array->elements[index]));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return KILN_C99_OK;
    }
    if (source.tag == KILN_VALUE_TYPED_ARRAY) {
        kiln_typed_array *view = kiln_c99_typed_array_of(source);
        /* Read each element at the moment it is visited: a view is live over its shared buffer. */
        for (index = 0; index < view->length; ++index) {
            kiln_value element;
            status = kiln_c99_typed_element(view, index, &element);
            if (status == KILN_C99_OK) status = kiln_c99_array_push(target, element);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return KILN_C99_OK;
    }
    if (source.tag == KILN_VALUE_COLLECTION) {
        /* A Map yields `[key, value]` pairs and a Set yields values, which is what makes
         * `for (const [k, v] of map)` and `for (const v of set)` both read naturally. */
        kiln_collection *collection = kiln_c99_collection_of(source);
        size_t length = collection->count;
        for (index = 0; index < length && index < collection->count; ++index) {
            kiln_value produced;
            if (collection->entries[index].dead) {
                continue;
            }
            produced = collection->entries[index].value;
            if (collection->is_map) {
                status = kiln_c99_new_array(&produced);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(produced, collection->entries[index].key);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(produced, collection->entries[index].value);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            status = kiln_c99_array_push(target, produced);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return KILN_C99_OK;
    }
    if (source.tag == KILN_VALUE_STRING) {
        /* A string iterates as its *characters*, not its code units: an astral character is one
         * step, which is the difference between iterating a string and indexing it. */
        uint16_t *units = NULL;
        size_t count = 0;
        status = kiln_c99_to_units(source, &units, &count);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < count; ++index) {
            size_t width = (units[index] >= 0xD800 && units[index] <= 0xDBFF &&
                            index + 1 < count && units[index + 1] >= 0xDC00 &&
                            units[index + 1] <= 0xDFFF)
                               ? 2
                               : 1;
            status = kiln_c99_array_push(target, kiln_c99_from_units(units + index, width));
            if (status != KILN_C99_OK) {
                free(units);
                return status;
            }
            index += width - 1;
        }
        free(units);
        return KILN_C99_OK;
    }
    /* Nullish is answered here rather than by the `@@iterator` lookup below, because the two
     * faults are reported differently: reading a property off null is "Cannot read properties of
     * null", but *iterating* it is "null is not iterable", and `[...null]` is the second one. */
    if (source.tag == KILN_VALUE_UNDEFINED || source.tag == KILN_VALUE_NULL) {
        return kiln_c99_throw_not_iterable(thrown, source);
    }
    /* Everything else goes through the protocol: ask for an iterator, then drive it. */
    if (source.tag == KILN_VALUE_GENERATOR) {
        iterator = source;
    } else {
        kiln_value method;
        status = kiln_c99_probe_symbol(host, source, "@@iterator", 10, &method, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        /* No `@@iterator` at all is the ordinary case -- `for (const x of 5)` and `[...null]` --
         * and the language ends the program there with a TypeError the program may catch, rather
         * than a fault. The lookup above already answered undefined for a missing `@@` name, so
         * reaching here means the value genuinely has nothing to iterate with. */
        if (!kiln_c99_is_callable(method)) {
            return kiln_c99_throw_not_iterable(thrown, source);
        }
        status = kiln_c99_call(host, &iterator, method, source, NULL, 0, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    for (;;) {
        kiln_value next;
        kiln_value step;
        kiln_value done;
        kiln_value value;
        status = kiln_c99_get(host, iterator, "next", 4, &next, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        /* An iterator whose `next` is not callable is a broken protocol, not a missing feature:
         * the language reports it against `next` itself, which is what the interpreter prints. */
        if (!kiln_c99_is_callable(next)) {
            return kiln_c99_throw_not_a(thrown, next, "function");
        }
        status = kiln_c99_call(host, &step, next, iterator, NULL, 0, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        status = kiln_c99_get(host, step, "done", 4, &done, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (kiln_c99_truthy(done)) {
            /* The value that comes back with `done` is the iterator's *return* value, which
             * spread and `for...of` both discard. */
            return KILN_C99_OK;
        }
        status = kiln_c99_get(host, step, "value", 5, &value, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        status = kiln_c99_array_push(target, value);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
}

/* A reserved key spelling: every symbol-keyed property reaches this layer as `@@name`, whether
 * it is well-known or came from `Symbol.for`. Written out inline in half a dozen places; named
 * here because two membership predicates deliberately answer *every* one of them and the `in`
 * operator must not. */
static int kiln_c99_is_symbol_key(const char *key, size_t key_len) {
    return key_len > 2 && key[0] == '@' && key[1] == '@';
}

/* Whether a built-in object -- a prototype table, a constructor, a namespace -- carries a name,
 * own or inherited.
 *
 * `in` asks whether the property *exists*, so this answers out of the membership predicates
 * rather than by attempting a read: a read refuses a member of the language this backend has not
 * built, which is the right answer for `String.prototype.anchor` and the wrong one for
 * `"anchor" in String.prototype` -- that question is answerable and the answer is true. It is the
 * same split the regexp arm below already makes for an instance.
 *
 * Without an arm of its own a built-in fell through to the object chain walk, which stops at the
 * first value that is not an object -- so every name was false. `"map" in Array.prototype` denied
 * a member `Array.prototype.map` demonstrably has, and a feature test that asks before calling
 * took the branch written for a realm that never had it. That is how `@kiln/concurrency` came to
 * install a polyfill over `parallelMap` on the one backend that had just grown it. */
static int kiln_c99_builtin_has_member(uint64_t builtin, const char *key, size_t key_len) {
    /* What this backend actually answers for, which is a subset of the language and the only
     * part a read can produce. */
    if (kiln_c99_namespace_method(builtin, key, key_len) != 0) {
        return 1;
    }
    if (builtin == KILN_C99_BUILTIN_ARRAY_PROTOTYPE &&
        (kiln_c99_same_name(key, key_len, "@@iterator") ||
         kiln_c99_same_name(key, key_len, "@@unscopables") ||
         kiln_c99_same_name(key, key_len, "length"))) {
        return 1;
    }
    /* A symbol key is settled by the lookup above and nowhere else. The per-kind predicates
     * below answer *every* `@@` name, which is right for the reads they were written for and
     * wrong here for the same reason as on a date instance: `Symbol.for("x") in Array.prototype`
     * is false, and only the well-known members the lookup knows about are true. */
    if (kiln_c99_is_symbol_key(key, key_len)) {
        return 0;
    }
    /* ...and the rest of what the *language* puts on this prototype, whether or not this backend
     * has built it. One predicate per receiver kind, the same ones the reads consult. */
    switch (builtin) {
    case KILN_C99_BUILTIN_ARRAY_PROTOTYPE:
        if (kiln_c99_is_array_member(key, key_len)) return 1;
        break;
    case KILN_C99_BUILTIN_STRING_PROTOTYPE:
        if (kiln_c99_is_string_member(key, key_len)) return 1;
        break;
    case KILN_C99_BUILTIN_DATE_PROTOTYPE:
        if (kiln_c99_is_date_member(key, key_len)) return 1;
        break;
    case KILN_C99_BUILTIN_MAP_PROTOTYPE:
    case KILN_C99_BUILTIN_SET_PROTOTYPE:
    case KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE:
    case KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE:
        if (kiln_c99_collection_kind_member(
                builtin == KILN_C99_BUILTIN_MAP_PROTOTYPE ||
                    builtin == KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE,
                builtin == KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE ||
                    builtin == KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE,
                key, key_len)) {
            return 1;
        }
        break;
    case KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE:
        if (kiln_c99_is_data_view_member(key, key_len)) return 1;
        break;
    case KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE:
        if (kiln_c99_same_name(key, key_len, "slice") ||
            kiln_c99_same_name(key, key_len, "byteLength") ||
            kiln_c99_is_array_buffer_member(key, key_len)) return 1;
        break;
    default:
        break;
    }
    /* A built-in that is a function answers to a callable's own members; a namespace does not.
     * The read path draws the same line. */
    if (!kiln_c99_is_namespace(builtin) &&
        (kiln_c99_callable_method(key, key_len) != 0 ||
         kiln_c99_same_name(key, key_len, "name") ||
         kiln_c99_same_name(key, key_len, "length") ||
         kiln_c99_same_name(key, key_len, "prototype"))) {
        return 1;
    }
    /* The chain ends at `Object.prototype` for all of them, which is what makes
     * `"hasOwnProperty" in Array.prototype` true -- the shape half the feature-detects in the
     * wild are written as. The same three the read path hands out there, plus `constructor`,
     * which every prototype table names. */
    return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                  KILN_C99_METHOD_OBJECT_VALUE_OF) != 0 ||
           kiln_c99_same_name(key, key_len, "constructor") ||
           kiln_c99_same_name(key, key_len, "toString") ||
           kiln_c99_same_name(key, key_len, "toLocaleString");
}

/* `key in target`: whether the name is reachable, own or inherited. Unlike a read this does not
 * call an accessor -- the question is whether the property exists, not what it produces. */
static int kiln_c99_has_property_key(kiln_host *host, kiln_value target, const kiln_string *key,
                                     const kiln_string *ikey, int *answer, int depth,
                                     kiln_value *thrown);

static int kiln_c99_has_key(kiln_host *host, kiln_value target, const kiln_string *view,
                            const kiln_string *canonical, int *answer, int depth,
                            kiln_value *thrown) {
    return kiln_c99_has_property_key(host, target, view, canonical, answer, depth, thrown);
}

static int kiln_c99_has_property_key(kiln_host *host, kiln_value target, const kiln_string *key,
                                     const kiln_string *ikey, int *answer, int depth,
                                     kiln_value *thrown) {
    size_t key_len = key == NULL ? 0 : key->unit_len;
    char intrinsic_buffer[256];
    const char *intrinsic_key = NULL;
    size_t intrinsic_len = 0;
    kiln_value current = target;
    size_t steps = 0;
    *answer = 0;
    if (kiln_c99_is_node_hidden_key_key(key)) {
        return KILN_C99_OK;
    }
kiln_has_next_prototype:
    if (steps >= KILN_C99_MAX_PROTOTYPE_DEPTH)
        KILN_C99_REFUSE("a property-membership prototype chain beyond the traversal limit");
    current = target;
    if (target.tag == KILN_VALUE_NULL) return KILN_C99_OK;
    if (target.tag == KILN_VALUE_PROXY) {
        kiln_value exposed = kiln_c99_key_value(key, &ikey);
        if (exposed.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        return kiln_c99_proxy_has(host, target, exposed, answer, depth, thrown);
    }
    if (target.tag == KILN_VALUE_SHARED) {
        kiln_value ignored;
        kiln_value prototype;
        int status = kiln_c99_shared_own_read(target, key, &ignored, answer, NULL);
        if (status != KILN_C99_OK || *answer) return status;
        status = kiln_c99_shared_prototype(target, &prototype);
        if (status != KILN_C99_OK || prototype.tag == KILN_VALUE_NULL) return status;
        return kiln_c99_has_property_key(host, prototype, key, ikey, answer, depth, thrown);
    }
    if (target.tag == KILN_VALUE_BUILTIN) {
        if (target.bits == KILN_C99_BUILTIN_REGEXP_PROTOTYPE &&
            kiln_c99_same_name_key(key, "@@toStringTag")) {
            *answer = 1;
            return KILN_C99_OK;
        }
        kiln_value store;
        if (kiln_c99_callable_store(target, &store, 0) == KILN_C99_OK &&
            kiln_c99_store_has(store, key, ikey)) {
            *answer = 1;
            return KILN_C99_OK;
        }
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        *answer = kiln_c99_builtin_has_member(target.bits, intrinsic_key, intrinsic_len);
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_GENERATOR) {
        kiln_value store = kiln_c99_generator_of(target)->properties;
        if (store.tag == KILN_VALUE_OBJECT) {
            int status = kiln_c99_has_property_key(host, store, key, ikey, answer, depth, thrown);
            if (status != KILN_C99_OK || *answer) return status;
        }
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        *answer = kiln_c99_same_name_key(key, "throw") ||
            (kiln_c99_same_name_key(key, "@@asyncIterator") && kiln_c99_generator_of(target)->async_generator) ||
            kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_GENERATOR_NEXT,
                                    KILN_C99_METHOD_GENERATOR_SELF) != 0;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_PROMISE) {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        /* Mirrors the promise read: its own store first, then the three methods and
         * `constructor`. Without an arm it fell through the chain walk below and answered false
         * about `then` -- the one name every promise-detecting `then` in the wild asks for. */
        kiln_promise *promise = kiln_c99_promise_of(target);
        if (promise->properties.tag == KILN_VALUE_OBJECT) {
            int carried = 0;
            int status = kiln_c99_has_property_key(host, promise->properties, key, ikey, &carried,
                                               depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (carried) {
                *answer = 1;
                return KILN_C99_OK;
            }
        }
        *answer = kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_THEN,
                                         KILN_C99_METHOD_FINALLY) != 0 ||
                  kiln_c99_same_name_key(key, "constructor") ||
                  kiln_c99_same_name_key(key, "@@toStringTag") ||
                  kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                         KILN_C99_METHOD_OBJECT_VALUE_OF) != 0;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_closure *closure = kiln_c99_closure_of(target);
        kiln_value store = kiln_c99_closure_store(closure);
        /* Membership asks only about own slots and then the actual [[Prototype]]. Reading an
         * inherited static symbol already follows this chain; `in` must follow it too without
         * invoking a getter. Callable methods belong to Function.prototype, so an explicit null
         * prototype must also end their membership rather than synthesizing them here. */
        if (kiln_c99_store_has(store, key, ikey) ||
            kiln_c99_same_name_key(key, "name") || kiln_c99_same_name_key(key, "length") ||
            (kiln_c99_same_name_key(key, "prototype") &&
             kiln_c99_function_owns_prototype(closure))) {
            *answer = 1;
            return KILN_C99_OK;
        }
        target = store.tag == KILN_VALUE_OBJECT
                     ? kiln_c99_object_prototype_read(store) : kiln_c99_undefined();
        if (target.tag == KILN_VALUE_UNDEFINED)
            target = kiln_c99_function_kind_prototype(closure);
        ++steps;
        goto kiln_has_next_prototype;
    }
    if (target.tag == KILN_VALUE_DATE) {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        /* The membership predicate the date read consults, so `"getTime" in d` and `d.getTime`
         * cannot disagree -- and a member the language has but this backend has not built is
         * still present, which is the question `in` asks.
         *
         * Except for the symbol keys. `kiln_c99_is_date_member` answers *every* `@@` name, which
         * is right for the read it was written for -- an unimplemented well-known symbol must
         * refuse rather than read as undefined -- and wrong here, where the question is whether
         * the property exists. A date owns one symbol-keyed member. Answering the rest true said
         * `Symbol.for("constructDateFrom") in date` was true to a bundled `date-fns`, which then
         * called the undefined it found. */
        if (kiln_c99_key_is_reserved(key)) {
            *answer = kiln_c99_same_name_key(key, "@@toPrimitive");
            return KILN_C99_OK;
        }
        *answer = kiln_c99_is_date_member(intrinsic_key, intrinsic_len) ||
                  kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                         KILN_C99_METHOD_OBJECT_VALUE_OF) != 0;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_ARRAY_BUFFER) {
        kiln_value store;
        kiln_value proto = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE);
        if (kiln_c99_callable_store(proto, &store, 0) == KILN_C99_OK &&
            kiln_c99_store_has(store, key, ikey)) {
            *answer = 1;
            return KILN_C99_OK;
        }
        *answer = kiln_c99_same_name_key(key, "byteLength") ||
                  kiln_c99_same_name_key(key, "constructor") ||
                  kiln_c99_same_name_key(key, "slice");
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_DATA_VIEW) {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        /* The same range and the same four names the read answers, listed here rather than
         * duplicated as a second table -- listing a method twice is how `in` and a read came to
         * disagree about the typed views. A DataView has no indices, so there is no index
         * question to ask first. */
        *answer = kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_DATA_VIEW_GET_INT8,
                                         KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64) != 0 ||
                  kiln_c99_same_name_key(key, "buffer") ||
                  kiln_c99_same_name_key(key, "byteLength") ||
                  kiln_c99_same_name_key(key, "byteOffset") ||
                  kiln_c99_same_name_key(key, "constructor") ||
                  kiln_c99_same_name_key(key, "isPrototypeOf") ||
                  kiln_c99_same_name_key(key, "propertyIsEnumerable") ||
                  kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                         KILN_C99_METHOD_OBJECT_VALUE_OF) != 0;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_TYPED_ARRAY) {
        kiln_typed_array *view = kiln_c99_typed_array_of(target);
        double index = kiln_c99_key_index_key(key);
        if (index >= 0.0) {
            *answer = (size_t)index < view->length;
        } else {
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
            *answer = kiln_c99_same_name_key(key, "length") ||
                      kiln_c99_same_name_key(key, "byteLength") ||
                      kiln_c99_same_name_key(key, "byteOffset") ||
                      kiln_c99_same_name_key(key, "BYTES_PER_ELEMENT") ||
                      kiln_c99_same_name_key(key, "buffer") ||
                      kiln_c99_same_name_key(key, "constructor") ||
                      kiln_c99_same_name_key(key, "isPrototypeOf") ||
                      kiln_c99_same_name_key(key, "propertyIsEnumerable") ||
                      kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                             KILN_C99_METHOD_OBJECT_VALUE_OF) != 0;
            if (!*answer) {
                kiln_value concrete_proto =
                    kiln_c99_builtin(kiln_c99_typed_prototype(view->kind));
                kiln_value store;
                if (kiln_c99_callable_store(concrete_proto, &store, 0) == KILN_C99_OK &&
                    kiln_c99_store_has(store, key, ikey)) *answer = 1;
                if (!*answer) {
                    kiln_value common_proto =
                        kiln_c99_builtin(KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE);
                    if (kiln_c99_callable_store(common_proto, &store, 0) == KILN_C99_OK &&
                        kiln_c99_store_has(store, key, ikey)) *answer = 1;
                }
            }
        }
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_REGEXP) {
        /* A regexp answers for its accessors, its own store and its prototype's methods, and
         * `in` has to agree with the read about all three. Without an arm here the walk fell
         * through to the generic chain, which does not know a regexp's prototype, so
         * `"exec" in /x/` and `"source" in /x/` were both false -- the interpreter says true, and
         * `"exec" in rx` is how a program feature-detects one. */
        static const char *const own[] = {"source",     "flags",       "lastIndex",  "constructor",
                                          "hasIndices", "global",      "ignoreCase", "multiline",
                                          "dotAll",     "unicode",     "unicodeSets", "sticky"};
        size_t index;
        kiln_value store = kiln_c99_regexp_of(target)->properties;
        if (kiln_c99_store_has(store, key, ikey)) {
            *answer = 1;
            return KILN_C99_OK;
        }
        for (index = 0; index < sizeof own / sizeof own[0]; ++index) {
            if (kiln_c99_same_name_key(key, own[index])) {
                *answer = 1;
                return KILN_C99_OK;
            }
        }
        /* The prototype's members are listed rather than read: a read *refuses* a name this
         * backend has not implemented, which is right for `rx.nope` and wrong for
         * `"nope" in rx` -- the question there is answerable and the answer is false. A regexp
         * carries no property store of its own here, so there is no third place to look. */
        {
            static const char *const inherited[] = {"exec",      "test",       "toString",
                                                    "compile",   "@@match",    "@@matchAll",
                                                    "@@replace", "@@search",   "@@split",
                                                    "@@toStringTag"};
            for (index = 0; index < sizeof inherited / sizeof inherited[0]; ++index) {
                if (kiln_c99_same_name_key(key, inherited[index])) {
                    *answer = 1;
                    return KILN_C99_OK;
                }
            }
        }
        *answer = 0;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        /* An index is present when it is in range, and `length` always is. */
        double index = kiln_c99_key_index_key(key);
        if (kiln_c99_same_name_key(key, "length")) {
            *answer = 1;
            return KILN_C99_OK;
        }
        if (index >= 0.0) {
            kiln_array *array = kiln_c99_array_of(target);
            *answer = (size_t)index < array->count && array->elements[(size_t)index] != KILN_VAL_HOLE;
            return KILN_C99_OK;
        }
        /* A name an array carries beside its elements -- `a.total`, the `index` on a match
         * result, a method of the class an array subclass was built from. The read answers these
         * out of the same store, and `for...in` filters its keys through this operator, so an
         * `in` that did not look here made an array's own named property invisible to the loop
         * that had just listed it. */
        {
            kiln_array *array = kiln_c99_array_of(target);
            if (array->properties.tag == KILN_VALUE_OBJECT) {
                int carried = 0;
                int status = kiln_c99_has_property_key(host, array->properties, key, ikey, &carried,
                                                   depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (carried) {
                    *answer = 1;
                    return KILN_C99_OK;
                }
                /* Not found there falls through to the method table rather than answering: the
                 * store's own chain ends at `Object.prototype` and would otherwise decide what
                 * `"map" in a` says, which is the array's question and not the store's. */
            }
        }
        /* `@@iterator` is not in the method table -- the read spells it out separately, because
         * `for...of` and spread reach it by different paths -- so `in` has to spell it out too
         * or `Symbol.iterator in []` answers false about a method the array demonstrably has. */
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        *answer = kiln_c99_same_name_key(key, "@@iterator") ||
                  kiln_c99_same_name_key(key, "@@unscopables") ||
                  kiln_c99_array_method(intrinsic_key, intrinsic_len) != 0 ||
                  kiln_c99_lookup_method(intrinsic_key, intrinsic_len,
                                         KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                         KILN_C99_METHOD_OBJECT_VALUE_OF) != 0;
        if (!*answer) {
            /* A member defined on `Array.prototype` itself (the bootstrap's parallel family, a
             * program's polyfill): the store the array read consults after the table. */
            kiln_value store;
            if (kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_PROTOTYPE), &store,
                                        0) == KILN_C99_OK &&
                kiln_c99_store_has(store, key, ikey)) *answer = 1;
        }
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_COLLECTION) {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        /* A collection is not an object here, so without this it fell through the chain walk
         * below and answered false about every member it demonstrably has -- `"get" in map` and
         * `Symbol.iterator in set` alike. That is not a refusal, it is a wrong answer, and vue's
         * collection handler is built on it: it installs its tracking instrumentations only for
         * a key that is `in` the raw target, so a false there silently handed the program the
         * bare `Map` method. Every reactive effect over a Map or a Set then ran once and never
         * again.
         *
         * Answered by the same predicate the read uses, so `"forEach" in m` and `m.forEach` can
         * never disagree. Entries are not properties: `"a" in new Map([["a", 1]])` is false. */
        kiln_collection *collection = kiln_c99_collection_of(target);
        if (collection->properties.tag == KILN_VALUE_OBJECT) {
            int carried = 0;
            int status = kiln_c99_has_property_key(host, collection->properties, key, ikey,
                                               &carried, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (carried) {
                *answer = 1;
                return KILN_C99_OK;
            }
        }
        *answer = kiln_c99_is_collection_member(collection, intrinsic_key, intrinsic_len);
        return KILN_C99_OK;
    }
    {
        kiln_gc_heap *heap = kiln_c99_gc_heap();
        const kiln_string *canonical = ikey != NULL ? ikey : kiln_c99_interned_key_in(heap, key);
        for (; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
            kiln_property property;
            if (current.tag != KILN_VALUE_OBJECT) {
                break;
            }
            if (kiln_c99_property_read_own_value(current, key, canonical, &property)) {
                *answer = 1;
                return KILN_C99_OK;
            }
            current = kiln_c99_object_prototype_read(current);
        }
    }
    if (current.tag == KILN_VALUE_FUNCTION || current.tag == KILN_VALUE_BUILTIN ||
        current.tag == KILN_VALUE_PROXY) {
        target = current;
        ++steps;
        goto kiln_has_next_prototype;
    }
    /* `in` has to agree with the read about what the end of the chain carries. */
    if (target.tag == KILN_VALUE_OBJECT &&
        kiln_c99_object_prototype_read(target).tag != KILN_VALUE_NULL) {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        *answer = kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                         KILN_C99_METHOD_OBJECT_VALUE_OF) != 0 ||
                  kiln_c99_same_name_key(key, "constructor");
        if (!*answer) {
            kiln_value store;
            if (kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE), &store, 0) == KILN_C99_OK &&
                kiln_c99_store_has(store, key, ikey)) *answer = 1;
        }
    }
    return KILN_C99_OK;
}

static int kiln_c99_has_property(kiln_host *host, kiln_value target, const char *key,
                                 size_t key_len, int *answer, int depth, kiln_value *thrown) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
    view.unit_len = (uint32_t)key_len;
    view.flags = kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_has_property_key(host, target, &view, NULL, answer, depth, thrown);
}

/* A typed array's own properties are exactly its live integer indices. Length, buffer metadata
 * and methods live on its intrinsic prototype even though an ordinary read reaches them. */
/* `delete target[key]`: remove an own property, answering whether the name is gone afterwards.
 *
 * A name that was never there deletes successfully, which is what makes `delete` almost always
 * true and why the answer is about the *end state* rather than about work done. */
static int kiln_c99_delete_key(kiln_host *host, kiln_value target, const kiln_string *key,
                               const kiln_string *ikey, int *answer, int depth,
                               kiln_value *thrown) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_object *object;
    *answer = 1;
    /* Native adapter state is not part of the JavaScript object. Deleting its reserved spelling
     * therefore succeeds as an ordinary absent-property delete without touching the slot. */
    if (kiln_c99_is_node_hidden_key_key(key)) {
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_PROXY) {
        kiln_value exposed = kiln_c99_key_value(key, &ikey);
        if (exposed.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        return kiln_c99_proxy_delete(host, target, exposed, answer, depth, thrown);
    }
    if (target.tag == KILN_VALUE_SHARED) {
        kiln_value ignored;
        int found = 0;
        int status = kiln_c99_shared_own_read(target, key, &ignored, &found, NULL);
        *answer = !found;
        return status;
    }
    if (target.tag == KILN_VALUE_STRING) {
        /* `delete "text".length` is false: the property is real and cannot be configured away.
         * Anything else on a string was never there, and deleting what is not there is true. */
        *answer = !kiln_c99_same_name_key(key, "length");
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        /* Deleting an element leaves a *hole*, not an undefined and not a shorter array: the
         * slot reads as undefined, `length` is unchanged, and `in` says the element is gone. */
        double slot = kiln_c99_key_index_key(key);
        kiln_array *array = kiln_c99_array_of(target);
        /* `length` is an own, non-configurable property even though it is stored on the array
         * header rather than in the named-property store. */
        if (kiln_c99_same_name_key(key, "length")) {
            *answer = 0;
            return KILN_C99_OK;
        }
        if (slot >= 0.0 && (size_t)slot < array->count &&
            array->elements[(size_t)slot] != KILN_VAL_HOLE) {
            if (array->frozen) {
                *answer = 0;
                return KILN_C99_OK;
            }
            kiln_c99_heap_store_val(array, &array->elements[(size_t)slot], KILN_VAL_HOLE);
            array->is_numeric = 0;
            return KILN_C99_OK;
        }
        /* Named and symbol properties live beside the indexed elements. Deleting one must
         * reach that ordinary store; otherwise getters that delete a later key leave it visible
         * to the fresh descriptor check performed by object spread. */
        if (slot < 0.0 && array->properties.tag == KILN_VALUE_OBJECT &&
            kiln_c99_store_has(array->properties, key, ikey)) {
            if (array->frozen) {
                *answer = 0;
                return KILN_C99_OK;
            }
            return kiln_c99_delete_key(host, array->properties, key, ikey, answer, depth, thrown);
        }
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_TYPED_ARRAY) {
        int found;
        int enumerable;
        kiln_c99_typed_own_key(target, key, &found, &enumerable);
        if (found) {
            *answer = 0;
            return KILN_C99_OK;
        }
        {
            kiln_value store = kiln_c99_typed_array_of(target)->properties;
            if (store.tag == KILN_VALUE_OBJECT && kiln_c99_store_has(store, key, ikey))
                return kiln_c99_delete_key(host, store, key, ikey, answer, depth, thrown);
        }
        *answer = 1;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_REGEXP) {
        kiln_value store = kiln_c99_regexp_of(target)->properties;
        if (kiln_c99_same_name_key(key, "lastIndex")) {
            *answer = 0;
            return KILN_C99_OK;
        }
        if (kiln_c99_store_has(store, key, ikey))
            return kiln_c99_delete_key(host, store, key, ikey, answer, depth, thrown);
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_DATE) {
        /* A Date's timestamp is its internal slot; properties created by the program live in its
         * separate ordinary store and can be deleted there like any other configurable own key. */
        kiln_value store = ((kiln_date *)(uintptr_t)target.bits)->properties;
        return store.tag == KILN_VALUE_OBJECT
                   ? kiln_c99_delete_key(host, store, key, ikey, answer, depth, thrown)
                   : KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_BUILTIN) {
        /* A member the program wrote onto a built-in lives in the built-in's store, and deleting
         * it puts the table's member back in view (`delete Float64Array.prototype.map` after a
         * patch). The store changing is a store onto the built-in, so every remembered answer
         * about its members is dropped (parallel issue 30). A table member itself is not
         * deleted: the tables are shared and have no per-realm tombstone. */
        kiln_value store = kiln_c99_undefined();
        int status = kiln_c99_callable_store(target, &store, 0);
        if (status != KILN_C99_OK) return status;
        if (store.tag == KILN_VALUE_OBJECT) {
            kiln_object *properties = kiln_c99_object_of(store);
            if (properties->shape != NULL) kiln_c99_ensure_dictionary(properties);
            if (kiln_c99_own_index_key(properties, ikey, key) != properties->count) {
                kiln_c99_realm_note_store();
                kiln_c99_realm_note_builtin_store((uint32_t)target.bits);
                return kiln_c99_delete_key(host, store, key, ikey, answer, depth, thrown);
            }
        }
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_GENERATOR) {
        kiln_value store = kiln_c99_generator_of(target)->properties;
        return store.tag == KILN_VALUE_OBJECT
            ? kiln_c99_delete_key(host, store, key, ikey, answer, depth, thrown) : KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_COMMONJS_REQUIRE) {
        kiln_value store;
        int status = kiln_c99_callable_store(target, &store, 0);
        if (status != KILN_C99_OK) return status;
        if (store.tag == KILN_VALUE_OBJECT)
            return kiln_c99_delete_key(host, store, key, ikey, answer, depth, thrown);
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_BOUND) {
        /* User-defined properties on a bound function live in its ordinary callable store.
         * Treat deletion just like deletion from the wrapped function's store; otherwise a
         * configurable own property remains visible after delete, including with a null
         * [[Prototype]]. */
        kiln_value store;
        int status = kiln_c99_callable_store(target, &store, 0);
        if (status != KILN_C99_OK) return status;
        if (store.tag == KILN_VALUE_OBJECT)
            return kiln_c99_delete_key(host, store, key, ikey, answer, depth, thrown);
        return KILN_C99_OK;
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_OK;
    }
    object = kiln_c99_object_of(target);
    if (object->elements != NULL) {
        double index_num = kiln_c99_key_index_key(key);
        if (index_num >= 0.0 && index_num < 100000.0) {
            size_t elem_idx = (size_t)index_num;
            if (object->frozen) {
                *answer = (elem_idx >= object->element_count || object->elements[elem_idx] == KILN_VAL_HOLE);
                return KILN_C99_OK;
            }
            if (elem_idx < object->element_count) {
                kiln_c99_heap_store_val(object, &object->elements[elem_idx], KILN_VAL_HOLE);
            }
            *answer = 1;
            return KILN_C99_OK;
        }
    }
    return kiln_c99_property_storage_delete(object, key, ikey, answer);
}

static int kiln_c99_delete(kiln_host *host, kiln_value target, const char *key,
                           size_t key_len, int *answer, int depth, kiln_value *thrown) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
    view.unit_len = (uint32_t)key_len;
    view.flags = kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_delete_key(host, target, &view, NULL, answer, depth, thrown);
}

static int kiln_c99_delete_or_throw(kiln_host *host, kiln_value target, const char *key,
                                    size_t key_len, int strict, int *answer, int depth,
                                    kiln_value *thrown) {
    int status = kiln_c99_delete(host, target, key, key_len, answer, depth, thrown);
    if (status != KILN_C99_OK || !strict || *answer) {
        return status;
    }
    if (target.tag == KILN_VALUE_OBJECT && kiln_c99_object_of(target)->module_namespace) {
        return kiln_c99_throw_module_namespace_delete(thrown, key, key_len);
    }
    return kiln_c99_throw_cannot_delete(thrown, key, key_len);
}

/* The headed form: the strict delete of a namespace or non-configurable property names
 * the key from units. */
static int kiln_c99_delete_or_throw_key(kiln_host *host, kiln_value target,
                                        const kiln_string *key, int strict, int *answer,
                                        int depth, kiln_value *thrown) {
    int status = kiln_c99_delete_key(host, target, key, NULL, answer, depth, thrown);
    if (status != KILN_C99_OK || !strict || *answer) {
        return status;
    }
    if (target.tag == KILN_VALUE_OBJECT && kiln_c99_object_of(target)->module_namespace) {
        return kiln_c99_throw_module_namespace_delete_key(thrown, key);
    }
    return kiln_c99_throw_cannot_delete_key(thrown, key);
}

static int kiln_c99_delete_computed_or_throw(kiln_host *host, kiln_value target, kiln_value key,
                                             int strict, int *answer, int depth,
                                             kiln_value *thrown) {
    kiln_value text = kiln_c99_property_key(key);
    int status;
    if (text.tag != KILN_VALUE_STRING) {
        KILN_C99_REFUSE("a strict delete of a key with no string spelling");
    }
    status = kiln_c99_delete_key(host, target, (const kiln_string *)text.ptr, NULL, answer, depth,
                                 thrown);
    if (status != KILN_C99_OK || !strict || *answer) {
        return status;
    }
    if (kiln_string_is_utf16((const kiln_string *)text.ptr)) {
        KILN_C99_REFUSE("a strict delete of a UTF-16 property key");
    }
    return kiln_c99_throw_cannot_delete(
        thrown, (const char *)kiln_string_latin1((const kiln_string *)text.ptr),
        ((const kiln_string *)text.ptr)->unit_len);
}

/* Append an ordinary store's enumerable string keys through the read-only projection. Dense
 * indices precede named shape keys; a mixed shape/list store keeps the dictionary conversion
 * until its integer-key merge has a read-only cursor. */
static int kiln_c99_append_enumerable_object_keys(kiln_object *object, kiln_value built,
                                                   int include_dense) {
    size_t index;
    size_t count;
    int status;
    if (object->properties == NULL && object->shape != NULL) {
        if (include_dense && object->elements != NULL) {
            for (index = 0; index < object->element_count; ++index) {
                char num_buf[32];
                size_t nlen;
                if (object->elements[index] == KILN_VAL_HOLE) continue;
                nlen = kiln_c99_double_to_js((double)index, num_buf);
                status = kiln_c99_array_push(built, kiln_c99_copy_text(num_buf, nlen));
                if (status != KILN_C99_OK) return status;
            }
        }
        count = object->shape->property_count;
    } else {
        status = kiln_c99_property_walk(object, &count);
        if (status != KILN_C99_OK) return status;
    }
    for (index = 0; index < count; ++index) {
        kiln_property projected;
        const kiln_property *property = kiln_c99_property_view(object, index, &projected);
        if (!property->enumerable) continue;
        status = kiln_c99_array_push(built, kiln_c99_stable_key_value(property->key));
        if (status != KILN_C99_OK) return status;
    }
    return KILN_C99_OK;
}

/* An array of a value's own enumerable string keys, in enumeration order: what `for...in` walks
 * and what `Object.keys` reports. A snapshot, which is what the language specifies -- a key added
 * during the loop may or may not be visited, and this chooses not to. */
static int kiln_c99_own_keys(kiln_host *host, kiln_value target, kiln_value *out, int depth,
                             kiln_value *thrown) {
    kiln_value built;
    size_t index;
    int status;
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_proxy_own_keys(host, target, out, depth, thrown);
    }
    status = kiln_c99_new_array(&built);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (target.tag == KILN_VALUE_SHARED) {
        kiln_shared_facade *facade = (kiln_shared_facade *)(uintptr_t)target.bits;
        kiln_shared_info node;
        uint64_t lease = kiln_shared_graph_retain(facade->graph, facade->generation, facade->node);
        if (lease == 0) return KILN_C99_INVALID_ARGUMENT;
        status = kiln_shared_graph_info(lease, &node) == 0 ? KILN_C99_OK : KILN_C99_INVALID_ARGUMENT;
        for (uint64_t at = 0; status == KILN_C99_OK && at < node.field_count; ++at) {
            kiln_value key;
            int enumerable;
            status = kiln_c99_shared_field_key(lease, at, &key, &enumerable);
            if (status == KILN_C99_OK && enumerable) status = kiln_c99_array_push(built, key);
        }
        (void)kiln_shared_graph_release(lease);
        if (status == KILN_C99_OK) *out = built;
        return status;
    }
    if (target.tag == KILN_VALUE_OBJECT) {
        kiln_object *object = kiln_c99_object_of(target);
        status = kiln_c99_append_enumerable_object_keys(object, built, 1);
        if (status != KILN_C99_OK) return status;
    } else if (target.tag == KILN_VALUE_BUILTIN) {
        /* A built-in constructor can have enumerable own statics even though its callable
         * properties are not held in an ordinary object store. Walk its authoritative own-key
         * list and use the same descriptor path as Object.getOwnPropertyDescriptor; this keeps
         * `Object.keys(Buffer)` and `for (const key in Buffer)` aligned with the members the
         * builtin actually exposes (notably Buffer's legacy enumerable statics). */
        kiln_value all;
        status = kiln_c99_builtin_own_keys(target.bits, &all);
        if (status != KILN_C99_OK) return status;
        for (index = 0; index < kiln_c99_array_of(all)->count; ++index) {
            kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(all)->elements[index]);
            kiln_value descriptor;
            kiln_value enumerable;
            int found = 0;
            if (key.tag != KILN_VALUE_STRING) continue; /* for-in ignores symbols */
            if (kiln_c99_same_name_key((const kiln_string *)key.ptr, "length") ||
                kiln_c99_same_name_key((const kiln_string *)key.ptr, "name") ||
                kiln_c99_same_name_key((const kiln_string *)key.ptr, "prototype")) {
                /* The callable slots are own but never enumerable, and some builtin descriptor
                 * paths intentionally leave their derived descriptors to the callable model. */
                continue;
            }
            status = kiln_c99_describe_property_key(
                host, target, (const kiln_string *)key.ptr, NULL, &descriptor, &found, depth,
                thrown);
            if (status != KILN_C99_OK) return status;
            if (!found) continue;
            status = kiln_c99_get(host, descriptor, "enumerable", 10, &enumerable, depth,
                                  thrown);
            if (status != KILN_C99_OK) return status;
            if (kiln_c99_truthy(enumerable)) {
                status = kiln_c99_array_push(built, key);
                if (status != KILN_C99_OK) return status;
            }
        }
    } else if (target.tag == KILN_VALUE_REGEXP) {
        kiln_value store = kiln_c99_regexp_of(target)->properties;
        if (store.tag == KILN_VALUE_OBJECT) {
            status = kiln_c99_append_enumerable_object_keys(kiln_c99_object_of(store), built, 1);
            if (status != KILN_C99_OK) return status;
        }
    } else if (target.tag == KILN_VALUE_ARRAY) {
        /* An array's own enumerable keys are its indices, as strings: `for (const i in [7,8])`
         * gives "0" and "1", not 0 and 1. */
        kiln_array *array = kiln_c99_array_of(target);
        for (index = 0; index < array->count; ++index) {
            if (array->elements[index] == KILN_VAL_HOLE) {
                continue;
            }
            status = kiln_c99_array_push(built, kiln_c99_text(kiln_c99_number((double)index)));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        if (array->properties.tag == KILN_VALUE_OBJECT) {
            kiln_object *extra = kiln_c99_object_of(array->properties);
            status = kiln_c99_append_enumerable_object_keys(extra, built, 0);
            if (status != KILN_C99_OK) return status;
        }
    } else if (target.tag == KILN_VALUE_TYPED_ARRAY) {
        kiln_typed_array *view = kiln_c99_typed_array_of(target);
        for (index = 0; index < view->length; ++index) {
            status = kiln_c99_array_push(built, kiln_c99_text(kiln_c99_number((double)index)));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        if (view->properties.tag == KILN_VALUE_OBJECT) {
            status = kiln_c99_append_enumerable_object_keys(
                kiln_c99_object_of(view->properties), built, 0);
            if (status != KILN_C99_OK) return status;
        }
    } else if (target.tag == KILN_VALUE_COMMONJS_REQUIRE) {
        kiln_value store;
        status = kiln_c99_callable_store(target, &store, 0);
        if (status != KILN_C99_OK) return status;
        if (store.tag == KILN_VALUE_OBJECT) {
            status = kiln_c99_append_enumerable_object_keys(kiln_c99_object_of(store), built, 0);
            if (status != KILN_C99_OK) return status;
        }
    }
    *out = built;
    return KILN_C99_OK;
}

/* Whether a list of key strings already holds this name. Linear, because the lists it searches
 * are one object's properties and the cost of a hash is the hash. */
static int kiln_c99_keys_contain(kiln_value keys, const kiln_string *key) {
    kiln_array *list = kiln_c99_array_of(keys);
    size_t index;
    for (index = 0; index < list->count; ++index) {
        kiln_value name = kiln_val_to_boundary(list->elements[index]);
        if (name.tag == KILN_VALUE_STRING &&
            kiln_string_equal((const kiln_string *)name.ptr, key)) {
            return 1;
        }
    }
    return 0;
}

/* What `for...in` walks: every *reachable* enumerable string key, own ones first and then the
 * prototype chain's.
 *
 * This is the one enumeration in the language that is not about own properties, which is why it
 * is a different function from `kiln_c99_own_keys` rather than a flag on it -- `Object.keys` and
 * `JSON.stringify` must not follow the chain, and a shared implementation would eventually let
 * one of them.
 *
 * Shadowing is by *name*, not by enumerability: a name already seen at a nearer link is skipped
 * further out even when the nearer one is non-enumerable. That is what makes
 * `Object.defineProperty(child, "k", { enumerable: false })` hide an enumerable `k` on the
 * parent rather than merely losing its own turn, and it is the case a naive union gets wrong. */
static int kiln_c99_enumerate_keys(kiln_host *host, kiln_value target, kiln_value *out,
                                   int depth, kiln_value *thrown) {
    kiln_value built;
    kiln_value shadowed;
    kiln_value current;
    size_t steps;
    size_t index;
    int status = kiln_c99_own_keys(host, target, &built, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        /* Only an ordinary object carries a chain a program can put enumerable names on. An
         * array's, a view's and a string's ends at a built-in prototype whose every member is
         * non-enumerable, so walking it could only ever add nothing. */
        *out = built;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_array(&shadowed);
    if (status != KILN_C99_OK) {
        return status;
    }
    {
        kiln_object *object = kiln_c99_object_of(target);
        if (object->properties == NULL && object->shape != NULL) {
            if (object->elements != NULL) {
                for (index = 0; index < object->element_count; ++index) {
                    if (object->elements[index] == KILN_VAL_HOLE) continue;
                    char num_buf[32];
                    size_t nlen = kiln_c99_double_to_js((double)index, num_buf);
                    status = kiln_c99_array_push(shadowed, kiln_c99_copy_text(num_buf, nlen));
                    if (status != KILN_C99_OK) return status;
                }
            }
            for (index = 0; index < object->shape->property_count; ++index) {
                const kiln_string *skey = object->shape->keys[index];
                status = kiln_c99_array_push(shadowed, kiln_c99_stable_key_value(skey));
                if (status != KILN_C99_OK) return status;
            }
        } else {
            size_t count;
            status = kiln_c99_property_walk(object, &count);
            if (status != KILN_C99_OK) return status;
            for (index = 0; index < count; ++index) {
                kiln_property projected;
                const kiln_string *key = kiln_c99_property_view(object, index, &projected)->key;
                status = kiln_c99_array_push(
                    shadowed, kiln_c99_stable_key_value(key));
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
        }
        current = object->prototype;
    }
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        kiln_object *object;
        if (current.tag != KILN_VALUE_OBJECT) {
            break;
        }
        object = kiln_c99_object_of(current);
        if (object->properties == NULL && object->shape != NULL) {
            if (object->elements != NULL) {
                for (index = 0; index < object->element_count; ++index) {
                    if (object->elements[index] == KILN_VAL_HOLE) continue;
                    char num_buf[32];
                    size_t key_len = kiln_c99_double_to_js((double)index, num_buf);
                    kiln_value num = kiln_c99_copy_text(num_buf, key_len);
                    const kiln_string *num_key = (const kiln_string *)num.ptr;
                    if (kiln_c99_keys_contain(shadowed, num_key)) continue;
                    status = kiln_c99_array_push(shadowed, num);
                    if (status != KILN_C99_OK) return status;
                    status = kiln_c99_array_push(built, num);
                    if (status != KILN_C99_OK) return status;
                }
            }
            for (index = 0; index < object->shape->property_count; ++index) {
                const kiln_string *key = object->shape->keys[index];
                int enumerable = kiln_c99_shape_slot_enumerable(object->shape, (uint32_t)index);
                if (kiln_c99_key_is_reserved(key) && !enumerable) continue;
                if (kiln_c99_keys_contain(shadowed, key)) continue;
                kiln_value key_value = kiln_c99_stable_key_value(key);
                status = kiln_c99_array_push(shadowed, key_value);
                if (status != KILN_C99_OK) return status;
                /* A method a class defined is a name that shadows, and is not enumerated. */
                if (!enumerable) continue;
                status = kiln_c99_array_push(built, key_value);
                if (status != KILN_C99_OK) return status;
            }
        } else {
            size_t count;
            status = kiln_c99_property_walk(object, &count);
            if (status != KILN_C99_OK) return status;
            for (index = 0; index < count; ++index) {
                kiln_property projected;
                const kiln_property *property = kiln_c99_property_view(object, index, &projected);
                const kiln_string *key = property->key;
                int enumerable = property->enumerable;
                if (kiln_c99_keys_contain(shadowed, key)) {
                    continue;
                }
                kiln_value key_value = kiln_c99_stable_key_value(key);
                status = kiln_c99_array_push(shadowed, key_value);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (!enumerable) {
                    continue;
                }
                status = kiln_c99_array_push(built, key_value);
                if (status != KILN_C99_OK) {
                    return status;
                }
                /* A push may have moved the list this loop is reading, so the object is found again
                 * rather than kept across the call. */
                object = kiln_c99_object_of(current);
            }
        }
        current = object->prototype;
    }
    *out = built;
    return KILN_C99_OK;
}

/* `{ ...proxy }`: the three traps, in the order `CopyDataProperties` runs them.
 *
 * A proxy has no property store to walk, so asking it what it holds means asking the handler:
 * `ownKeys` for the names, `getOwnPropertyDescriptor` for which of them are enumerable, and `get`
 * for each value. All three are observable -- a handler that counts its calls can tell -- so none
 * of them is short-circuited to the target. The value comes from the *read*, not from the
 * descriptor, because a proxy is free to answer a read with something its descriptor never
 * mentioned and a spread copies what a read produces.
 *
 * Symbols are kept, unlike `kiln_c99_proxy_own_keys`, which drops them because `Object.keys` and
 * `for...in` do. A spread does not: it is the one enumeration that carries a symbol key across. */
static int kiln_c99_spread_proxy(kiln_host *host, kiln_value target, kiln_value source, int depth,
                                 kiln_value *thrown) {
    kiln_value listed;
    size_t index;
    int status = kiln_c99_proxy_all_keys(host, source, &listed, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* The list is found again each time round, because a descriptor or a `get` trap is a program
     * function and may have grown the array the handler handed back. */
    for (index = 0; index < kiln_c99_array_of(listed)->count; ++index) {
        kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(listed)->elements[index]);
        kiln_value descriptor;
        kiln_value enumerable;
        kiln_value copied;
        int found;
        status = kiln_c99_proxy_describe(host, source, key, &descriptor, &found, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (!found) {
            continue;
        }
        status = kiln_c99_get(host, descriptor, "enumerable", 10, &enumerable, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (!kiln_c99_truthy(enumerable)) {
            continue;
        }
        status = kiln_c99_get_computed(host, source, key, &copied, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        /* CopyDataProperties uses CreateDataProperty on its fresh target. Preserve the original
         * string or symbol key instead of assigning through [[Set]], which could invoke an
         * inherited setter on Object.prototype. */
        status = kiln_c99_create_data_property(host, target, key, copied, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    return KILN_C99_OK;
}

/* `{ ...source }`: copy every own enumerable property of the source into the target.
 *
 * A spread copies *values*, which for an accessor means calling its getter and storing what it
 * produced -- the target gets a plain data property, not the accessor. Spreading nullish
 * contributes nothing rather than throwing, which is the one place object spread is more
 * forgiving than array spread.
 *
 * The source's kind decides only *where* its properties are kept, never which ones are copied.
 * A function and a bound function keep their store on the value rather than in an object, which
 * is the whole reason `{ ...postcss }` copies anything at all when `postcss` is a function
 * carrying its plugins; an array and a string contribute their indices and then whatever was
 * hung on them by name. */
