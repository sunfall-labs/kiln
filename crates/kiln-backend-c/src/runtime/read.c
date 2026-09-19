
/* Fixed-name readers still have byte-oriented tables.  A headed key can use those tables only
 * when every unit is ASCII; this adapter is exact in that case.  A wide or non-ASCII key takes
 * the headed property walk below rather than being converted or refused. */
static int kiln_c99_intrinsic_ascii_view(const kiln_string *key, char buffer[256],
                                         const char **bytes, size_t *length) {
    size_t index;
    if (key == NULL) return 0;
    if (!kiln_string_is_utf16(key)) {
        const unsigned char *raw = (const unsigned char *)kiln_string_latin1(key);
        for (index = 0; index < key->unit_len; ++index) {
            if (raw[index] > 0x7f) return 0;
        }
        *bytes = (const char *)kiln_string_latin1(key);
        *length = key->unit_len;
        return 1;
    }
    if (key->unit_len > 256) return 0;
    for (index = 0; index < key->unit_len; ++index) {
        uint32_t unit = kiln_string_unit_at(key, index);
        if (unit > 0x7f) return 0;
        buffer[index] = (char)unit;
    }
    *bytes = buffer;
    *length = key->unit_len;
    return 1;
}

/* The boundary value used by a proxy trap must point at a realm-owned header.  A borrowed view
 * is fine for an ordinary walk, but a trap can retain the key in user code, so it is interned
 * exactly at this exposure point. */
static kiln_value kiln_c99_key_value(const kiln_string *view, const kiln_string **canonical) {
    const kiln_string *stable = canonical == NULL ? NULL : *canonical;
    if (stable == NULL) {
        stable = kiln_c99_intern_key(view);
        if (stable == NULL) return kiln_c99_undefined();
        if (canonical != NULL) *canonical = stable;
    }
    return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)stable, stable->unit_len);
}

#include <assert.h>

/* Stored shape/property keys are already realm-owned canonical headers. Keep their exposure
 * allocation-free; only a borrowed computed key takes the interning path above. */
static kiln_value kiln_c99_stable_key_value(const kiln_string *stable) {
    assert(stable != NULL);
    return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)stable, stable->unit_len);
}

/* A read the instance path could not answer, tried against the prototype built-in's own store.
 * That store is where the runtime bootstrap defines the members it writes in TypeScript
 * (design.md §5): `RegExp.prototype[@@replace]`, `Date.prototype[@@toPrimitive]`,
 * `Function.prototype[@@hasInstance]`, `Symbol.prototype[@@toPrimitive]`, and it is the same
 * store the built-in consults first for a read off the prototype object itself, so `re[k]` and
 * `RegExp.prototype[k]` answer alike. `*found` says whether the store had the key; when it did
 * not, the caller goes on to its own refusal or absence, whichever the name deserves. */
static int kiln_c99_prototype_store_read(kiln_host *host, uint64_t prototype,
                                         const kiln_string *key, const kiln_string *ikey,
                                         kiln_value *out, int depth, kiln_value *thrown,
                                         int *found) {
    kiln_value store;
    int status = kiln_c99_callable_store(kiln_c99_builtin(prototype), &store, 0);
    *found = 0;
    if (status != KILN_C99_OK) return status;
    if (store.tag == KILN_VALUE_OBJECT) {
        kiln_object *properties = kiln_c99_object_of(store);
        size_t index;
        if (properties->shape != NULL) kiln_c99_ensure_dictionary(properties);
        for (index = 0; index < properties->count; ++index) {
            if (kiln_c99_same_key_view(&properties->properties[index], key)) {
                *found = 1;
                return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
            }
        }
    }
    return KILN_C99_OK;
}

static int kiln_c99_get_own_kind(kiln_host *host, kiln_value target, const kiln_string *key,
                                 const kiln_string *ikey, kiln_value *out, int depth,
                                 kiln_value *thrown) {
    /* `length` only means anything special to an array or a string; every other kind -- and an
     * ordinary object, the one a request or response property read is, most of all -- paid for
     * this `memcmp` on every read whether or not the key could possibly be it. Checked instead at
     * the two places that use it, so the cost is real only where the answer can matter. */
    size_t index;
    size_t key_len = key == NULL ? 0 : key->unit_len;
    char intrinsic_buffer[256];
    const char *intrinsic_key = NULL;
    size_t intrinsic_len = 0;
    switch (target.tag) {
    case KILN_VALUE_OBJECT: {
        kiln_value current = target;
        size_t steps;
        int walk;
        /* Canonicalized once for the whole walk (ADR 0047), unless a compiled site already holds
         * the canonical pointer. NULL is a spelling the realm has never stored, so no level of
         * the chain can hold it and the walk is not made at all -- unless the check mode is on,
         * which wants every level's scan run by bytes. */
        if (ikey == NULL) ikey = kiln_c99_interned_key(key);
        walk = ikey != NULL || kiln_c99_keys_checked() || kiln_c99_key_index_key(key) >= 0.0;
        /* Own properties first, then the chain. Iterative and bounded rather than recursive:
         * the walk is on the C stack, and a program can build a chain long enough to exhaust
         * it. */
        for (steps = 0; walk && steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
            kiln_object *object;
            /* A constructor's statics live in its own store, so a chain that reaches one
             * continues *into* that store rather than stopping at the function. */
            if (current.tag == KILN_VALUE_FUNCTION) {
                current = kiln_c99_closure_of(current)->properties;
            }
            if (current.tag != KILN_VALUE_OBJECT) {
                if (current.tag != KILN_VALUE_UNDEFINED && current.tag != KILN_VALUE_NULL) {
                    return kiln_c99_get_key(host, current, key, ikey, out, depth, thrown);
                }
                break;
            }
            object = kiln_c99_object_of(current);
            if (object->shape != NULL) {
                if (ikey != NULL) {
                    uint32_t slot;
                    if (kiln_c99_shape_find_slot(object->shape, ikey, &slot)) {
                        if (slot < object->slot_capacity) {
                            *out = kiln_val_to_boundary(object->slots[slot]);
                            return KILN_C99_OK;
                        }
                    }
                }
                if (object->elements != NULL) {
                    double index_num = kiln_c99_key_index_key(key);
                    if (index_num >= 0.0 && index_num < 100000.0) {
                        size_t elem_idx = (size_t)index_num;
                        if (elem_idx < object->element_count &&
                            object->elements[elem_idx] != KILN_VAL_HOLE) {
                            *out = kiln_val_to_boundary(object->elements[elem_idx]);
                            return KILN_C99_OK;
                        }
                    }
                }
                if (object->properties == NULL) {
                    current = object->prototype;
                    continue;
                }
                kiln_c99_ensure_dictionary(object);
            }
            index = kiln_c99_own_index_key(object, ikey, key);
            if (index != object->count) {
                /* A published name with nothing behind it stops the program and lets the
                 * instruction say which one. See `kiln_property::refused`. */
                if (object->properties[index].refused) {
                    KILN_C99_REFUSE("a member this backend declares and has not implemented");
                }
                if (!object->properties[index].accessor) {
                    *out = object->properties[index].value;
                    return KILN_C99_OK;
                }
                /* Reading an accessor is a call, and the receiver is the object the read
                 * started from -- not the one the accessor was found on. */
                if (object->properties[index].getter.tag == KILN_VALUE_UNDEFINED) {
                    KILN_C99_ABSENT("an accessor with no getter, whose read is undefined by "
                                    "definition",
                                    out);
                }
                return kiln_c99_call(host, out, object->properties[index].getter, target, NULL, 0,
                                     depth, thrown);
            }
            /* Lazy HTTP headers live behind a hidden pointer only `node:http`
             * installs. Without the HTTP group the pointer can never exist,
             * so this probe is compiled out (see `retained_method_groups`). */
#if KILN_RETAIN_GROUP_NODE_HTTP
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                                &intrinsic_len);
            if (intrinsic_key != NULL && kiln_c99_node_materialize_header(current, intrinsic_key, intrinsic_len, out)) {
                return KILN_C99_OK;
            }
#endif
            current = object->prototype;
        }
        /* An iterator helper's chain ends at `Iterator.prototype`, which is a method table rather
         * than an object, so the walk above stopped the moment it reached it. Answer the helpers
         * out of the same table `Iterator.prototype` itself is read through: `endless.map(f)`
         * hands back an object, and `.take(3)` has to find `take` on it or the pipeline is one
         * call long.
         *
         * Three answers, in the order the language gives them. A helper is a function; a member
         * the language puts here that this backend has not built refuses by name; anything else
         * falls through to the ordinary object answer below, which is undefined. That last
         * distinction is the whole point -- `it.nope` is undefined on any object and must stay
         * so, while `it.chunks` reading undefined would quietly fail a feature test. */
        if (kiln_c99_object_of(target)->prototype.tag == KILN_VALUE_BUILTIN &&
            kiln_c99_object_of(target)->prototype.bits == KILN_C99_BUILTIN_ITERATOR_PROTOTYPE) {
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                                &intrinsic_len);
            uint64_t helper = kiln_c99_iterator_prototype_method(intrinsic_key, intrinsic_len);
            if (helper != 0) {
                *out = kiln_c99_method(helper);
                return KILN_C99_OK;
            }
            if (kiln_c99_same_name_key(key, "constructor")) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_ITERATOR);
                return KILN_C99_OK;
            }
            if (kiln_c99_iterator_prototype_gap(intrinsic_key, intrinsic_len)) {
                KILN_C99_REFUSE("an Iterator.prototype helper this backend has not implemented");
            }
        }
        /* The chain ends at `Object.prototype`, whose members every object has -- unless the
         * program asked for a bare dictionary with `Object.create(null)`, which is exactly what
         * that asks for. */
        if (kiln_c99_object_of(target)->prototype.tag != KILN_VALUE_NULL) {
            uint64_t inherited;
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                                &intrinsic_len);
            /* An error inherits `toString` from `Error.prototype`, not from `Object.prototype`:
             * it is the one object that converts to something other than "[object Object]". The
             * range lookup below would hand out the generic method and `String(err)` would lose
             * the message, so the error's own is answered first. */
            if (kiln_c99_same_name_key(key, "toString") &&
                kiln_c99_error_kind(target) != 0) {
                *out = kiln_c99_method(KILN_C99_METHOD_ERROR_TO_STRING);
                return KILN_C99_OK;
            }
            /* `name` and `message` live on the error prototypes, not on the instance, so an own
             * `name = "TimeoutError"` class field and a `get name()` on a subclass prototype are
             * both found by the walk above and only the default reaches here. */
            if (kiln_c99_same_name_key(key, "name") ||
                kiln_c99_same_name_key(key, "message")) {
                uint64_t kind = kiln_c99_error_kind(target);
                const char *named = kind == 0 ? NULL : kiln_c99_error_name(kind);
                if (named != NULL) {
                    *out = key_len == 4 ? kiln_c99_copy_text(named, strlen(named))
                                        : kiln_c99_from_units(NULL, 0);
                    return KILN_C99_OK;
                }
            }
            inherited = kiln_c99_namespace_method(KILN_C99_BUILTIN_OBJECT_PROTOTYPE,
                                                  intrinsic_key, intrinsic_len);
            if (inherited != 0) {
                *out = kiln_c99_method(inherited);
                return KILN_C99_OK;
            }
            if (kiln_c99_same_name_key(key, "constructor")) {
                /* An error already records its kind, and `error.constructor.name` is the ordinary
                 * way a program asks which one it caught -- so the answer comes from the kind
                 * rather than from the fact that an error is an object. */
                uint64_t kind = kiln_c99_error_kind(target);
                *out = kiln_c99_builtin(kind != 0 ? kind : KILN_C99_BUILTIN_OBJECT);
                return KILN_C99_OK;
            }
        }
        if (kiln_c99_same_name_key(key, "__proto__") &&
            kiln_c99_object_of(target)->prototype.tag != KILN_VALUE_NULL) {
            /* An accessor on `Object.prototype`, not a property, which is why it is answered
             * here rather than stored: it reports the same link `Object.getPrototypeOf` does and
             * has to keep reporting it after a `setPrototypeOf` moves it. Reading undefined for
             * it was a silent wrong answer about a name every second library still uses.
             *
             * `Object.create(null)` does not inherit the accessor, so on such an object
             * `__proto__` is an ordinary absent name and stays undefined -- reached above,
             * because the chain ends in an explicit null rather than in nothing recorded. */
            return kiln_c99_invoke_method(host, out, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                                          kiln_c99_undefined(), &target, 1, depth, thrown);
        }
        KILN_C99_ABSENT("a name an ordinary object does not own, which is what the language "
                        "answers",
                        out);
    }
    case KILN_VALUE_FUNCTION: {
        kiln_closure *closure = kiln_c99_closure_of(target);
        if (kiln_c99_same_name_key(key, "prototype") &&
            !closure->arrow &&
            (!closure->asynchronous || closure->async_generator) &&
            (!closure->is_method || closure->generator)) {
            return kiln_c99_prototype_object(target, out);
        }
        if (kiln_c99_same_name_key(key, "name")) {
            *out = closure->name == NULL ? kiln_c99_from_units(NULL, 0)
                                         : kiln_c99_stable_key_value(closure->name);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "length")) {
            *out = kiln_c99_number((double)closure->arity);
            return KILN_C99_OK;
        }
        if (closure->properties.tag == KILN_VALUE_OBJECT) {
            kiln_object *store = kiln_c99_object_of(closure->properties);
            size_t index;
            if (store->shape != NULL) {
                kiln_c99_ensure_dictionary(store);
            }
            for (index = 0; index < store->count; ++index) {
                if (kiln_c99_same_key_view(&store->properties[index], key)) {
                    /* An assigned property wins, exactly as an own property shadows an
                     * inherited one -- which is what `Function.prototype.call` is. */
                    return kiln_c99_get_key(host, closure->properties, key, ikey, out, depth,
                                        thrown);
                }
            }
        }
        if (kiln_c99_same_name_key(key, "@@species")) {
            *out = target;
            return KILN_C99_OK;
        }
        {
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                                &intrinsic_len);
            uint64_t callable = kiln_c99_callable_method(intrinsic_key, intrinsic_len);
            if (callable != 0) {
                *out = kiln_c99_method(callable);
                return KILN_C99_OK;
            }
        }
        /* `f.constructor === Function`, which is how a program asks what it is holding. The
         * bound and method paths already answered this; a plain function did not. */
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION);
            return KILN_C99_OK;
        }
        /* ...and the `Object.prototype` members a program borrows, which the builtin path below
         * already answers and this one never did.
         *
         * `valueOf` is here and `toString` is not, which is the whole of the distinction once
         * written as "toString and valueOf would shadow what a constructor answers". Only
         * `toString` would: `Object.prototype.toString` gives `[object Function]`, which is worse
         * than refusing, and a callable answers that name for itself through
         * `kiln_c99_callable_method`. `Object.prototype.valueOf` hands back the receiver, so it
         * shadows nothing -- `ToPrimitive` gets a non-primitive from it and falls through to
         * `toString` exactly as it does for a plain object. */
        {
            uint64_t borrowed =
                kiln_c99_same_name_key(key, "hasOwnProperty") ? KILN_C99_METHOD_HAS_OWN_PROPERTY
                : kiln_c99_same_name_key(key, "isPrototypeOf") ? KILN_C99_METHOD_IS_PROTOTYPE_OF
                : kiln_c99_same_name_key(key, "propertyIsEnumerable")
                    ? KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE
                : kiln_c99_same_name_key(key, "valueOf") ? KILN_C99_METHOD_OBJECT_VALUE_OF
                    : 0;
            if (borrowed != 0) {
                *out = kiln_c99_method(borrowed);
                return KILN_C99_OK;
            }
        }
        /* Own members were answered above; what `Function.prototype` carries in its store comes
         * next, before the name is called absent. */
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_FUNCTION_PROTOTYPE,
                                                       key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
        }
        if (closure->properties.tag != KILN_VALUE_OBJECT) {
            KILN_C99_ABSENT("a callable with no property store, so it has no own member to "
                            "answer",
                            out);
        }
        return kiln_c99_get_key(host, closure->properties, key, ikey, out, depth, thrown);
    }
