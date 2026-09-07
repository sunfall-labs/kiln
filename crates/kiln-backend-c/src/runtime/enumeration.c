
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
            status = kiln_c99_array_push(target, kiln_c99_number(kiln_c99_typed_get(view, index)));
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
        status = kiln_c99_get(host, source, "@@iterator", 10, &method, depth, thrown);
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

/* `key in target`: whether the name is reachable, own or inherited. Unlike a read this does not
 * call an accessor -- the question is whether the property exists, not what it produces. */
static int kiln_c99_has_property(kiln_host *host, kiln_value target, const char *key,
                                 size_t key_len, int *answer, int depth, kiln_value *thrown) {
    kiln_value current = target;
    size_t steps;
    *answer = 0;
    if (kiln_c99_is_node_hidden_key(key, key_len)) {
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_has_computed(host, target,
                                     kiln_c99_value(KILN_VALUE_STRING, 0, key, key_len), answer,
                                     depth, thrown);
    }
    if (target.tag == KILN_VALUE_ARRAY_BUFFER) {
        *answer = kiln_c99_same_name(key, key_len, "byteLength") ||
                  kiln_c99_same_name(key, key_len, "constructor");
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_DATA_VIEW) {
        /* The same range and the same four names the read answers, listed here rather than
         * duplicated as a second table -- listing a method twice is how `in` and a read came to
         * disagree about the typed views. A DataView has no indices, so there is no index
         * question to ask first. */
        *answer = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_DATA_VIEW_GET_INT8,
                                         KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64) != 0 ||
                  kiln_c99_same_name(key, key_len, "buffer") ||
                  kiln_c99_same_name(key, key_len, "byteLength") ||
                  kiln_c99_same_name(key, key_len, "byteOffset") ||
                  kiln_c99_same_name(key, key_len, "constructor") ||
                  kiln_c99_same_name(key, key_len, "isPrototypeOf") ||
                  kiln_c99_same_name(key, key_len, "propertyIsEnumerable") ||
                  kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                         KILN_C99_METHOD_OBJECT_VALUE_OF) != 0;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_TYPED_ARRAY) {
        kiln_typed_array *view = kiln_c99_typed_array_of(target);
        double index = kiln_c99_key_index(key, key_len);
        if (index >= 0.0) {
            *answer = (size_t)index < view->length;
        } else {
            /* The same table the read uses, so `"map" in view` and `view.map` can never
             * disagree -- listing the methods a second time here is how they came to. */
            *answer = kiln_c99_typed_prototype_method(key, key_len) != 0 ||
                      kiln_c99_same_name(key, key_len, "length") ||
                      kiln_c99_same_name(key, key_len, "byteLength") ||
                      kiln_c99_same_name(key, key_len, "byteOffset") ||
                      kiln_c99_same_name(key, key_len, "BYTES_PER_ELEMENT") ||
                      kiln_c99_same_name(key, key_len, "buffer") ||
                      kiln_c99_same_name(key, key_len, "constructor") ||
                      kiln_c99_same_name(key, key_len, "@@iterator") ||
                      kiln_c99_same_name(key, key_len, "isPrototypeOf") ||
                      kiln_c99_same_name(key, key_len, "propertyIsEnumerable") ||
                      kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                             KILN_C99_METHOD_OBJECT_VALUE_OF) != 0;
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
        for (index = 0; index < sizeof own / sizeof own[0]; ++index) {
            if (kiln_c99_same_name(key, key_len, own[index])) {
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
                                                    "@@replace", "@@search",   "@@split"};
            for (index = 0; index < sizeof inherited / sizeof inherited[0]; ++index) {
                if (kiln_c99_same_name(key, key_len, inherited[index])) {
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
        double index = kiln_c99_key_index(key, key_len);
        if (kiln_c99_same_name(key, key_len, "length")) {
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
                int status = kiln_c99_has_property(host, array->properties, key, key_len, &carried,
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
        *answer = kiln_c99_same_name(key, key_len, "@@iterator") ||
                  kiln_c99_array_method(key, key_len) != 0;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_COLLECTION) {
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
            int status = kiln_c99_has_property(host, collection->properties, key, key_len,
                                               &carried, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (carried) {
                *answer = 1;
                return KILN_C99_OK;
            }
        }
        *answer = kiln_c99_is_collection_member(collection, key, key_len);
        return KILN_C99_OK;
    }
    {
        kiln_gc_heap *heap = kiln_c99_gc_heap();
        const char *ikey = kiln_c99_interned_in(heap, key, key_len);
        for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
            kiln_object *object;
            if (current.tag != KILN_VALUE_OBJECT) {
                break;
            }
            object = kiln_c99_object_of(current);
            if (object->elements != NULL) {
                double index_num = kiln_c99_key_index(key, key_len);
                if (index_num >= 0.0 && index_num < 100000.0) {
                    size_t elem_idx = (size_t)index_num;
                    if (elem_idx < object->element_count &&
                        object->elements[elem_idx] != KILN_VAL_HOLE) {
                        *answer = 1;
                        return KILN_C99_OK;
                    }
                }
            }
            if (object->shape != NULL && ikey != NULL) {
                uint32_t slot;
                if (kiln_c99_shape_find_slot(object->shape, ikey, &slot)) {
                    *answer = 1;
                    return KILN_C99_OK;
                }
            }
            if (object->properties != NULL || object->shape == NULL) {
                if (kiln_c99_own_index(object, ikey, key, key_len) != object->count) {
                    *answer = 1;
                    return KILN_C99_OK;
                }
                if (kiln_c99_node_has_lazy_header(current, key, key_len)) {
                    *answer = 1;
                    return KILN_C99_OK;
                }
            }
            current = object->prototype;
        }
    }
    /* `in` has to agree with the read about what the end of the chain carries. */
    if (target.tag == KILN_VALUE_OBJECT &&
        kiln_c99_object_of(target)->prototype.tag != KILN_VALUE_NULL) {
        *answer = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                         KILN_C99_METHOD_OBJECT_VALUE_OF) != 0 ||
                  kiln_c99_same_name(key, key_len, "constructor");
    }
    return KILN_C99_OK;
}

/* A typed array's own properties are exactly its live integer indices. Length, buffer metadata
 * and methods live on its intrinsic prototype even though an ordinary read reaches them. */
static void kiln_c99_typed_own(kiln_value target, const char *key, size_t key_len,
                               int *found, int *enumerable) {
    double index = kiln_c99_key_index(key, key_len);
    kiln_typed_array *view = kiln_c99_typed_array_of(target);
    *found = index >= 0.0 && (size_t)index < view->length;
    *enumerable = *found;
}

/* Concrete typed-array prototypes own their constructor metadata; methods live one link above on
 * the common typed-array prototype. Prototype properties are non-enumerable. */
static int kiln_c99_typed_prototype_own(kiln_value target, const char *key, size_t key_len,
                                        int *found, int *enumerable) {
    kiln_typed_kind typed_kind;
    int concrete = target.tag == KILN_VALUE_BUILTIN &&
                   kiln_c99_typed_prototype_kind(target.bits, &typed_kind);
    int common = target.tag == KILN_VALUE_BUILTIN &&
                 target.bits == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE;
    (void)typed_kind;
    *found = 0;
    *enumerable = 0;
    if (!concrete && !common) return 0;
    if (concrete) {
        *found = kiln_c99_same_name(key, key_len, "constructor") ||
                 kiln_c99_same_name(key, key_len, "BYTES_PER_ELEMENT");
    } else {
        *found = kiln_c99_typed_prototype_method(key, key_len) != 0;
    }
    return 1;
}

/* `delete target[key]`: remove an own property, answering whether the name is gone afterwards.
 *
 * A name that was never there deletes successfully, which is what makes `delete` almost always
 * true and why the answer is about the *end state* rather than about work done. */
static int kiln_c99_delete(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                           int *answer, int depth, kiln_value *thrown) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_object *object;
    size_t index;
    *answer = 1;
    /* Native adapter state is not part of the JavaScript object. Deleting its reserved spelling
     * therefore succeeds as an ordinary absent-property delete without touching the slot. */
    if (kiln_c99_is_node_hidden_key(key, key_len)) {
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_delete_computed(host, target,
                                        kiln_c99_value(KILN_VALUE_STRING, 0, key, key_len),
                                        answer, depth, thrown);
    }
    if (target.tag == KILN_VALUE_STRING) {
        /* `delete "text".length` is false: the property is real and cannot be configured away.
         * Anything else on a string was never there, and deleting what is not there is true. */
        *answer = !kiln_c99_same_name(key, key_len, "length");
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        /* Deleting an element leaves a *hole*, not an undefined and not a shorter array: the
         * slot reads as undefined, `length` is unchanged, and `in` says the element is gone. */
        double slot = kiln_c99_key_index(key, key_len);
        kiln_array *array = kiln_c99_array_of(target);
        if (array->frozen) {
            *answer = 0;
            return KILN_C99_OK;
        }
        if (slot >= 0.0 && (size_t)slot < array->count) {
            array->elements[(size_t)slot] = KILN_VAL_HOLE;
            array->is_numeric = 0;
        }
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_TYPED_ARRAY) {
        int found;
        int enumerable;
        kiln_c99_typed_own(target, key, key_len, &found, &enumerable);
        *answer = !found;
        return KILN_C99_OK;
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_OK;
    }
    object = kiln_c99_object_of(target);
    if (object->elements != NULL) {
        double index_num = kiln_c99_key_index(key, key_len);
        if (index_num >= 0.0 && index_num < 100000.0) {
            size_t elem_idx = (size_t)index_num;
            if (object->frozen) {
                *answer = (elem_idx >= object->element_count || object->elements[elem_idx] == KILN_VAL_HOLE);
                return KILN_C99_OK;
            }
            if (elem_idx < object->element_count) {
                object->elements[elem_idx] = KILN_VAL_HOLE;
            }
            *answer = 1;
            return KILN_C99_OK;
        }
    }
    if (object->shape != NULL) {
        kiln_c99_ensure_dictionary(object);
        object->shape = NULL;
    }
    if (object->frozen) {
        *answer = 0;
        return KILN_C99_OK;
    }
    {
        kiln_gc_heap *heap = kiln_c99_gc_heap();
        index = kiln_c99_own_index(object, kiln_c99_interned_in(heap, key, key_len), key,
                                   key_len);
    }
    if (index != object->count) {
        if (!object->properties[index].configurable) {
            /* `delete` answers about the end state, and the property is still there. */
            *answer = 0;
            return KILN_C99_OK;
        }
        if (object->properties[index].accessor) {
            object->accessor_count -= 1;
        }
        /* Shifted rather than swapped with the last: property order is observable, and a
         * `delete` must not reorder what remains. */
        for (; index + 1 < object->count; ++index) {
            object->properties[index] = object->properties[index + 1];
        }
        object->count -= 1;
        return KILN_C99_OK;
    }
    return KILN_C99_OK;
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

static int kiln_c99_delete_computed_or_throw(kiln_host *host, kiln_value target, kiln_value key,
                                             int strict, int *answer, int depth,
                                             kiln_value *thrown) {
    int status = kiln_c99_delete_computed(host, target, key, answer, depth, thrown);
    kiln_value text;
    if (status != KILN_C99_OK || !strict || *answer) {
        return status;
    }
    text = kiln_c99_property_key(key);
    if (text.tag != KILN_VALUE_STRING) {
        KILN_C99_REFUSE("a strict delete of a key with no string spelling");
    }
    return kiln_c99_throw_cannot_delete(thrown, text.ptr, text.len);
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
    if (target.tag == KILN_VALUE_OBJECT) {
        kiln_c99_node_materialize_all_headers(target);
        kiln_object *object = kiln_c99_object_of(target);
        if (object->properties == NULL && object->shape != NULL) {
            if (object->elements != NULL) {
                for (index = 0; index < object->element_count; ++index) {
                    if (object->elements[index] == KILN_VAL_HOLE) continue;
                    char num_buf[32];
                    int nlen = kiln_number_to_js_string((double)index, num_buf, sizeof num_buf);
                    status = kiln_c99_array_push(built, kiln_c99_copy_text(num_buf, (size_t)nlen));
                    if (status != KILN_C99_OK) return status;
                }
            }
            for (index = 0; index < object->shape->property_count; ++index) {
                const char *skey = object->shape->keys[index];
                size_t key_len = strlen(skey);
                if (key_len >= 2 && skey[0] == '@' && skey[1] == '@') {
                    continue;
                }
                status = kiln_c99_array_push(built, kiln_c99_copy_text(skey, key_len));
                if (status != KILN_C99_OK) return status;
            }
        } else {
            if (object->shape != NULL) {
                kiln_c99_ensure_dictionary(object);
            }
            for (index = 0; index < object->count; ++index) {
                if (!object->properties[index].enumerable) {
                    continue;
                }
                status = kiln_c99_array_push(built, kiln_c99_copy_text(object->properties[index].key,
                                                                       object->properties[index].key_len));
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
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
            if (extra->properties == NULL && extra->shape != NULL) {
                for (index = 0; index < extra->shape->property_count; ++index) {
                    const char *skey = extra->shape->keys[index];
                    size_t key_len = strlen(skey);
                    if (key_len >= 2 && skey[0] == '@' && skey[1] == '@') continue;
                    status = kiln_c99_array_push(built, kiln_c99_copy_text(skey, key_len));
                    if (status != KILN_C99_OK) return status;
                }
            } else {
                if (extra->shape != NULL) {
                    kiln_c99_ensure_dictionary(extra);
                }
                for (index = 0; index < extra->count; ++index) {
                    if (!extra->properties[index].enumerable) {
                        continue;
                    }
                    status = kiln_c99_array_push(
                        built, kiln_c99_copy_text(extra->properties[index].key,
                                                   extra->properties[index].key_len));
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
            }
        }
    } else if (target.tag == KILN_VALUE_TYPED_ARRAY) {
        kiln_typed_array *view = kiln_c99_typed_array_of(target);
        for (index = 0; index < view->length; ++index) {
            status = kiln_c99_array_push(built, kiln_c99_text(kiln_c99_number((double)index)));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    *out = built;
    return KILN_C99_OK;
}

/* Whether a list of key strings already holds this name. Linear, because the lists it searches
 * are one object's properties and the cost of a hash is the hash. */
static int kiln_c99_keys_contain(kiln_value keys, const char *key, size_t key_len) {
    kiln_array *list = kiln_c99_array_of(keys);
    size_t index;
    for (index = 0; index < list->count; ++index) {
        kiln_value name = kiln_val_to_boundary(list->elements[index]);
        if (name.tag == KILN_VALUE_STRING && name.len == key_len &&
            (key_len == 0 || memcmp(name.ptr, key, key_len) == 0)) {
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
                    int nlen = kiln_number_to_js_string((double)index, num_buf, sizeof num_buf);
                    status = kiln_c99_array_push(shadowed, kiln_c99_copy_text(num_buf, (size_t)nlen));
                    if (status != KILN_C99_OK) return status;
                }
            }
            for (index = 0; index < object->shape->property_count; ++index) {
                const char *skey = object->shape->keys[index];
                size_t key_len = strlen(skey);
                status = kiln_c99_array_push(shadowed, kiln_c99_copy_text(skey, key_len));
                if (status != KILN_C99_OK) return status;
            }
        } else {
            if (object->shape != NULL) {
                kiln_c99_ensure_dictionary(object);
            }
            for (index = 0; index < object->count; ++index) {
                status = kiln_c99_array_push(shadowed,
                                             kiln_c99_copy_text(object->properties[index].key,
                                                                object->properties[index].key_len));
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
                    int nlen = kiln_number_to_js_string((double)index, num_buf, sizeof num_buf);
                    size_t key_len = (size_t)nlen;
                    if (kiln_c99_keys_contain(shadowed, num_buf, key_len)) continue;
                    status = kiln_c99_array_push(shadowed, kiln_c99_copy_text(num_buf, key_len));
                    if (status != KILN_C99_OK) return status;
                    status = kiln_c99_array_push(built, kiln_c99_copy_text(num_buf, key_len));
                    if (status != KILN_C99_OK) return status;
                }
            }
            for (index = 0; index < object->shape->property_count; ++index) {
                const char *key = object->shape->keys[index];
                size_t key_len = strlen(key);
                if (key_len >= 2 && key[0] == '@' && key[1] == '@') continue;
                if (kiln_c99_keys_contain(shadowed, key, key_len)) continue;
                status = kiln_c99_array_push(shadowed, kiln_c99_copy_text(key, key_len));
                if (status != KILN_C99_OK) return status;
                status = kiln_c99_array_push(built, kiln_c99_copy_text(key, key_len));
                if (status != KILN_C99_OK) return status;
            }
        } else {
            if (object->shape != NULL) {
                kiln_c99_ensure_dictionary(object);
            }
            for (index = 0; index < object->count; ++index) {
                const char *key = object->properties[index].key;
                size_t key_len = object->properties[index].key_len;
                int enumerable = object->properties[index].enumerable;
                if (kiln_c99_keys_contain(shadowed, key, key_len)) {
                    continue;
                }
                status = kiln_c99_array_push(shadowed, kiln_c99_copy_text(key, key_len));
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (!enumerable) {
                    continue;
                }
                status = kiln_c99_array_push(built, kiln_c99_copy_text(key, key_len));
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
        kiln_value name;
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
        /* A symbol is stored under the reserved name it carries, the same spelling every other
         * write of one uses -- and that name lives on the symbol, so it outlives this frame. */
        name = kiln_c99_property_key(key);
        if (name.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        status = kiln_c99_put(host, target, name.ptr, name.len, copied, depth, thrown);
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
