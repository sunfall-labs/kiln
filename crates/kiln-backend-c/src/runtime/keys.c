
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
    return kiln_c99_has_property(host, target, name.ptr, name.len, answer, depth, thrown);
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
    return kiln_c99_delete(host, target, name.ptr, name.len, answer, depth, thrown);
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
            if (member->builtin != builtin) {
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
    for (code = 1; code < 256; ++code) {
        const char *name = kiln_c99_method_name(code);
        size_t len;
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
    int status;
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_proxy_all_keys(host, target, out, depth, thrown);
    }
    if (target.tag == KILN_VALUE_FUNCTION) {
        target = kiln_c99_closure_of(target)->properties;
    } else if (target.tag == KILN_VALUE_COLLECTION) {
        target = kiln_c99_collection_of(target)->properties;
    } else if (target.tag == KILN_VALUE_PROMISE) {
        target = kiln_c99_promise_of(target)->properties;
    }
    status = kiln_c99_new_array(&built);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (target.tag == KILN_VALUE_TYPED_ARRAY) {
        kiln_typed_array *view = kiln_c99_typed_array_of(target);
        for (index = 0; index < view->length && status == KILN_C99_OK; ++index) {
            status = kiln_c99_array_push(built, kiln_c99_text(kiln_c99_number((double)index)));
        }
        *out = built;
        return status;
    }
    if (target.tag == KILN_VALUE_BUILTIN) {
        return kiln_c99_builtin_own_keys(target.bits, out);
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        *out = built;
        return KILN_C99_OK;
    }
    kiln_c99_node_materialize_all_headers(target);
    /* `OrdinaryOwnPropertyKeys`: the string keys, then the symbol keys, each in insertion order.
     * Two passes because the order is part of the answer -- a copy that walks them reproduces
     * it, and the specification puts every symbol after every name. */
    object = kiln_c99_object_of(target);
    if (object->shape != NULL) {
        kiln_c99_ensure_dictionary(object);
    }
    for (index = 0; index < object->count && status == KILN_C99_OK; ++index) {
        if (object->properties[index].key_len >= 2 && object->properties[index].key[0] == '@' &&
            object->properties[index].key[1] == '@') {
            continue;
        }
        status = kiln_c99_array_push(built,
                                     kiln_c99_copy_text(object->properties[index].key,
                                                        object->properties[index].key_len));
    }
    for (index = 0; index < object->count && status == KILN_C99_OK; ++index) {
        kiln_value symbol;
        /* A `@@` key is either a symbol-keyed property or the machine's own bookkeeping. The
         * first is reported and the second never is; asking the symbol registry is what tells
         * them apart, because the key alone cannot. */
        if (object->properties[index].key_len < 2 || object->properties[index].key[0] != '@' ||
            object->properties[index].key[1] != '@') {
            continue;
        }
        if (!kiln_c99_symbol_of_key(object->properties[index].key,
                                    object->properties[index].key_len, &symbol)) {
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
    kiln_value *slot = kiln_c99_exotic_store(target);
    kiln_value current;
    size_t steps;
    *found = 0;
    if (slot == NULL) {
        return KILN_C99_OK;
    }
    current = *slot;
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        kiln_object *object;
        size_t index;
        /* A chain that reaches a constructor continues into its statics, exactly as an
         * object's does. */
        if (current.tag == KILN_VALUE_FUNCTION) {
            current = kiln_c99_closure_of(current)->properties;
        }
        if (current.tag != KILN_VALUE_OBJECT) {
            return KILN_C99_OK;
        }
        object = kiln_c99_object_of(current);
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
        }
        for (index = 0; index < object->count; ++index) {
            if (!kiln_c99_same_key(&object->properties[index], key, key_len)) {
                continue;
            }
            *found = 1;
            if (object->properties[index].refused) {
                KILN_C99_REFUSE_NAMED("a member this backend declares and has not implemented", 
                                      key, key_len);
            }
            if (!object->properties[index].accessor) {
                *out = object->properties[index].value;
                return KILN_C99_OK;
            }
            if (object->properties[index].getter.tag == KILN_VALUE_UNDEFINED) {
                KILN_C99_ABSENT("an accessor with no getter, whose read is undefined by "
                                "definition",
                                out);
            }
            return kiln_c99_call(host, out, object->properties[index].getter, target, NULL, 0,
                                 depth, thrown);
        }
        current = object->prototype;
    }
    return KILN_C99_OK;
}