#if KILN_RETAIN_GROUP_TEMPORAL
    case KILN_VALUE_TEMPORAL: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        /* Every calendar and wall-clock field of all eight types comes through one name-keyed call
         * into the engine, so there is no per-field code here and no second table to drift. */
        char inline_buffer[64];
        double number = 0.0;
        int32_t length = 0;
        int32_t shape = kiln_temporal_get(kiln_c99_temporal_of(target),
                                          (const unsigned char *)intrinsic_key, intrinsic_len, &number,
                                          (unsigned char *)inline_buffer, sizeof inline_buffer,
                                          &length);
        switch (shape) {
        case 1:
            *out = kiln_c99_number(number);
            return KILN_C99_OK;
        case 3:
            *out = kiln_c99_boolean(number != 0.0);
            return KILN_C99_OK;
        case 4:
            /* The property exists and its answer is `undefined` -- `weekOfYear` in a calendar with
             * no week numbering. Not the same as having no such property. */
            KILN_C99_ABSENT("a Temporal field the calendar leaves empty, which is a value and not "
                            "a missing member",
                            out);
        case 2:
        case 5: {
            /* Two shapes, one buffer. Shape 2 is text the language shows as a string; shape 5 is
             * `epochNanoseconds`, whose exact value only a BigInt can hold, so it arrives as its
             * decimal digits and is parsed into one rather than read out of the double slot --
             * a `Temporal` instant carries more precision than a double, and rounding it would be
             * a wrong answer that prints. */
            const int wide = shape == 5;
            const char *digits = inline_buffer;
            char *spilled = NULL;
            void *big = NULL;
            if (length < 0) {
                size_t wanted = (size_t)(-length);
                spilled = (char *)malloc(wanted);
                if (spilled == NULL) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
                shape = kiln_temporal_get(kiln_c99_temporal_of(target),
                                          (const unsigned char *)intrinsic_key, intrinsic_len, &number,
                                          (unsigned char *)spilled, wanted, &length);
                if (shape != (wide ? 5 : 2) || length < 0) {
                    free(spilled);
                    KILN_C99_REFUSE("a Temporal field whose text the provider would not produce");
                }
                digits = spilled;
            }
            if (!wide) {
                *out = kiln_c99_copy_text(digits, (size_t)length);
                free(spilled);
                return KILN_C99_OK;
            }
            if (kiln_bigint_parse((const unsigned char *)digits, (size_t)length, 10, &big) != 0) {
                free(spilled);
                KILN_C99_REFUSE("a Temporal field whose digits the BigInt provider would not "
                                "parse");
            }
            free(spilled);
            *out = kiln_c99_bigint_value(big);
            return KILN_C99_OK;
        }
        default:
            break;
        }
        if (kiln_c99_same_name_key(key, "toLocaleString")) {
            /* `toString` is in the operation table below instead, because it takes options --
             * `{ smallestUnit: "minute" }` really does shorten the text. `toLocaleString` takes
             * locale arguments this realm does not have, and answers the plain form. */
            *out = kiln_c99_method(KILN_C99_METHOD_TEMPORAL_TO_STRING);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "toJSON")) {
            *out = kiln_c99_method(KILN_C99_METHOD_TEMPORAL_TO_JSON);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "valueOf")) {
            *out = kiln_c99_method(KILN_C99_METHOD_TEMPORAL_VALUE_OF);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "@@toStringTag")) {
            const char *tag = kiln_c99_temporal_tag(target);
            *out = kiln_c99_copy_text(tag, strlen(tag));
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(kiln_c99_temporal_builtin_of(target));
            return KILN_C99_OK;
        }
        /* The arithmetic and conversion surface -- `add`, `until`, `round`, `with`, `equals`,
         * `toPlainDate` and their siblings -- through the one name-keyed call into the engine that
         * the readers above already use. */
        {
            uint64_t operation = kiln_c99_temporal_op_code(intrinsic_key, intrinsic_len);
            if (operation != 0) {
                *out = kiln_c99_method(operation);
                return KILN_C99_OK;
            }
        }
        /* A well-known symbol the type does not carry is *absent*, not unwired, and absence is
         * the answer the language gives. `@@toStringTag` is answered above because `Temporal`
         * does define it; every other `@@` key is the machine probing rather than a member the
         * program named, and `kiln_c99_to_primitive` opens by asking for `@@toPrimitive` on every
         * reference it reduces. Refusing that probe turned `String(aPlainDate)` into a thrown
         * "unsupported Temporal member" -- a refusal for a conversion the value plainly has, one
         * step after the missing text case stopped it reading `undefined`. */
        if (kiln_c99_key_is_reserved(key)) {
            KILN_C99_ABSENT("a well-known symbol this Temporal type does not carry", out);
        }
        /* A name that exists in the language and is not wired says which. Reading `undefined`
         * would surface three frames away as "not a function". */
        return kiln_c99_temporal_unsupported(thrown, kiln_c99_temporal_tag(target), intrinsic_key, intrinsic_len);
    }
