
/* The computed forms, which exist so a Symbol reaches the trap as itself rather than as the
 * reserved slot name every other path reduces it to. */
static int kiln_c99_has_computed(kiln_host *host, kiln_value target, kiln_value key, int *answer,
                                 int depth, kiln_value *thrown) {
    kiln_value name;
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_proxy_has(host, target, key, answer, depth, thrown);
    }
    name = kiln_c99_property_key(key);
    if (name.tag != KILN_VALUE_STRING) {
        KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol slot");
    }
    return kiln_c99_has_key(host, target, (const kiln_string *)name.ptr, NULL, answer, depth,
                            thrown);
}

static int kiln_c99_delete_computed(kiln_host *host, kiln_value target, kiln_value key,
                                    int *answer, int depth, kiln_value *thrown) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_value name;
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_proxy_delete(host, target, key, answer, depth, thrown);
    }
    name = kiln_c99_property_key(key);
    if (name.tag != KILN_VALUE_STRING) {
        KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol slot");
    }
    return kiln_c99_delete_key(host, target, (const kiln_string *)name.ptr, NULL, answer, depth,
                               thrown);
}

/* The keys a program (or the shared TypeScript bootstrap) defined on a built-in, held in its
 * own-property store, appended to `built` after the ones the table answers -- skipping any it
 * already lists. Both branches of `kiln_c99_builtin_own_keys` end here: a table-driven prototype
 * such as `Array.prototype` carries bootstrap members too (the parallel family, ADR 0069), and a
 * list without them made `hasOwnProperty` refuse a member a read answers. */
static int kiln_c99_builtin_store_keys(uint64_t builtin, kiln_value built) {
    kiln_value store;
    kiln_object *obj;
    size_t p;
    size_t count;
    int status;
    if (kiln_c99_callable_store(kiln_c99_builtin(builtin), &store, 0) != KILN_C99_OK ||
        store.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_OK;
    }
    obj = kiln_c99_object_of(store);
    status = kiln_c99_property_walk(obj, &count);
    if (status != KILN_C99_OK) return status;
    for (p = 0; p < count; ++p) {
        kiln_property projected;
        const kiln_property *property = kiln_c99_property_view(obj, p, &projected);
        size_t b_idx;
        int duplicate = 0;
        kiln_value key;
        kiln_array *arr = kiln_c99_array_of(built);
        if (kiln_c99_key_is_reserved(property->key)) {
            if (!kiln_c99_symbol_of_key_view(property->key, &key)) continue;
        } else {
            key = kiln_c99_stable_key_value(property->key);
        }
        for (b_idx = 0; b_idx < arr->count; ++b_idx) {
            if (kiln_c99_strict_equal(kiln_val_to_boundary(arr->elements[b_idx]), key)) {
                duplicate = 1;
                break;
            }
        }
        if (!duplicate) {
            status = kiln_c99_array_push(built, key);
            if (status != KILN_C99_OK) return status;
        }
    }
    return KILN_C99_OK;
}

/* Every name a built-in namespace or prototype table answers to.
 *
 * Derived by round-tripping the code space through the same lookup a read uses, rather than
 * listed a second time: a method added to the table shows up here the day it is added, and one
 * that is not in the table cannot be reported. This is not the list a real engine would give --
 * these prototypes are method tables and grow a method at a time -- but everything in it is a
 * member the program can actually reach, which is the property a caller walking descriptors
 * depends on. */