#endif
    case KILN_VALUE_REGEXP: {
        kiln_regexp *regexp = kiln_c99_regexp_of(target);
        size_t index;
        /* The flag letters, in the order the specification lists their accessors. */
        static const char known[] = "dgimsuvy";
        static const char *const names[] = {"hasIndices", "global",  "ignoreCase", "multiline",
                                            "dotAll",     "unicode", "unicodeSets", "sticky"};
        if (kiln_c99_same_name_key(key, "source")) {
            *out = kiln_c99_stable_key_value(regexp->source);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "flags")) {
            *out = kiln_c99_stable_key_value(regexp->flags);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "lastIndex")) {
            *out = kiln_c99_number(regexp->last_index);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_REGEXP);
            return KILN_C99_OK;
        }
        /* Each flag is a boolean named after itself, read off the flag *string* rather than from
         * a field per flag -- so a flag the backend does nothing else with still reads back. */
        for (index = 0; index + 1 < sizeof known; ++index) {
            size_t at;
            int present = 0;
            if (!kiln_c99_same_name_key(key, names[index])) {
                continue;
            }
            for (at = 0; regexp->flags != NULL && at < regexp->flags->unit_len; ++at) {
                present = present || kiln_string_unit_at(regexp->flags, at) == (unsigned char)known[index];
            }
            *out = kiln_c99_boolean(present);
            return KILN_C99_OK;
        }
        {
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                                &intrinsic_len);
            uint64_t code = kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_REGEXP_TEST,
                                                   KILN_C99_METHOD_REGEXP_TO_STRING);
            if (code != 0) {
                *out = kiln_c99_method(code);
                return KILN_C99_OK;
            }
        }
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_REGEXP_PROTOTYPE, key,
                                                       ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
        }
        KILN_C99_REFUSE("a RegExp.prototype member this backend has not implemented");
    }
    case KILN_VALUE_ARRAY_BUFFER:
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        if (kiln_c99_same_name_key(key, "byteLength")) {
            *out = kiln_c99_number((double)kiln_c99_typed_buffer_of(target)->byte_length);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "slice")) {
            *out = kiln_c99_method(KILN_C99_METHOD_SLICE);
            return KILN_C99_OK;
        }
        if (kiln_c99_is_array_buffer_member(intrinsic_key, intrinsic_len)) {
            KILN_C99_REFUSE("an ArrayBuffer.prototype member this backend has not implemented");
        }
        KILN_C99_ABSENT("not a member of ArrayBuffer.prototype; kiln_c99_is_array_buffer_member "
                        "holds the ones that are",
                        out);
    case KILN_VALUE_DATA_VIEW: {
        kiln_data_view *view = kiln_c99_data_view_of(target);
        if (kiln_c99_same_name_key(key, "buffer")) {
            /* The buffer the view was made over, not a copy of it: `new DataView(b).buffer === b`
             * is what a program checks before it starts writing through one. */
            *out = kiln_c99_value(KILN_VALUE_ARRAY_BUFFER, (uint64_t)(uintptr_t)view->buffer,
                                  NULL, 0);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "byteLength")) {
            *out = kiln_c99_number((double)view->byte_length);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "byteOffset")) {
            *out = kiln_c99_number((double)view->byte_offset);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_DATA_VIEW);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "@@toStringTag")) {
            /* The same string `Object.prototype.toString` reports, answered here for the reason a
             * typed view answers its own: a program reads one spelling or the other, and the two
             * must not be able to disagree. */
            *out = kiln_c99_copy_text("DataView", 8);
            return KILN_C99_OK;
        }
        {
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                                &intrinsic_len);
            /* One range, shared with the prototype table and with `in`, so a read and a presence
             * test cannot disagree about which names a view has. No `@@iterator` and no indices:
             * a DataView is not iterable and has no element to index, which is exactly what
             * distinguishes it from the typed views below. */
            uint64_t method =
                kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_DATA_VIEW_GET_INT8,
                                       KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64);
            if (method == 0 && kiln_c99_is_data_view_member(intrinsic_key, intrinsic_len)) {
                /* A real member of `DataView.prototype` this backend has not grown. The
                 * interpreter answers a function for both, so `undefined` here would be the
                 * silent kind of wrong -- the program would take the branch written for a realm
                 * without half precision, which is a different program. It refuses and joins the
                 * backlog by name, exactly as an unimplemented typed-array method does. */
                KILN_C99_REFUSE("a DataView.prototype member this backend has not implemented");
            }
            if (method == 0) {
                method = kiln_c99_same_name_key(key, "isPrototypeOf")
                             ? KILN_C99_METHOD_IS_PROTOTYPE_OF
                         : kiln_c99_same_name_key(key, "propertyIsEnumerable")
                             ? KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE
                             : kiln_c99_lookup_method(intrinsic_key, intrinsic_len,
                                                      KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                                      KILN_C99_METHOD_OBJECT_VALUE_OF);
            }
            *out = method == 0 ? kiln_c99_undefined() : kiln_c99_method(method);
        }
        return KILN_C99_OK;
    }
    case KILN_VALUE_TYPED_ARRAY: {
        kiln_typed_array *view = kiln_c99_typed_array_of(target);
        if (kiln_c99_same_name_key(key, "@@toStringTag")) {
            const char *name = view->is_buffer ? "Uint8Array" : kiln_c99_typed_name(view->kind);
            *out = kiln_c99_copy_text(name, strlen(name));
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "length")) {
            *out = kiln_c99_number((double)view->length);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "byteLength")) {
            *out = kiln_c99_number((double)(view->length * kiln_c99_typed_width(view->kind)));
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "byteOffset")) {
            *out = kiln_c99_number((double)view->byte_offset);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "BYTES_PER_ELEMENT")) {
            *out = kiln_c99_number((double)kiln_c99_typed_width(view->kind));
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "buffer")) {
            *out = kiln_c99_value(KILN_VALUE_ARRAY_BUFFER,
                                  (uint64_t)(uintptr_t)view->buffer, NULL, 0);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(view->is_buffer ? KILN_C99_BUILTIN_BUFFER
                                                    : kiln_c99_typed_constructor(view->kind));
            return KILN_C99_OK;
        }
        if (view->is_buffer && kiln_c99_same_name_key(key, "toString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_BUFFER_TO_STRING);
            return KILN_C99_OK;
        }
        {
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                                &intrinsic_len);
            /* One table for the prototype's own members, shared with `in` and with the
             * reflective paths, so a read and a presence test can never disagree about which
             * names a view has. `@@iterator` is `values`, which is what makes `[...view]` and
             * `for (const x of view)` walk the live view rather than a copy. */
            uint64_t method = kiln_c99_same_name_key(key, "@@iterator")
                                  ? KILN_C99_METHOD_ARRAY_VALUES
                                  : kiln_c99_typed_prototype_method(intrinsic_key, intrinsic_len);
            if (method == 0) {
                kiln_value concrete_proto =
                    kiln_c99_builtin(kiln_c99_typed_prototype(view->kind));
                kiln_value store;
                if (kiln_c99_callable_store(concrete_proto, &store, 0) == KILN_C99_OK &&
                    store.tag == KILN_VALUE_OBJECT) {
                    kiln_object *properties = kiln_c99_object_of(store);
                    size_t pidx;
                    if (properties->shape != NULL) kiln_c99_ensure_dictionary(properties);
                    for (pidx = 0; pidx < properties->count; ++pidx) {
                        if (kiln_c99_same_key_view(&properties->properties[pidx], key)) {
                            return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
                        }
                    }
                }
                kiln_value common_proto =
                    kiln_c99_builtin(KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE);
                if (kiln_c99_callable_store(common_proto, &store, 0) == KILN_C99_OK &&
                    store.tag == KILN_VALUE_OBJECT) {
                    kiln_object *properties = kiln_c99_object_of(store);
                    size_t pidx;
                    if (properties->shape != NULL) kiln_c99_ensure_dictionary(properties);
                    for (pidx = 0; pidx < properties->count; ++pidx) {
                        if (kiln_c99_same_key_view(&properties->properties[pidx], key)) {
                            return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
                        }
                    }
                }
                if (kiln_c99_is_typed_array_member(view->kind == KILN_TYPED_UINT8, intrinsic_key, intrinsic_len)) {
                    /* A real member this backend has not grown yet. `undefined` would be a
                     * silent wrong answer -- the program would call it and get a TypeError
                     * naming the wrong thing -- so this refuses and joins the backlog. */
                    KILN_C99_REFUSE("a typed-array member this backend has not implemented");
                }
                method = kiln_c99_same_name_key(key, "isPrototypeOf")
                             ? KILN_C99_METHOD_IS_PROTOTYPE_OF
                         : kiln_c99_same_name_key(key, "propertyIsEnumerable")
                             ? KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE
                             : kiln_c99_lookup_method(intrinsic_key, intrinsic_len,
                                                      KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                                      KILN_C99_METHOD_OBJECT_VALUE_OF);
            }
            *out = method == 0 ? kiln_c99_undefined() : kiln_c99_method(method);
        }
        return KILN_C99_OK;
    }
    case KILN_VALUE_ARRAY: {
        uint64_t code;
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "length")) {
            *out = kiln_c99_number((double)kiln_c99_array_of(target)->count);
            return KILN_C99_OK;
        }
        /* `for...of` asks for this and then drives `next`, where spread walks the array
         * directly. Both have to work, and they are different code paths, so an array needs the
         * protocol spelled out even though nothing else about it is unusual. */
        if (kiln_c99_same_name_key(key, "@@iterator") ||
            kiln_c99_same_name_key(key, "values")) {
            *out = kiln_c99_method(KILN_C99_METHOD_ARRAY_VALUES);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "@@unscopables")) {
            return kiln_c99_get_array_unscopables(out);
        }
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        code = kiln_c99_array_method(intrinsic_key, intrinsic_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        code = kiln_c99_same_name_key(key, "isPrototypeOf")
                   ? KILN_C99_METHOD_IS_PROTOTYPE_OF
               : kiln_c99_same_name_key(key, "propertyIsEnumerable")
                   ? KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE
                   : kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                            KILN_C99_METHOD_OBJECT_VALUE_OF);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        {
            /* A property that is not an element: `a.total`, the `index` a match result carries,
             * or a method of the class an array subclass was built from -- the store's chain is
             * where `class Stack extends Array {}` hangs `Stack.prototype`. Only a key the chain
             * actually holds is answered, so a *missing* name is still the error below rather
             * than undefined. */
            int found;
            int status = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown,
                                            &found);
            if (status != KILN_C99_OK || found) {
                return status;
            }
        }
        {
            kiln_array *arr = kiln_c99_array_of(target);
            if (arr->match_index >= 0) {
                if (kiln_c99_same_name_key(key, "index")) {
                    *out = kiln_c99_number((double)arr->match_index);
                    return KILN_C99_OK;
                }
                if (kiln_c99_same_name_key(key, "input")) {
                    *out = arr->match_input;
                    return KILN_C99_OK;
                }
                if (kiln_c99_same_name_key(key, "groups")) {
                    int found;
                    kiln_c99_materialize_match_properties(arr);
                    return kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown,
                                              &found);
                }
            }
        }
        if (kiln_c99_same_name_key(key, "toLocaleString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING);
            return KILN_C99_OK;
        }
        /* A method this backend does not have.  Answering undefined would turn `a.sort(f)` into
         * "undefined is not a function" three frames from the cause, so it says so here -- but
         * only for a name the language actually puts on `Array.prototype`. Anything else is
         * absent, and absent is undefined. */
        if (!kiln_c99_is_array_member(intrinsic_key, intrinsic_len)) {
            KILN_C99_ABSENT("not a member of Array.prototype; kiln_c99_is_array_member holds the "
                            "ones that are",
                            out);
        }
        KILN_C99_REFUSE("an Array.prototype member this backend has not implemented");
    }
    case KILN_VALUE_BOOLEAN: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                            &intrinsic_len);
        uint64_t code = kiln_c99_namespace_method(KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE, intrinsic_key,
                                                  intrinsic_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_BOOLEAN);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "toLocaleString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_PRIMITIVE_TO_STRING);
            return KILN_C99_OK;
        }
    }
        /* `GetV` boxes a primitive and then performs an ordinary prototype lookup. A name the
         * wrapper prototype does not contain is absent, not a backend refusal. Keeping that
         * distinction here makes named and computed access share one semantic seam and lets
         * optional chaining compose with arbitrary primitive-valued fields. */
        KILN_C99_ABSENT("not a member of the wrapper prototype this primitive boxes to", out);
    case KILN_VALUE_STRING: {
        uint64_t code;
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_STRING);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "toString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_PRIMITIVE_TO_STRING);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "valueOf")) {
            *out = kiln_c99_method(KILN_C99_METHOD_OBJECT_VALUE_OF);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "length")) {
            *out = kiln_c99_number((double)kiln_c99_string_length(target));
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "@@iterator")) {
            *out = kiln_c99_method(KILN_C99_METHOD_ARRAY_VALUES);
            return KILN_C99_OK;
        }
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        code = kiln_c99_string_method(intrinsic_key, intrinsic_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        if (kiln_c99_is_string_member(intrinsic_key, intrinsic_len)) {
            KILN_C99_REFUSE("a String.prototype member this backend has not implemented");
        }
        /* Not an index, not `length`, not a member: the property is absent, and absent has an
         * answer. */
        KILN_C99_ABSENT("not a member of String.prototype; kiln_c99_is_string_member holds the "
                        "ones that are",
                        out);
    }
    case KILN_VALUE_PROMISE: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        {
            int found;
            int status = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown,
                                            &found);
            if (status != KILN_C99_OK || found) {
                return status;
            }
        }
        uint64_t code = kiln_c99_namespace_method(KILN_C99_BUILTIN_PROMISE_PROTOTYPE, intrinsic_key,
                                                  intrinsic_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE);
            return KILN_C99_OK;
        }
        {
            kiln_value member;
            if (kiln_c99_surface_value(KILN_C99_BUILTIN_PROMISE_PROTOTYPE, intrinsic_key, intrinsic_len, &member)) {
                *out = member;
                return KILN_C99_OK;
            }
        }
        KILN_C99_ABSENT("not a member of Promise.prototype; Promise only defines then, catch, "
                        "finally, constructor, and @@toStringTag",
                        out);
    }
    case KILN_VALUE_COLLECTION: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        kiln_collection *collection = kiln_c99_collection_of(target);
        uint64_t code;
        /* A weak collection has no `size` at all, so its name falls through to the absent
         * answer below rather than stopping the program. */
        if (!collection->is_weak && kiln_c99_same_name_key(key, "size")) {
            *out = kiln_c99_number((double)collection->live);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            /* Which of the four this is, spelled the way every other exotic kind spells it. */
            *out = kiln_c99_builtin(collection->is_weak
                                        ? (collection->is_map ? KILN_C99_BUILTIN_WEAK_MAP
                                                              : KILN_C99_BUILTIN_WEAK_SET)
                                        : (collection->is_map ? KILN_C99_BUILTIN_MAP
                                                              : KILN_C99_BUILTIN_SET));
            return KILN_C99_OK;
        }
        code = kiln_c99_lookup_method(
            intrinsic_key, intrinsic_len, KILN_C99_METHOD_COLLECTION_GET,
            collection->is_weak ? KILN_C99_METHOD_COLLECTION_DELETE
                                : KILN_C99_METHOD_COLLECTION_SELF);
        /* A Set has no `get` or `set`, and a Map has no `add`: offering one would let a program
         * call something that has no meaning for the receiver it was reached through. */
        if (code == 0 ||
            (collection->is_map && code == KILN_C99_METHOD_COLLECTION_ADD) ||
            (!collection->is_map && (code == KILN_C99_METHOD_COLLECTION_GET ||
                                     code == KILN_C99_METHOD_COLLECTION_SET))) {
            /* Not one of the collection's own members, so it is whatever the class a subclass
             * declared put on the prototype -- and only then the refusal that names a member
             * this backend has not grown. */
            int found;
            int status = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown,
                                            &found);
            if (status != KILN_C99_OK || found) {
                return status;
            }
            /* `Object.prototype` is above every collection's prototype, and its members have to
             * be resolved here rather than fall to the absent answer below: turning
             * `m.hasOwnProperty` into undefined would replace a refusal with a wrong answer,
             * which is the trade this whole path exists to avoid. */
            code = kiln_c99_same_name_key(key, "isPrototypeOf")
                       ? KILN_C99_METHOD_IS_PROTOTYPE_OF
                   : kiln_c99_same_name_key(key, "propertyIsEnumerable")
                       ? KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE
                       : kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                                KILN_C99_METHOD_OBJECT_VALUE_OF);
            if (code != 0) {
                *out = kiln_c99_method(code);
                return KILN_C99_OK;
            }
            /* A method this backend does not have.  Answering undefined would turn
             * `m.forEach(f)` into "undefined is not a function" three frames from the cause, so
             * it says so here -- but only for a name the language actually puts on this
             * collection's prototype. Anything else is absent, and absent is undefined. */
            if (!collection->is_weak) {
                int in_store;
                status = kiln_c99_prototype_store_read(
                    host,
                    collection->is_map ? KILN_C99_BUILTIN_MAP_PROTOTYPE : KILN_C99_BUILTIN_SET_PROTOTYPE,
                    key, ikey, out, depth, thrown, &in_store);
                if (status != KILN_C99_OK || in_store) return status;
            }
            if (!kiln_c99_is_collection_member(collection, intrinsic_key, intrinsic_len)) {
                KILN_C99_ABSENT("not a member of this collection's prototype; "
                                "kiln_c99_is_collection_member holds the ones that are",
                                out);
            }
            KILN_C99_REFUSE("a collection prototype member this backend has not implemented");
        }
        *out = kiln_c99_method(code);
        return KILN_C99_OK;
    }
    case KILN_VALUE_DATE: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        static const struct {
            const char *name;
            uint64_t code;
        } ALIASES[] = {
            {"valueOf", KILN_C99_METHOD_GET_TIME},
            {"getUTCFullYear", KILN_C99_METHOD_GET_FULL_YEAR},
            {"getUTCMonth", KILN_C99_METHOD_GET_MONTH},
            {"getUTCDate", KILN_C99_METHOD_GET_DATE},
            {"getUTCDay", KILN_C99_METHOD_GET_DAY},
            {"getUTCHours", KILN_C99_METHOD_GET_HOURS},
            {"getUTCMinutes", KILN_C99_METHOD_GET_MINUTES},
            {"getUTCSeconds", KILN_C99_METHOD_GET_SECONDS},
            {"getUTCMilliseconds", KILN_C99_METHOD_GET_MILLISECONDS},
            /* `toUTCString`, `toDateString` and `toTimeString` were aliased to `toString` here
             * and answered its whole spelling for all three -- `Sat Sep 19 2026 00:00:00 GMT+0000
             * (Coordinated Universal Time)` where node says `Sat, 19 Sep 2026 00:00:00 GMT` -- a
             * plausible wrong string. They are answered from the prototype's store now, where the
             * runtime bootstrap defines them. The three `toLocale*` spellings were aliased the
             * same way and now refuse: their answer needs locale data, which is `Intl`'s to link
             * (ADR 0063 rule 5). */
            /* The local writers are the UTC writers, for the same reason the local readers are
             * the UTC readers: the realm has one zone and its offset is zero. */
            {"setFullYear", KILN_C99_METHOD_SET_FULL_YEAR},
            {"setMonth", KILN_C99_METHOD_SET_MONTH},
            {"setDate", KILN_C99_METHOD_SET_DATE},
            {"setHours", KILN_C99_METHOD_SET_HOURS},
            {"setMinutes", KILN_C99_METHOD_SET_MINUTES},
            {"setSeconds", KILN_C99_METHOD_SET_SECONDS},
            {"setMilliseconds", KILN_C99_METHOD_SET_MILLISECONDS},
        };
        uint64_t code = kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_GET_TIME,
                                               KILN_C99_METHOD_DATE_TO_STRING);
        size_t alias;
        if (code == 0) {
            code = kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_SET_TIME,
                                          KILN_C99_METHOD_SET_MILLISECONDS);
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_DATE);
            return KILN_C99_OK;
        }
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        for (alias = 0; alias < sizeof ALIASES / sizeof *ALIASES; ++alias) {
            if (kiln_c99_same_name_key(key, ALIASES[alias].name)) {
                *out = kiln_c99_method(ALIASES[alias].code);
                return KILN_C99_OK;
            }
        }
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_DATE_PROTOTYPE, key,
                                                       ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
        }
        /* Same split as the array above: a member this backend lacks refuses by name, and a name
         * that is not a member at all is undefined. */
        if (!kiln_c99_is_date_member(intrinsic_key, intrinsic_len)) {
            KILN_C99_ABSENT("not a member of Date.prototype; kiln_c99_is_date_member holds the "
                            "ones that are",
                            out);
        }
        KILN_C99_REFUSE("a Date.prototype member this backend has not implemented");
    }
    case KILN_VALUE_GENERATOR: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        uint64_t code;
        if (kiln_c99_same_name_key(key, "@@asyncIterator")) {
            /* Only an async generator has one, and answering for a sync one would make
             * `for await` take the wrong loop. */
            if (!kiln_c99_generator_of(target)->async_generator) {
                KILN_C99_REFUSE("@@asyncIterator on a synchronous generator");
            }
            *out = kiln_c99_method(KILN_C99_METHOD_GENERATOR_SELF);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "throw")) {
            *out = kiln_c99_method(KILN_C99_METHOD_GENERATOR_THROW);
            return KILN_C99_OK;
        }
        code = kiln_c99_lookup_method(intrinsic_key, intrinsic_len, KILN_C99_METHOD_GENERATOR_NEXT,
                                      KILN_C99_METHOD_GENERATOR_SELF);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        KILN_C99_ABSENT("not a member of Generator.prototype; Generator only defines next, "
                        "return, throw, and @@iterator",
                        out);
    }
    case KILN_VALUE_BOUND:
    case KILN_VALUE_COMMONJS_REQUIRE:
    case KILN_VALUE_METHOD:
    case KILN_VALUE_SETTLER: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        uint64_t callable;
        kiln_value store;
        int stored;
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION);
            return KILN_C99_OK;
        }
        callable = kiln_c99_callable_method(intrinsic_key, intrinsic_len);
        if (callable != 0) {
            *out = kiln_c99_method(callable);
            return KILN_C99_OK;
        }
        /* ...and the `Object.prototype` members a program borrows, which the builtin path below
         * already answers and this one never did.
         *
         * `valueOf` is here and `toString` is not, which is the whole of the distinction once
         * written as "toString and valueOf would shadow what a constructor answers". Only
         * `toString` would: `Object.prototype.toString` gives `[object Function]`, which is worse
         * than refusing, and a callable answers that name for itself through
         * `kiln_c99_callable_method`. `Object.prototype.valueOf` hands back the receiver, so it
         * shadows nothing -- `ToPrimitive` gets a non-primitive from it and falls through to
         * `toString` exactly as it does for a plain object. */
        {
            uint64_t borrowed =
                kiln_c99_same_name_key(key, "hasOwnProperty") ? KILN_C99_METHOD_HAS_OWN_PROPERTY
                : kiln_c99_same_name_key(key, "isPrototypeOf") ? KILN_C99_METHOD_IS_PROTOTYPE_OF
                : kiln_c99_same_name_key(key, "propertyIsEnumerable")
                    ? KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE
                : kiln_c99_same_name_key(key, "valueOf") ? KILN_C99_METHOD_OBJECT_VALUE_OF
                    : 0;
            if (borrowed != 0) {
                *out = kiln_c99_method(borrowed);
                return KILN_C99_OK;
            }
        }
        if (target.tag == KILN_VALUE_BOUND) {
            /* `bind` reports the arity still to be supplied and prefixes the name it wrapped,
             * which is what makes a stack trace through one readable. Both are derived from the
             * target rather than stored, so binding a bound function composes. */
            kiln_bound *bound = (kiln_bound *)(uintptr_t)target.bits;
            stored = kiln_c99_callable_store(target, &store, 0);
            if (stored != KILN_C99_OK) return stored;
            if (store.tag == KILN_VALUE_OBJECT) {
                kiln_object *properties = kiln_c99_object_of(store);
                size_t index;
                if (properties->shape != NULL) {
                    kiln_c99_ensure_dictionary(properties);
                }
                for (index = 0; index < properties->count; ++index) {
                    if (kiln_c99_same_key_view(&properties->properties[index], key))
                        return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
                }
            }
            if (kiln_c99_same_name_key(key, "name")) {
                kiln_value inner;
                int status = kiln_c99_get(host, bound->target, "name", 4, &inner, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                *out = kiln_c99_concat(kiln_c99_copy_text("bound ", 6), inner);
                return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_same_name_key(key, "length")) {
                kiln_value inner;
                double remaining;
                int status = kiln_c99_get(host, bound->target, "length", 6, &inner, depth,
                                          thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                remaining = kiln_c99_as_number(inner) - (double)bound->count;
                *out = kiln_c99_number(remaining > 0.0 ? remaining : 0.0);
                return KILN_C99_OK;
            }
            KILN_C99_REFUSE("a member of a bound function other than name and length");
        }
        if (target.tag == KILN_VALUE_METHOD &&
            target.bits == KILN_C99_METHOD_PROCESS_HRTIME &&
            kiln_c99_same_name_key(key, "bigint")) {
            *out = kiln_c99_method(KILN_C99_METHOD_PROCESS_HRTIME_BIGINT);
            return KILN_C99_OK;
        }
        if (target.tag == KILN_VALUE_METHOD &&
            target.bits == KILN_C99_METHOD_NODE_EVENT_EMITTER &&
            (kiln_c99_same_name_key(key, "EventEmitter") ||
             kiln_c99_same_name_key(key, "default"))) {
            *out = target;
            return KILN_C99_OK;
        }
        if (target.tag == KILN_VALUE_METHOD &&
            target.bits == KILN_C99_METHOD_NODE_EVENT_EMITTER &&
            kiln_c99_same_name_key(key, "listenerCount")) {
            *out = kiln_c99_method(KILN_C99_METHOD_NODE_EVENT_LISTENER_COUNT);
            return KILN_C99_OK;
        }
        if (target.tag == KILN_VALUE_METHOD && kiln_c99_same_name_key(key, "prototype")) {
            return kiln_c99_callable_prototype(target, out);
        }
        stored = kiln_c99_callable_store(target, &store, 0);
        if (stored != KILN_C99_OK) return stored;
        if (store.tag == KILN_VALUE_OBJECT) {
            kiln_object *properties = kiln_c99_object_of(store);
            size_t index;
            if (properties->shape != NULL) {
                kiln_c99_ensure_dictionary(properties);
            }
            for (index = 0; index < properties->count; ++index) {
                if (kiln_c99_same_key_view(&properties->properties[index], key)) {
                    return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
                }
            }
        }
        /* `parseInt.name`. A method has no other property worth answering, and a wrong answer
         * here reads as a missing function three frames later. */
        if (kiln_c99_same_name_key(key, "name")) {
            const char *method = kiln_c99_method_name(target.bits);
            *out = method == NULL ? kiln_c99_from_units(NULL, 0)
                                  : kiln_c99_copy_text(method, strlen(method));
            return KILN_C99_OK;
        }
        KILN_C99_REFUSE("a member of a native method other than name");
    }
    case KILN_VALUE_BUILTIN: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        kiln_value store;
        int stored = kiln_c99_callable_store(target, &store, 0);
        if (stored != KILN_C99_OK) return stored;
        if (store.tag == KILN_VALUE_OBJECT) {
            kiln_object *properties = kiln_c99_object_of(store);
            size_t index;
            if (properties->shape != NULL) {
                kiln_c99_ensure_dictionary(properties);
            }
            for (index = 0; index < properties->count; ++index) {
                if (kiln_c99_same_key_view(&properties->properties[index], key))
                    return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
            }
        }
        /* `Temporal` first, because the namespace tables below are keyed by method name and every
         * name here is a *value*: `Temporal.PlainDate` is a constructor, not a function to call. */
#if KILN_RETAIN_GROUP_TEMPORAL
        if (target.bits == KILN_C99_BUILTIN_TEMPORAL) {
            static const struct {
                const char *name;
                uint64_t builtin;
            } types[] = {{"PlainDate", KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE},
                         {"PlainTime", KILN_C99_BUILTIN_TEMPORAL_PLAIN_TIME},
                         {"PlainDateTime", KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE_TIME},
                         {"PlainYearMonth", KILN_C99_BUILTIN_TEMPORAL_PLAIN_YEAR_MONTH},
                         {"PlainMonthDay", KILN_C99_BUILTIN_TEMPORAL_PLAIN_MONTH_DAY},
                         {"Duration", KILN_C99_BUILTIN_TEMPORAL_DURATION},
                         {"Instant", KILN_C99_BUILTIN_TEMPORAL_INSTANT},
                         {"ZonedDateTime", KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME},
                         {"Now", KILN_C99_BUILTIN_TEMPORAL_NOW}};
            size_t index;
            for (index = 0; index < sizeof types / sizeof *types; ++index) {
                if (kiln_c99_same_name_key(key, types[index].name)) {
                    *out = kiln_c99_builtin(types[index].builtin);
                    return KILN_C99_OK;
                }
            }
            if (kiln_c99_same_name_key(key, "@@toStringTag")) {
                *out = kiln_c99_copy_text("Temporal", 8);
                return KILN_C99_OK;
            }
            /* A name the namespace does not have is `undefined`, as it is on any object -- the
             * refusal below is for the ones that *do* exist and are not wired yet. */
            KILN_C99_ABSENT("not a member of the Temporal namespace", out);
        }
        if (target.bits == KILN_C99_BUILTIN_TEMPORAL_NOW) {
            /* `Temporal.Now` reads the realm's clock, which ADR 0015 makes virtual, UTC, and
             * started at the epoch. Every one of these is a function the program calls rather than
             * a value it reads, and the kind rides in the code. */
            static const struct {
                const char *name;
                uint64_t kind;
            } readers[] = {{"instant", 6},
                           {"plainDateISO", 0},
                           {"plainTimeISO", 1},
                           {"plainDateTimeISO", 2},
                           {"zonedDateTimeISO", 7}};
            size_t index;
            if (kiln_c99_same_name_key(key, "timeZoneId")) {
                *out = kiln_c99_method(KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID);
                return KILN_C99_OK;
            }
            for (index = 0; index < sizeof readers / sizeof *readers; ++index) {
                if (kiln_c99_same_name_key(key, readers[index].name)) {
                    *out = kiln_c99_method(KILN_C99_METHOD_TEMPORAL_NOW_READ +
                                           readers[index].kind);
                    return KILN_C99_OK;
                }
            }
            KILN_C99_ABSENT("not a member of Temporal.Now", out);
        }
        {
            int32_t constructed = kiln_c99_temporal_kind_of_builtin(target.bits);
            if (constructed >= 0) {
                if (kiln_c99_same_name_key(key, "from")) {
                    *out = kiln_c99_method(KILN_C99_METHOD_TEMPORAL_FROM +
                                           (uint64_t)constructed);
                    return KILN_C99_OK;
                }
                if (kiln_c99_same_name_key(key, "compare")) {
                    *out = kiln_c99_method(KILN_C99_METHOD_TEMPORAL_COMPARE +
                                           (uint64_t)constructed);
                    return KILN_C99_OK;
                }
                if (kiln_c99_same_name_key(key, "name")) {
                    const char *name = kiln_c99_temporal_builtin_name(target.bits);
                    /* The bare type name, as `Temporal.PlainDate.name` answers -- not the dotted
                     * spelling the tag uses. */
                    const char *bare = name == NULL ? NULL : strchr(name, '.');
                    bare = bare == NULL ? name : bare + 1;
                    *out = bare == NULL ? kiln_c99_undefined()
                                        : kiln_c99_copy_text(bare, strlen(bare));
                    return KILN_C99_OK;
                }
                /* `Temporal.Instant` alone has the two epoch constructors: an epoch count names a
                 * point on the timeline, and no other of the eight *is* a point on a timeline. On
                 * the other seven these names do not exist in the language at all, so they read as
                 * undefined below rather than saying which. */
                if (constructed == 6) {
                    if (kiln_c99_same_name_key(key, "fromEpochMilliseconds")) {
                        *out = kiln_c99_method(
                            KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS);
                        return KILN_C99_OK;
                    }
                    if (kiln_c99_same_name_key(key, "fromEpochNanoseconds")) {
                        *out = kiln_c99_method(
                            KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS);
                        return KILN_C99_OK;
                    }
                }
                /* `prototype` exists in the language and is not here, so it says which rather
                 * than reading as undefined.
                 *
                 * It stays a refusal on purpose rather than for want of effort, but ONE OF THE TWO
                 * REASONS IT USED TO CARRY IS GONE. The interpreter answers a
                 * `Native::Temporal(Op::Prototype(kind))`, and that value now reports `typeof` as
                 * "object" and links to `Object.prototype`, as the specification and `deno` say --
                 * so an instance's chain is real and `d instanceof Temporal.PlainDate` is true on
                 * that side. What is still missing is the property store: the method table carries
                 * `constructor`, the tag and the methods, and cannot hold a name a program writes.
                 * Until it can, answering here would diverge from the interpreter on the first
                 * program that writes one, and reproducing the interpreter's shape would make the
                 * two engines agree on an incomplete object. The move is still the interpreter's,
                 * so this names the missing member and waits for it. */
                if (kiln_c99_same_name_key(key, "prototype")) {
                    const char *owner = kiln_c99_temporal_builtin_name(target.bits);
                    if (intrinsic_key == NULL) KILN_C99_REFUSE("a Temporal member with a non-ASCII key");
                    return kiln_c99_temporal_unsupported(thrown, owner == NULL ? "Temporal" : owner,
                                                         intrinsic_key, intrinsic_len);
                }
                KILN_C99_ABSENT("not a static of this Temporal type", out);
            }
        }
#endif
        {
            kiln_typed_kind typed_kind;
            int concrete = kiln_c99_typed_prototype_kind(target.bits, &typed_kind);
            if (concrete || target.bits == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE) {
                uint64_t method;
                if (concrete && kiln_c99_same_name_key(key, "constructor")) {
                    *out = kiln_c99_builtin(kiln_c99_typed_constructor(typed_kind));
                    return KILN_C99_OK;
                }
                if (concrete && kiln_c99_same_name_key(key, "BYTES_PER_ELEMENT")) {
                    *out = kiln_c99_number((double)kiln_c99_typed_width(typed_kind));
                    return KILN_C99_OK;
                }
                method = kiln_c99_typed_prototype_method(intrinsic_key, intrinsic_len);
                if (method == 0 && concrete) {
                    kiln_value common_proto =
                        kiln_c99_builtin(KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE);
                    kiln_value store;
                    if (kiln_c99_callable_store(common_proto, &store, 0) == KILN_C99_OK &&
                        store.tag == KILN_VALUE_OBJECT) {
                        kiln_object *properties = kiln_c99_object_of(store);
                        size_t pidx;
                        if (properties->shape != NULL) kiln_c99_ensure_dictionary(properties);
                        for (pidx = 0; pidx < properties->count; ++pidx) {
                            if (kiln_c99_same_key_view(&properties->properties[pidx], key)) {
                                return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
                            }
                        }
                    }
                }
                if (method == 0) {
                    /* The same question the *instance* read asks two hundred lines above, and it
                     * was missing here: a real member of the prototype that this backend has not
                     * built read as `undefined` off the prototype object while refusing off an
                     * instance. `Uint8Array.prototype.toSorted` is one, and a library that patches
                     * or feature-detects a method reads it off the prototype, which is the half
                     * that was answering wrongly. */
                    if (kiln_c99_is_typed_array_member(concrete && typed_kind == KILN_TYPED_UINT8,
                                                       intrinsic_key, intrinsic_len)) {
                        KILN_C99_REFUSE("a typed-array member this backend has not implemented");
                    }
                    method = kiln_c99_same_name_key(key, "isPrototypeOf")
                                 ? KILN_C99_METHOD_IS_PROTOTYPE_OF
                             : kiln_c99_same_name_key(key, "propertyIsEnumerable")
                                 ? KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE
                                 : kiln_c99_lookup_method(intrinsic_key, intrinsic_len,
                                                          KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                                          KILN_C99_METHOD_OBJECT_VALUE_OF);
                }
                *out = method == 0 ? kiln_c99_undefined() : kiln_c99_method(method);
                return KILN_C99_OK;
            }
        }
        uint64_t code = kiln_c99_namespace_method(target.bits, intrinsic_key, intrinsic_len);
        if (code == 0 && !kiln_c99_is_namespace(target.bits)) {
            /* A constructor is a function, so it answers to these too. A namespace is not. */
            code = kiln_c99_callable_method(intrinsic_key, intrinsic_len);
        }
        if (code == 0) {
            /* ...and everything inherits from `Object.prototype`, which is where
             * `Object.hasOwnProperty.call(x, k)` finds its function. Only the three a program
             * borrows. `toString` is the one still withheld here: `Object.prototype.toString`
             * would answer `[object Function]` for `String(Object)`, which is worse than not
             * answering at all, and a constructor already gives its own text through
             * `kiln_c99_callable_method`. `valueOf` hands back the receiver and shadows nothing. */
            code = kiln_c99_same_name_key(key, "hasOwnProperty") ? KILN_C99_METHOD_HAS_OWN_PROPERTY
                   : kiln_c99_same_name_key(key, "isPrototypeOf") ? KILN_C99_METHOD_IS_PROTOTYPE_OF
                   : kiln_c99_same_name_key(key, "propertyIsEnumerable")
                       ? KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE
                   : kiln_c99_same_name_key(key, "valueOf") ? KILN_C99_METHOD_OBJECT_VALUE_OF
                       : 0;
        }
        /* `RangeError.name` is how `error.constructor.name` gets its answer, which is the ordinary
         * way a program asks what kind of error it caught. A namespace is not a constructor and
         * has no name to give. */
        if (kiln_c99_same_name_key(key, "constructor")) {
            /* A prototype table names its own constructor, which is how `[].constructor` and
             * `Array.prototype.constructor` reach the same value. */
            static const struct {
                uint64_t table;
                uint64_t constructor;
            } owners[] = {{KILN_C99_BUILTIN_OBJECT_PROTOTYPE, KILN_C99_BUILTIN_OBJECT},
                          {KILN_C99_BUILTIN_ARRAY_PROTOTYPE, KILN_C99_BUILTIN_ARRAY},
                          {KILN_C99_BUILTIN_ERROR_PROTOTYPE, KILN_C99_BUILTIN_ERROR},
                          {KILN_C99_BUILTIN_AGGREGATE_ERROR_PROTOTYPE,
                           KILN_C99_BUILTIN_AGGREGATE_ERROR},
                          {KILN_C99_BUILTIN_MAP_PROTOTYPE, KILN_C99_BUILTIN_MAP},
                          {KILN_C99_BUILTIN_SET_PROTOTYPE, KILN_C99_BUILTIN_SET},
                          {KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE, KILN_C99_BUILTIN_WEAK_MAP},
                          {KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE, KILN_C99_BUILTIN_WEAK_SET},
                          {KILN_C99_BUILTIN_STRING_PROTOTYPE, KILN_C99_BUILTIN_STRING},
                          {KILN_C99_BUILTIN_ITERATOR_PROTOTYPE, KILN_C99_BUILTIN_ITERATOR},
                          {KILN_C99_BUILTIN_REGEXP_PROTOTYPE, KILN_C99_BUILTIN_REGEXP},
                          {KILN_C99_BUILTIN_DATE_PROTOTYPE, KILN_C99_BUILTIN_DATE},
                          {KILN_C99_BUILTIN_NUMBER_PROTOTYPE, KILN_C99_BUILTIN_NUMBER},
                          {KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE, KILN_C99_BUILTIN_BOOLEAN},
                          {KILN_C99_BUILTIN_BIGINT_PROTOTYPE, KILN_C99_BUILTIN_BIGINT},
                          {KILN_C99_BUILTIN_FUNCTION_PROTOTYPE, KILN_C99_BUILTIN_FUNCTION},
                          {KILN_C99_BUILTIN_PROMISE_PROTOTYPE, KILN_C99_BUILTIN_PROMISE},
                          {KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE, KILN_C99_BUILTIN_DATA_VIEW},
                          {KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE, KILN_C99_BUILTIN_ARRAY_BUFFER}};
            size_t owner;
            for (owner = 0; owner < sizeof owners / sizeof *owners; ++owner) {
                if (target.bits == owners[owner].table) {
                    *out = kiln_c99_builtin(owners[owner].constructor);
                    return KILN_C99_OK;
                }
            }
            /* Anything else answers for what it *is*, which the table above could not say because
             * it maps prototypes rather than values. A callable is a function, so
             * `Object.constructor` is `Function` exactly as `f.constructor` is for a function a
             * program wrote; a namespace is an ordinary object holding functions, so
             * `Math.constructor` is `Object`. Refusing had been the answer to both. */
            *out = kiln_c99_builtin(kiln_c99_is_namespace(target.bits) ? KILN_C99_BUILTIN_OBJECT
                                                                      : KILN_C99_BUILTIN_FUNCTION);
            return KILN_C99_OK;
        }
        if (!kiln_c99_is_namespace(target.bits) && kiln_c99_same_name_key(key, "name")) {
            const char *builtin = kiln_c99_builtin_name(target.bits);
            *out = kiln_c99_copy_text(builtin, strlen(builtin));
            return KILN_C99_OK;
        }
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        {
            kiln_typed_kind typed_kind;
            if (kiln_c99_builtin_typed_kind(target.bits, &typed_kind)) {
                if (kiln_c99_same_name_key(key, "BYTES_PER_ELEMENT")) {
                    *out = kiln_c99_number((double)kiln_c99_typed_width(typed_kind));
                    return KILN_C99_OK;
                }
                /* `Uint8Array.of` and `Uint8Array.from` are not `Array`'s with a conversion
                 * afterwards: the constructor decides the width, so which constructor was named
                 * is part of the answer -- and it is the receiver of the call these return. */
                if (kiln_c99_same_name_key(key, "of")) {
                    *out = kiln_c99_method(KILN_C99_METHOD_TYPED_OF);
                    return KILN_C99_OK;
                }
                if (kiln_c99_same_name_key(key, "from")) {
                    *out = kiln_c99_method(KILN_C99_METHOD_TYPED_FROM);
                    return KILN_C99_OK;
                }
            }
        }
        /* `Set`, `SharedArrayBuffer` and the typed-array constructors define `@@species` exactly
         * as `Array` and `Map` do, but none of them is table-driven, so the accessor is answered
         * here: it reports its receiver. A closure inheriting from one of these answers the same
         * way on its own read path above. */
        if (kiln_c99_same_name_key(key, "@@species")) {
            kiln_typed_kind species_kind;
            if (target.bits == KILN_C99_BUILTIN_SET ||
                target.bits == KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER ||
                kiln_c99_builtin_typed_kind(target.bits, &species_kind)) {
                *out = target;
                return KILN_C99_OK;
            }
        }
        /* A member whose answer is a value rather than a call -- `Math.PI`, `Number.MAX_VALUE`,
         * `Symbol.iterator`, `Array[Symbol.species]` -- out of the same rows the methods come
         * from. Three separate hand-written lists stood here, one per namespace, which is how
         * `Number` came to answer five of the interpreter's fifteen statics and how
         * `Array[Symbol.species]` came to answer nothing at all. */
        {
            kiln_value member;
            if (kiln_c99_surface_value(target.bits, intrinsic_key, intrinsic_len, &member)) {
                *out = member;
                return KILN_C99_OK;
            }
        }
        if (kiln_c99_same_name_key(key, "prototype")) {
            /* Reached for its methods rather than as an object: these are not yet the objects the
             * chains actually end at, which is what `Object.getPrototypeOf` would need.
             *
             * A table rather than the chain of fourteen `if`s this was. The chain is the same
             * shape that let the static surface drift -- a list a human extends by remembering to
             * -- and it had already stopped one short in a way nothing caught: `Boolean.prototype`
             * refused, which is where a bundled `lodash` stops. Errors and the typed arrays stay
             * out of the table because each is a family answered from the receiver rather than a
             * pair, and folding them in would mean listing every member of both. */
            static const struct {
                uint64_t builtin;
                uint64_t prototype;
            } prototypes[] = {
                {KILN_C99_BUILTIN_OBJECT, KILN_C99_BUILTIN_OBJECT_PROTOTYPE},
                {KILN_C99_BUILTIN_ARRAY, KILN_C99_BUILTIN_ARRAY_PROTOTYPE},
                {KILN_C99_BUILTIN_MAP, KILN_C99_BUILTIN_MAP_PROTOTYPE},
                {KILN_C99_BUILTIN_SET, KILN_C99_BUILTIN_SET_PROTOTYPE},
                {KILN_C99_BUILTIN_WEAK_MAP, KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE},
                {KILN_C99_BUILTIN_WEAK_SET, KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE},
                {KILN_C99_BUILTIN_STRING, KILN_C99_BUILTIN_STRING_PROTOTYPE},
                /* The read a bundled `mobx` makes first. It is a distinct path from resolving
                 * `it.map`: without the pair here the table exists, every helper behind it works,
                 * and `Iterator.prototype` still refuses. */
                {KILN_C99_BUILTIN_ITERATOR, KILN_C99_BUILTIN_ITERATOR_PROTOTYPE},
                {KILN_C99_BUILTIN_REGEXP, KILN_C99_BUILTIN_REGEXP_PROTOTYPE},
                {KILN_C99_BUILTIN_DATE, KILN_C99_BUILTIN_DATE_PROTOTYPE},
                {KILN_C99_BUILTIN_NUMBER, KILN_C99_BUILTIN_NUMBER_PROTOTYPE},
                {KILN_C99_BUILTIN_BOOLEAN, KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE},
                {KILN_C99_BUILTIN_BIGINT, KILN_C99_BUILTIN_BIGINT_PROTOTYPE},
                {KILN_C99_BUILTIN_FUNCTION, KILN_C99_BUILTIN_FUNCTION_PROTOTYPE},
                {KILN_C99_BUILTIN_PROMISE, KILN_C99_BUILTIN_PROMISE_PROTOTYPE},
                {KILN_C99_BUILTIN_DATA_VIEW, KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE},
                {KILN_C99_BUILTIN_ARRAY_BUFFER, KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE},
                {KILN_C99_BUILTIN_SYMBOL, KILN_C99_BUILTIN_SYMBOL_PROTOTYPE}};
            size_t pair;
            for (pair = 0; pair < sizeof prototypes / sizeof *prototypes; ++pair) {
                if (target.bits == prototypes[pair].builtin) {
                    *out = kiln_c99_builtin(prototypes[pair].prototype);
                    return KILN_C99_OK;
                }
            }
            if (kiln_c99_error_name(target.bits) != NULL) {
                *out = kiln_c99_builtin(target.bits == KILN_C99_BUILTIN_AGGREGATE_ERROR
                                            ? KILN_C99_BUILTIN_AGGREGATE_ERROR_PROTOTYPE
                                            : KILN_C99_BUILTIN_ERROR_PROTOTYPE);
                return KILN_C99_OK;
            }
            {
                kiln_typed_kind typed_kind;
                if (kiln_c99_builtin_typed_kind(target.bits, &typed_kind)) {
                    *out = kiln_c99_builtin(kiln_c99_typed_prototype(typed_kind));
                    return KILN_C99_OK;
                }
            }
        }
        /* THE THREE NAMES THE SHARED SURFACE DELIBERATELY DOES NOT LIST.
         *
         * `length`, `name` and `prototype` belong to every callable rather than to any particular
         * built-in, so `kiln_builtin_surface::SURFACE` leaves them out and each engine answers
         * them from its own knowledge of what a function is (ADR 0043). The consequence here was
         * not intended: the rule below -- "a name this table has no row for is `undefined`" --
         * was answering for them too, so `Array.length`, `ArrayBuffer.prototype` and
         * `Symbol.prototype` read `undefined` against the interpreter's number and objects. That
         * is the silent wrong answer ADR 0042 forbids, and it was invisible to
         * `assert_surface_is_covered` precisely because the three are outside the shared table.
         *
         * `name` is answered above for every non-namespace built-in. `prototype` is answered
         * above for every built-in this backend has a prototype object for. What reaches here is
         * a built-in whose prototype this backend does not model -- `ArrayBuffer` and `Symbol`
         * are the two -- and a `length` this backend has no arity for.
         *
         * WHY THESE REFUSE RATHER THAN ANSWER. A `prototype` here would have to be a new
         * prototype built-in with no method table behind it, which would publish nothing and
         * answer `undefined` for each of its own members -- twelve silent wrong answers on
         * `ArrayBuffer.prototype` and four on `Symbol.prototype`, in place of the one that is
         * here now. `length` is a built-in function's arity, which lives in neither table; a
         * guess is a wrong answer and nothing in this repository feature-detects on it. Both are
         * work with an owner, and until somebody does it a refusal names the member and stops the
         * program at the cause. */
        if (!kiln_c99_is_namespace(target.bits) &&
            kiln_c99_same_name_key(key, "length")) {
            double arity;
            if (kiln_c99_builtin_length(target.bits, &arity)) {
                *out = kiln_c99_number(arity);
                return KILN_C99_OK;
            }
        }
        if (!kiln_c99_is_namespace(target.bits) &&
            (kiln_c99_same_name_key(key, "length") ||
             kiln_c99_same_name_key(key, "prototype"))) {
            KILN_C99_REFUSE("a built-in callable's own `length` or `prototype`, which this "
                            "backend does not model for this built-in");
        }
        /* A constructor inherits from `Function.prototype`, so what the runtime bootstrap defined
         * into that store -- `@@hasInstance` -- is read off `Array` and `Map` too, before the
         * name is called absent. A namespace inherits from `Object.prototype` and has nothing to
         * find here. */
        if (!kiln_c99_is_namespace(target.bits) &&
            target.bits != KILN_C99_BUILTIN_FUNCTION_PROTOTYPE) {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_FUNCTION_PROTOTYPE,
                                                       key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
        }
        {
            /* An absent name on a table-driven built-in is `undefined`, exactly as it is on an
             * object the program built: `Object.nope` is not a gap here, it is what the language
             * answers, and refusing it ended the process where a bundled `mobx` read one.
             *
             * The backlog is what makes that safe. A member the language really does put here and
             * this backend has not built is in it, and still refuses by name -- so `Object.groupBy`
             * says so rather than reading as undefined and quietly failing a feature test. A
             * built-in whose surface is not table-driven has no list to consult, so it keeps
             * refusing; that is the honest answer for a question we cannot yet ask. */
            int owned = 0;
            (void)kiln_c99_builtin_member(target.bits, intrinsic_key, intrinsic_len, &owned);
            if (owned && !kiln_c99_backlog_names(kiln_c99_builtin_owner_name(target.bits), intrinsic_key, intrinsic_len)) {
                KILN_C99_ABSENT("not a member of this built-in; BUILTIN_SURFACE and "
                                "BUILTIN_SURFACE_BACKLOG hold every one that is",
                                out);
            }
            if (target.bits == KILN_C99_BUILTIN_STRING_PROTOTYPE &&
                !kiln_c99_is_string_member(intrinsic_key, intrinsic_len)) {
                KILN_C99_ABSENT("not a member of String.prototype; kiln_c99_is_string_member holds "
                                "the ones that are",
                                out);
            }
            if (target.bits == KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE) {
                if (kiln_c99_same_name_key(key, "slice")) {
                    *out = kiln_c99_method(KILN_C99_METHOD_SLICE);
                    return KILN_C99_OK;
                }
                if (kiln_c99_same_name_key(key, "constructor")) {
                    *out = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER);
                    return KILN_C99_OK;
                }
                if (kiln_c99_same_name_key(key, "byteLength")) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "get byteLength: receiver does not have the required internal slot");
                }
                if (kiln_c99_is_array_buffer_member(intrinsic_key, intrinsic_len)) {
                    KILN_C99_REFUSE("an ArrayBuffer.prototype member this backend has not implemented");
                }
                KILN_C99_ABSENT("not a member of ArrayBuffer.prototype; kiln_c99_is_array_buffer_member holds "
                                "the ones that are",
                                out);
            }
            if (target.bits == KILN_C99_BUILTIN_FUNCTION) {
                KILN_C99_ABSENT("not a member of Function; Function only owns length, name, and prototype",
                                out);
            }
            if (target.bits == KILN_C99_BUILTIN_ERROR_PROTOTYPE) {
                /* `toString` and `constructor` are answered above; `name` and `message` are data
                 * properties the runtime bootstrap defines into this built-in's store, which the
                 * top of this case consults before anything else. Until 2026-09-19 this site
                 * named the four and checked none of them, so `Error.prototype.name` -- and
                 * `.name` on every instance of a `class X extends Error` -- read `undefined`. */
                KILN_C99_ABSENT("not a member of Error.prototype: toString and constructor are "
                                "answered above, name and message from the bootstrap-defined store",
                                out);
            }

        }
        KILN_C99_REFUSE("a member the language puts on this built-in and this backend has not "
                        "built");
    }
    case KILN_VALUE_UNDEFINED:
    case KILN_VALUE_NULL:
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    target.tag == KILN_VALUE_NULL
                                        ? "Cannot read properties of null"
                                        : "Cannot read properties of undefined");
    case KILN_VALUE_BIGINT: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                            &intrinsic_len);
        uint64_t code = kiln_c99_namespace_method(KILN_C99_BUILTIN_BIGINT_PROTOTYPE, intrinsic_key,
                                                  intrinsic_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_BIGINT);
            return KILN_C99_OK;
        }
        {
            kiln_value member;
            if (kiln_c99_surface_value(KILN_C99_BUILTIN_BIGINT_PROTOTYPE, intrinsic_key,
                                       intrinsic_len, &member)) {
                *out = member;
                return KILN_C99_OK;
            }
        }
        KILN_C99_ABSENT("not a member of BigInt.prototype; BUILTIN_SURFACE holds the ones that are",
                        out);
    }
    case KILN_VALUE_NUMBER: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                            &intrinsic_len);
        uint64_t code = kiln_c99_namespace_method(KILN_C99_BUILTIN_NUMBER_PROTOTYPE, intrinsic_key,
                                                  intrinsic_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_NUMBER);
            return KILN_C99_OK;
        }
        KILN_C99_ABSENT("not a member of Number.prototype", out);
    }
    case KILN_VALUE_SYMBOL: {
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_SYMBOL);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "toString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_PRIMITIVE_TO_STRING);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "valueOf")) {
            *out = kiln_c99_method(KILN_C99_METHOD_OBJECT_VALUE_OF);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "description")) {
            kiln_symbol *symbol = kiln_c99_symbol_of(target);
            if (symbol != NULL && symbol->description_value.tag == KILN_VALUE_STRING) {
                *out = symbol->description_value;
            } else if (symbol != NULL && symbol->description != NULL) {
                *out = kiln_c99_copy_text(symbol->description, symbol->description_len);
            } else {
                *out = kiln_c99_undefined();
            }
            return KILN_C99_OK;
        }
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_SYMBOL_PROTOTYPE, key,
                                                       ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
        }
        KILN_C99_ABSENT("not a member of Symbol.prototype", out);
    }
    default:
        /* A boolean or callable has no own properties here. */
        KILN_C99_REFUSE("own properties are not modelled on this value");
    }
}

/* A read, with one rule about the machine's own keys layered over it. This headed form is the
 * semantic implementation; fixed compiler names below are borrowed Latin1 adapters. */
static int kiln_c99_get_key(kiln_host *host, kiln_value target, const kiln_string *key,
                            const kiln_string *ikey, kiln_value *out, int depth,
                            kiln_value *thrown) {
    int status;
    if (key == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (kiln_c99_is_node_hidden_key_key(key)) {
        KILN_C99_ABSENT("a hidden runtime slot, which no program names", out);
    }
    if (target.tag == KILN_VALUE_PROXY) {
        kiln_value key_value = kiln_c99_key_value(key, &ikey);
        if (key_value.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        return kiln_c99_proxy_get(host, target, key_value, target, out, depth, thrown);
    }
    status = kiln_c99_get_own_kind(host, target, key, ikey, out, depth, thrown);
    if (status == KILN_C99_CONTROL_ERROR && kiln_c99_key_is_program_symbol(key)) {
        /* `Boolean[Symbol.for("immer-draftable")]`, which a bundled `immer` reads off every
         * value's constructor. A built-in carries a program's symbol only through the store the
         * read consulted first, so the refusal the tables raised for an unknown `@@` name is
         * answered as the absence it is. A *well-known* symbol is not answered here: the
         * language may put one on this receiver, and refusing is the honest report until the
         * backend wires it. */
        kiln_c99_diagnostic_detail = NULL;
        KILN_C99_ABSENT("a symbol the program made, which a built-in can only carry through the "
                        "store already consulted",
                        out);
    }
    return status;
}

/* The machine's own read of a well-known symbol: `kiln_c99_to_primitive` asks every reference for
 * `@@toPrimitive`, iteration asks for `@@iterator`, `Object.prototype.toString` for
 * `@@toStringTag`. A receiver that does not carry the symbol answers absent, and a receiver that
 * carries it but this backend has not wired refuses -- and a probe the *runtime* makes on its own
 * initiative must not turn that refusal into a stop, because the program named nothing.
 *
 * Until 2026-09-19 that rule lived in `kiln_c99_get_key` and so applied to every read, including
 * the program's own `Map[Symbol.species]` and `RegExp.prototype[Symbol.replace]`: a refusal
 * became `undefined` for whoever asked, and the inventory measured 35 silent wrong answers behind
 * it. The comment on the site said as much -- "THIS IS A CATCH-ALL AND NOT A LIST, WHICH MAKES IT
 * THE ONE ABSENCE HERE THAT CAN LIE" -- and asked for the fix upstream. This is the upstream: the
 * probes come through here, and a program's read of a symbol the backend has not wired refuses
 * by name like any other member. */
static int kiln_c99_probe_symbol(kiln_host *host, kiln_value target, const char *key,
                                 size_t key_len, kiln_value *out, int depth,
                                 kiln_value *thrown) {
    int status = kiln_c99_get_ikey(host, target, key, key_len, NULL, out, depth, thrown);
    if (status == KILN_C99_CONTROL_ERROR) {
        /* Nothing refused, so nothing may be left saying it did: the diagnostic keeps the *first*
         * note for the rest of the run, and a detail left behind here would be reported as the
         * cause of whatever refuses next. */
        kiln_c99_diagnostic_detail = NULL;
        KILN_C99_ABSENT("the machine's own probe for a well-known symbol on a receiver that "
                        "carries it unwired; the program named no member, so nothing stops",
                        out);
    }
    return status;
}

static int kiln_c99_get_ikey(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                             const kiln_string *ikey, kiln_value *out, int depth,
                             kiln_value *thrown) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    return kiln_c99_get_key(host, target, &view, ikey, out, depth, thrown);
}

static int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value *out, int depth, kiln_value *thrown) {
    return kiln_c99_get_ikey(host, target, key, key_len, NULL, out, depth, thrown);
}