static int kiln_c99_builtin_own_keys(uint64_t builtin, kiln_value *out) {
    kiln_value built;
    uint64_t code;
    int status;
    if (kiln_c99_has_surface(builtin)) {
        /* A table-driven built-in can describe itself, which is the one thing the hand-written
         * chains could never do: the names it answers to *are* its rows. Refusing here is what
         * kept the native side from enumerating its own surface, so the two engines could only be
         * compared one member at a time -- by whichever bundle happened to read one. */
        size_t index;
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < KILN_C99_SURFACE_MEMBER_COUNT && status == KILN_C99_OK; ++index) {
            const kiln_builtin_member *member = &kiln_c99_builtin_members[index];
            size_t len;
            if (member->builtin != builtin || kiln_c99_builtin_row_withheld(index)) {
                continue;
            }
            len = strlen(member->name);
            if (len >= 2 && member->name[0] == '@' && member->name[1] == '@') {
                /* Reported as the symbol it is, which is what `Reflect.ownKeys` wants;
                 * `getOwnPropertyNames` drops it again, and the two exist to differ. */
                kiln_value symbol;
                if (!kiln_c99_symbol_of_key(member->name, len, &symbol)) {
                    continue;
                }
                status = kiln_c99_array_push(built, symbol);
                continue;
            }
            status = kiln_c99_array_push(built, kiln_c99_copy_text(member->name, len));
        }
        if (status == KILN_C99_OK && !kiln_c99_is_namespace(builtin)) {
            /* A constructor is a function, and these two are answered by the read path rather
             * than held as slots. `length` is deliberately not listed: reading it refuses, and a
             * name in this list that a read cannot answer is the same lie in the other
             * direction. */
            kiln_value prototype;
            status = kiln_c99_array_push(built, kiln_c99_copy_text("name", 4));
            if (status == KILN_C99_OK &&
                kiln_c99_get(NULL, kiln_c99_builtin(builtin), "prototype", 9, &prototype, 0,
                             NULL) == KILN_C99_OK &&
                prototype.tag == KILN_VALUE_BUILTIN) {
                status = kiln_c99_array_push(built, kiln_c99_copy_text("prototype", 9));
            }
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_builtin_store_keys(builtin, built);
        }
        *out = built;
        return status;
    }
    if (!kiln_c99_is_namespace(builtin)) {
        /* A constructor whose statics are not table-driven -- the typed-array constructors and
         * `Temporal`'s eight types -- still cannot list itself, because its members are computed
         * from the kind rather than declared. A short list a caller could not tell from a
         * complete one is worse than a refusal, so it stays a refusal and lands in the drift
         * test's backlog by name. */
        KILN_C99_REFUSE("own keys of a built-in whose statics are computed from its kind rather "
                        "than declared");
    }
    status = kiln_c99_new_array(&built);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* Every code the unit declares, in numeric order. This stopped at 256 once, which is where
     * the prototype blocks ended when it was written; the append-only tail (ADR 0043) has
     * carried array members since -- `toSorted`, `toReversed`, `toSpliced`, `with`,
     * `parallelMap` -- and each was a member a read answered, `in` confirmed, and this list
     * omitted, so `hasOwnProperty` refused it by name. */
    for (code = 1; code < kiln_c99_method_code_end(); ++code) {
        const char *name = kiln_c99_method_name(code);
        size_t len;
        if (code >= KILN_C99_METHOD_DATA_VIEW_GET_INT8 && code <= KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64) {
            /* Listed by the block below, unconditionally: these are accessors the namespace
             * lookup does not answer as methods. */
            continue;
        }
        if (name == NULL || name[0] == '@') {
            /* A `@@` name is a reserved slot reached through a Symbol, not a string key. */
            continue;
        }
        len = strlen(name);
        if (kiln_c99_namespace_method(builtin, name, len) != code) {
            continue;
        }
        status = kiln_c99_array_push(built, kiln_c99_copy_text(name, len));
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    /* `DataView.prototype`'s accessors are numbered above the loop's bound, which stops where
     * every other prototype table's range does. Walking this one table's own range as well is
     * cheaper than raising a bound each of the others would then pay -- and a prototype that
     * cannot list its own members is exactly the gap ADR 0043 exists to close. */
    if (builtin == KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE) {
        for (code = KILN_C99_METHOD_DATA_VIEW_GET_INT8;
             code <= KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64; ++code) {
            const char *accessor = kiln_c99_method_name(code);
            if (accessor == NULL) {
                continue;
            }
            status = kiln_c99_array_push(built, kiln_c99_copy_text(accessor, strlen(accessor)));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    /* A prototype table owns the link back to its constructor, which the read path answers and
     * the method table does not hold. */
    {
        kiln_value owner;
        if (kiln_c99_get(NULL, kiln_c99_builtin(builtin), "constructor", 11, &owner, 0, NULL) ==
                KILN_C99_OK &&
            owner.tag == KILN_VALUE_BUILTIN) {
            status = kiln_c99_array_push(built, kiln_c99_copy_text("constructor", 11));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    if (builtin == KILN_C99_BUILTIN_ARRAY_PROTOTYPE) {
        status = kiln_c99_array_push(built, kiln_c99_copy_text("length", 6));
        if (status != KILN_C99_OK) {
            return status;
        }
        kiln_value symbol;
        if (kiln_c99_symbol_of_key("@@iterator", 10, &symbol)) {
            status = kiln_c99_array_push(built, symbol);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        if (kiln_c99_symbol_of_key("@@unscopables", 13, &symbol)) {
            status = kiln_c99_array_push(built, symbol);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    if (builtin == KILN_C99_BUILTIN_REGEXP_PROTOTYPE) {
        kiln_value symbol;
        if (kiln_c99_symbol_of_key("@@toStringTag", 13, &symbol)) {
            status = kiln_c99_array_push(built, symbol);
            if (status != KILN_C99_OK) return status;
        }
    }
    status = kiln_c99_builtin_store_keys(builtin, built);
    if (status != KILN_C99_OK) return status;
    *out = built;
    return KILN_C99_OK;
}

/* Every own string key, enumerable or not -- which is the whole difference from `Object.keys`,
 * and why a function's assigned properties show up here. A proxy answers through its `ownKeys`
 * trap, which is the one operation that reports the unfiltered list. */
static int kiln_c99_all_own_keys(kiln_host *host, kiln_value target, kiln_value *out, int depth,
                                 kiln_value *thrown) {
    kiln_object *object;
    kiln_value built;
    size_t index;
    size_t count;
    int status;
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_proxy_all_keys(host, target, out, depth, thrown);
    }
    if (target.tag == KILN_VALUE_FUNCTION || target.tag == KILN_VALUE_BOUND ||
        target.tag == KILN_VALUE_METHOD || target.tag == KILN_VALUE_SETTLER) {
        kiln_value store, extra;
        kiln_closure *closure = target.tag == KILN_VALUE_FUNCTION ? kiln_c99_closure_of(target) : NULL;
        int owns_prototype = closure != NULL && !closure->arrow &&
            (!closure->asynchronous || closure->async_generator) &&
            (!closure->is_method || closure->generator);
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_array_push(built, kiln_c99_copy_text("length", 6));
        if (status == KILN_C99_OK) status = kiln_c99_array_push(built, kiln_c99_copy_text("name", 4));
        if (status == KILN_C99_OK && owns_prototype)
            status = kiln_c99_array_push(built, kiln_c99_copy_text("prototype", 9));
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_callable_store(target, &store, 0);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_all_own_keys(host, store, &extra, depth, thrown);
        if (status != KILN_C99_OK) return status;
        for (index = 0; index < kiln_c99_array_of(extra)->count; ++index) {
            kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(extra)->elements[index]);
            if (key.tag == KILN_VALUE_STRING &&
                (kiln_c99_same_name_key((const kiln_string *)key.ptr, "length") ||
                 kiln_c99_same_name_key((const kiln_string *)key.ptr, "name") ||
                 (owns_prototype && kiln_c99_same_name_key((const kiln_string *)key.ptr, "prototype")))) continue;
            status = kiln_c99_array_push(built, key);
            if (status != KILN_C99_OK) return status;
        }
        *out = built;
        return KILN_C99_OK;
    } else if (target.tag == KILN_VALUE_COLLECTION) {
        target = kiln_c99_collection_of(target)->properties;
    } else if (target.tag == KILN_VALUE_GENERATOR) {
        target = kiln_c99_generator_of(target)->properties;
    } else if (target.tag == KILN_VALUE_PROMISE) {
        target = kiln_c99_promise_of(target)->properties;
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
            status = kiln_c99_shared_field_key(lease, at, &key, NULL);
            if (status == KILN_C99_OK) status = kiln_c99_array_push(built, key);
        }
        if (status == KILN_C99_OK && node.array)
            status = kiln_c99_array_push(built, kiln_c99_copy_text("length", 6));
        (void)kiln_shared_graph_release(lease);
        if (status == KILN_C99_OK) *out = built;
        return status;
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(target);
        kiln_value extra;
        for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
            if (array->elements[index] != KILN_VAL_HOLE)
                status = kiln_c99_array_push(built, kiln_c99_text(kiln_c99_number((double)index)));
        }
        if (status == KILN_C99_OK)
            status = kiln_c99_array_push(built, kiln_c99_copy_text("length", 6));
        if (status != KILN_C99_OK) return status;
        if (array->properties.tag == KILN_VALUE_OBJECT) {
            status = kiln_c99_all_own_keys(host, array->properties, &extra, depth, thrown);
            if (status != KILN_C99_OK) return status;
            for (index = 0; index < kiln_c99_array_of(extra)->count; ++index) {
                status = kiln_c99_array_push(built,
                    kiln_val_to_boundary(kiln_c99_array_of(extra)->elements[index]));
                if (status != KILN_C99_OK) return status;
            }
        }
        *out = built;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_TYPED_ARRAY) {
        kiln_typed_array *view = kiln_c99_typed_array_of(target);
        for (index = 0; index < view->length && status == KILN_C99_OK; ++index) {
            status = kiln_c99_array_push(built, kiln_c99_text(kiln_c99_number((double)index)));
        }
        *out = built;
        return status;
    }
    if (target.tag == KILN_VALUE_REGEXP) {
        /* RegExp instances have one own property, `lastIndex`. It is non-enumerable, but
         * Reflect.ownKeys and Object.getOwnPropertyNames must still report it. */
        kiln_value store = kiln_c99_regexp_of(target)->properties;
        kiln_value extra = kiln_c99_undefined();
        kiln_array *listed = NULL;
        if (store.tag == KILN_VALUE_OBJECT) {
            status = kiln_c99_all_own_keys(host, store, &extra, depth, thrown);
            if (status != KILN_C99_OK) return status;
            listed = kiln_c99_array_of(extra);
            /* OrdinaryOwnPropertyKeys places integer-index names before all other strings. */
            for (index = 0; index < listed->count && status == KILN_C99_OK; ++index) {
                kiln_value key = kiln_val_to_boundary(listed->elements[index]);
                if (key.tag == KILN_VALUE_STRING &&
                    kiln_c99_key_index_key((const kiln_string *)key.ptr) >= 0.0)
                    status = kiln_c99_array_push(built, key);
            }
        }
        if (status == KILN_C99_OK)
            status = kiln_c99_array_push(built, kiln_c99_copy_text("lastIndex", 9));
        if (status == KILN_C99_OK && store.tag == KILN_VALUE_OBJECT) {
            for (index = 0; index < listed->count && status == KILN_C99_OK; ++index) {
                kiln_value key = kiln_val_to_boundary(listed->elements[index]);
                if (key.tag == KILN_VALUE_STRING &&
                    kiln_c99_key_index_key((const kiln_string *)key.ptr) >= 0.0) continue;
                status = kiln_c99_array_push(built, key);
            }
        }
        if (status == KILN_C99_OK) *out = built;
        return status;
    }
    if (target.tag == KILN_VALUE_BUILTIN) {
        return kiln_c99_builtin_own_keys(target.bits, out);
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        *out = built;
        return KILN_C99_OK;
    }
    /* `OrdinaryOwnPropertyKeys`: the string keys, then the symbol keys, each in insertion order.
     * Two passes because the order is part of the answer -- a copy that walks them reproduces
     * it, and the specification puts every symbol after every name. */
    object = kiln_c99_object_of(target);
    status = kiln_c99_property_walk(object, &count);
    if (status != KILN_C99_OK) return status;
    for (index = 0; index < count && status == KILN_C99_OK; ++index) {
        kiln_property projected;
        const kiln_property *property = kiln_c99_property_view(object, index, &projected);
        if (kiln_c99_key_is_reserved(property->key)) {
            continue;
        }
        status = kiln_c99_array_push(built, kiln_c99_stable_key_value(property->key));
    }
    for (index = 0; index < count && status == KILN_C99_OK; ++index) {
        kiln_property projected;
        const kiln_property *property = kiln_c99_property_view(object, index, &projected);
        kiln_value symbol;
        /* A `@@` key is either a symbol-keyed property or the machine's own bookkeeping. The
         * first is reported and the second never is; asking the symbol registry is what tells
         * them apart, because the key alone cannot. */
        if (!kiln_c99_key_is_reserved(property->key)) {
            continue;
        }
        if (!kiln_c99_symbol_of_key_view(property->key, &symbol)) {
            continue;
        }
        status = kiln_c99_array_push(built, symbol);
    }
    *out = built;
    return status;
}

/* The ordinary property store an exotic value carries, or NULL if its kind has none.
 *
 * An array and a collection are not objects here, but `class Stack extends Array {}` gives their
 * instances both a prototype to inherit from and somewhere for a named write to land. Those are
 * the same question, so the two kinds answer it in one place rather than each spelling out where
 * it keeps its store. */
static kiln_value *kiln_c99_exotic_store(kiln_value target) {
    if (target.tag == KILN_VALUE_GENERATOR) return &kiln_c99_generator_of(target)->properties;
    if (target.tag == KILN_VALUE_BOUND) return &((kiln_bound *)(uintptr_t)target.bits)->properties;
    if (target.tag == KILN_VALUE_ARRAY) {
        kiln_array *arr = kiln_c99_array_of(target);
        if (arr->match_index >= 0 && arr->properties.tag != KILN_VALUE_OBJECT) {
            kiln_c99_materialize_match_properties(arr);
        }
        return &arr->properties;
    }
    if (target.tag == KILN_VALUE_COLLECTION) {
        return &kiln_c99_collection_of(target)->properties;
    }
    if (target.tag == KILN_VALUE_PROMISE) {
        return &kiln_c99_promise_of(target)->properties;
    }
    if (target.tag == KILN_VALUE_REGEXP) {
        return &kiln_c99_regexp_of(target)->properties;
    }
    return NULL;
}

/* Read `key` out of an exotic value's store and the prototype chain above it, reporting whether
 * anything was there -- because *absent* and *present and undefined* lead to different places at
 * every call site: one falls through to the refusal that names an unimplemented member, the
 * other is the answer.
 *
 * The receiver stays the exotic value itself, not the store: a getter a subclass declared has to
 * see the array. */
static int kiln_c99_store_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                              kiln_value *out, int depth, kiln_value *thrown, int *found) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
    view.unit_len = (uint32_t)key_len;
    view.flags = kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_store_get_key(host, target, &view, NULL, out, depth, thrown, found);
}