/* Remember where `key` sits on `object`, or on its immediate prototype, for next time. A
 * machine-private (`@@`) key has hiding rules and `__proto__` a relinking rule; both stay the
 * slow path's alone, so a site never learns either. */
static int kiln_c99_site_learn(kiln_c99_site *site, const kiln_object *object,
                               const kiln_string *key, const kiln_string *canonical) {
    size_t index;
    if (kiln_c99_key_is_reserved(key) || kiln_c99_same_name_key(key, "__proto__")) {
        return 0;
    }
    if (object->shape != NULL && canonical != NULL) {
        uint32_t slot;
        if (kiln_c99_shape_find_slot(object->shape, canonical, &slot)) {
            uint32_t p;
            for (p = 0; p < site->pic_count; ++p) {
                if (site->entries[p].shape == object->shape) {
                    site->entries[p].slot = slot;
                    site->entries[p].transition_to = NULL;
                    return 1;
                }
            }
            if (site->pic_count < KILN_C99_PIC_SIZE) {
                site->entries[site->pic_count].shape = object->shape;
                site->entries[site->pic_count].transition_to = NULL;
                site->entries[site->pic_count].slot = slot;
                site->pic_count += 1;
                return 1;
            }
            return 1;
        }
    }
    for (index = 0; index < object->count && object->properties != NULL; ++index) {
        if (kiln_c99_same_key_view(&object->properties[index], key)) {
            site->own = kiln_c99_site_encode_dictionary_index(index);
            site->proto = 0;
            return 1;
        }
    }
    if (object->prototype.tag == KILN_VALUE_OBJECT) {
        const kiln_object *parent = kiln_c99_object_of(object->prototype);
        for (index = 0; index < parent->count && parent->properties != NULL; ++index) {
            if (kiln_c99_same_key_view(&parent->properties[index], key)) {
                site->own = 0;
                site->proto = kiln_c99_site_encode_dictionary_index(index);
                return 1;
            }
        }
    }
    return 0;
}

/* `kiln_c99_get` with a site. `key` is the unit's canonical pointer for the name.
 *
 * A hit answers exactly what the slow path would. The own hit is the property the slow path's
 * first scan finds, because a key is on an object at most once. The prototype hit is taken only
 * after the receiver's own list has been scanned and missed -- by pointer, no bytes -- and only
 * when the immediate prototype is an ordinary object, which is precisely the slow path's second
 * step. An accessor is never answered here: reading one is a call. */
static int kiln_c99_get_site(kiln_host *host, kiln_value target, const kiln_string *key,
                             const kiln_string *canonical, kiln_value *out, int depth,
                             kiln_value *thrown, kiln_c99_site *site) {
    int status;
    if (key == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (site == NULL || target.tag != KILN_VALUE_OBJECT) {
        return kiln_c99_get_key(host, target, key, canonical, out, depth, thrown);
    }
    {
        const kiln_object *object = kiln_c99_object_of(target);
        if (object->shape != NULL) {
            uint32_t p;
            for (p = 0; p < site->pic_count; ++p) {
                if (site->entries[p].shape == object->shape &&
                    site->entries[p].slot < object->slot_capacity) {
                    *out = kiln_val_to_boundary(object->slots[site->entries[p].slot]);
                    return KILN_C99_OK;
                }
            }
            {
                uint32_t slot;
                if (canonical != NULL && kiln_c99_shape_find_slot(object->shape, canonical, &slot)) {
                    if (slot < object->slot_capacity) {
                        *out = kiln_val_to_boundary(object->slots[slot]);
                        if (site->pic_count < KILN_C99_PIC_SIZE) {
                            site->entries[site->pic_count].shape = object->shape;
                            site->entries[site->pic_count].transition_to = NULL;
                            site->entries[site->pic_count].slot = slot;
                            site->pic_count += 1;
                        }
                        return KILN_C99_OK;
                    }
                }
            }
            if (object->properties == NULL) {
                kiln_value current = object->prototype;
                while (current.tag == KILN_VALUE_OBJECT) {
                    const kiln_object *proto_obj = kiln_c99_object_of(current);
                    if (proto_obj->shape != NULL) {
                        uint32_t proto_slot;
                        if (canonical != NULL &&
                            kiln_c99_shape_find_slot(proto_obj->shape, canonical, &proto_slot)) {
                            if (proto_slot < proto_obj->slot_capacity) {
                                *out = kiln_val_to_boundary(proto_obj->slots[proto_slot]);
                                return KILN_C99_OK;
                            }
                        }
                    } else if (proto_obj->properties != NULL) {
                        size_t index = 0;
                        while (index < proto_obj->count &&
                               !kiln_c99_same_key_view(&proto_obj->properties[index], key)) ++index;
                        if (index < proto_obj->count && !proto_obj->properties[index].accessor) {
                            *out = proto_obj->properties[index].value;
                            return KILN_C99_OK;
                        }
                        break;
                    }
                    current = proto_obj->prototype;
                }
            }
        }
        uint32_t own = kiln_c99_site_decode_dictionary_index(site->own);
        if (own < object->count && object->properties != NULL &&
            kiln_c99_same_key_view(&object->properties[own], key)) {
            if (!object->properties[own].accessor) {
                *out = object->properties[own].value;
                return KILN_C99_OK;
            }
        } else if (site->proto != 0 &&
                   object->prototype.tag == KILN_VALUE_OBJECT) {
            size_t index = 0;
            while (index < object->count && object->properties != NULL &&
                   !kiln_c99_same_key_view(&object->properties[index], key)) ++index;
            if (index == object->count) {
                const kiln_object *parent = kiln_c99_object_of(object->prototype);
                uint32_t at = kiln_c99_site_decode_dictionary_index(site->proto);
                if (parent->properties != NULL && at < parent->count &&
                    kiln_c99_same_key_view(&parent->properties[at], key) &&
                    !parent->properties[at].accessor) {
                    *out = parent->properties[at].value;
                    return KILN_C99_OK;
                }
            }
        }
        status = kiln_c99_get_key(host, target, key, canonical, out, depth, thrown);
        if (status == KILN_C99_OK && site->misses < KILN_C99_SITE_GIVE_UP &&
            !kiln_c99_site_learn(site, object, key, canonical)) {
            site->misses += 1;
        }
        return status;
    }
}
