
/* An array index, or -1 for anything else.  A key is an index when it round-trips through the
 * canonical decimal spelling, which is what keeps `a["01"]` a named property and `a["1"]` an
 * element -- the distinction the language draws and the one a naive `atoi` erases. */
static double kiln_c99_array_index(kiln_value key) {
    double number;
    if (key.tag == KILN_VALUE_NUMBER) {
        number = kiln_c99_as_number(key);
    } else if (key.tag == KILN_VALUE_STRING) {
        char buffer[64];
        int length;
        number = kiln_string_to_number(key.ptr, key.len);
        if (!isfinite(number) || number < 0.0 || floor(number) != number) {
            return -1.0;
        }
        length = kiln_number_to_js_string(number, buffer, sizeof(buffer));
        if (length < 0 || (size_t)length != key.len ||
            memcmp(buffer, key.ptr, key.len) != 0) {
            return -1.0;
        }
        return number;
    } else {
        return -1.0;
    }
    if (!isfinite(number) || number < 0.0 || floor(number) != number ||
        number > 4294967294.0) {
        return -1.0;
    }
    return number;
}

/* The object `new f()` gives instances, created on first use.
 *
 * It carries `constructor` back to the function, which is what makes `x.constructor === C` true
 * and what a console reads to print `C {}` rather than `{}`. The two point at each other, so a
 * constructor that is never collected keeps its prototype alive -- and nothing here collects. */
static int kiln_c99_prototype_object(kiln_value function, kiln_value *out) {
    kiln_closure *closure = kiln_c99_closure_of(function);
    if (closure->prototype.tag == KILN_VALUE_UNDEFINED) {
        int status = kiln_c99_new_object(&closure->prototype);
        if (status != KILN_C99_OK) {
            return status;
        }
        /* Defined rather than assigned: `constructor` is a real property and is never
         * enumerated, so `for (const k in new C())` visits what the constructor assigned and not
         * the link back to the class. */
        status = kiln_c99_define_method(closure->prototype, "constructor", 11, function);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    *out = closure->prototype;
    return KILN_C99_OK;
}

/* A prototype belongs to the JavaScript callable, not to its backend representation. Generated
 * functions keep theirs in the closure; Rust-backed constructors use stable realm-owned
 * objects. Keeping that distinction behind this helper prevents a METHOD id from ever being
 * mistaken for a kiln_closure pointer by reflection, `util.inherits`, or `instanceof`. */
static int kiln_c99_callable_prototype(kiln_value callable, kiln_value *out) {
    size_t index;
    int status;
    if (callable.tag == KILN_VALUE_FUNCTION) {
        return kiln_c99_prototype_object(callable, out);
    }
    if (callable.tag != KILN_VALUE_METHOD)
        KILN_C99_REFUSE("a prototype is modelled only for generated functions and native "
                        "constructors");
    for (index = 0;
         index < sizeof kiln_c99_native_constructors / sizeof *kiln_c99_native_constructors;
         ++index) {
        kiln_native_constructor *constructor = &kiln_c99_native_constructors[index];
        if (constructor->method != callable.bits) continue;
        if (constructor->prototype.tag == KILN_VALUE_UNDEFINED) {
            status = kiln_c99_new_object(&constructor->prototype);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_define_method(constructor->prototype, "constructor", 11, callable);
            if (status != KILN_C99_OK) return status;
            if (constructor->parent != 0) {
                kiln_value parent;
                status = kiln_c99_callable_prototype(kiln_c99_method(constructor->parent),
                                                     &parent);
                if (status == KILN_C99_OK)
                    status = kiln_c99_set_prototype(constructor->prototype, parent);
                if (status != KILN_C99_OK) return status;
            }
            switch (constructor->surface) {
            case KILN_C99_NATIVE_EVENT_EMITTER:
                status = kiln_c99_install_event_emitter_methods(constructor->prototype);
                break;
            case KILN_C99_NATIVE_READABLE:
                status = kiln_c99_install_readable_methods(constructor->prototype);
                break;
            case KILN_C99_NATIVE_WRITABLE:
                status = kiln_c99_install_writable_methods(constructor->prototype);
                break;
            case KILN_C99_NATIVE_SERVER_RESPONSE:
                status = kiln_c99_install_server_response_methods(constructor->prototype);
                break;
            default:
                status = kiln_c99_refuse("a native constructor whose prototype surface this "
                                         "backend does not install");
                break;
            }
            if (status != KILN_C99_OK) return status;
        }
        *out = constructor->prototype;
        return KILN_C99_OK;
    }
    KILN_C99_REFUSE("a prototype for a method id that is not a registered native constructor");
}

/* The store `f.x = 1` writes into. Separate from the prototype object: a static belongs to the
 * function and an instance method belongs to what it constructs. */
static int kiln_c99_function_store(kiln_value function, kiln_value *out) {
    kiln_closure *closure = kiln_c99_closure_of(function);
    if (closure->properties.tag == KILN_VALUE_UNDEFINED) {
        int status = kiln_c99_new_object(&closure->properties);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    *out = closure->properties;
    return KILN_C99_OK;
}

/* Return the property store for either representation of callable. `create == 0` makes reads
 * allocation-free when no static was ever attached. Generated closures retain their inline
 * store; runtime methods use the realm registry above. */
static kiln_method_store *kiln_c99_fixture_method_stores;

static void kiln_c99_drop_callable_store(void *pointer) {
    kiln_method_store *entry = (kiln_method_store *)pointer;
    if (entry == NULL) return;
    /* `entry->properties` is a collected object in its own right. The store owns only this
     * routing record, and releasing it here is the explicit half of ADR 0041's contract: the
     * realm drops the record when it drops the registry, the collector is only the backstop.
     * This is the record's one release -- nothing else frees it. */
    kiln_c99_gc_release(entry);
}

/* Bumped by every write that reaches a builtin's own-property store, whether it creates the store
 * or adds to one; a method-call site that cached a builtin's method compares the epoch it learned
 * under, so `Promise.resolve = f` is seen by the next call at every site in the program. Reads
 * pass with `create` clear and do not move it. */
static uint64_t kiln_c99_builtin_store_epoch;

static int kiln_c99_callable_store(kiln_value callable, kiln_value *out, int create) {
    kiln_method_store *entry;
    if (callable.tag == KILN_VALUE_FUNCTION) {
        kiln_closure *closure = kiln_c99_closure_of(callable);
        if (closure->properties.tag != KILN_VALUE_OBJECT && !create) {
            KILN_C99_ABSENT("a callable with no property store, so it has no own member to "
                            "answer",
                            out);
        }
        return kiln_c99_function_store(callable, out);
    }
    if (callable.tag == KILN_VALUE_BOUND) {
        kiln_bound *bound = (kiln_bound *)(uintptr_t)callable.bits;
        if (bound->properties.tag != KILN_VALUE_OBJECT) {
            if (!create) {
                KILN_C99_ABSENT("a bound callable with no property store, so it has no own member "
                                "to answer",
                                out);
            }
            if (kiln_c99_new_object(&bound->properties) != KILN_C99_OK)
                return KILN_C99_OUT_OF_MEMORY;
        }
        *out = bound->properties;
        return KILN_C99_OK;
    }
    if (callable.tag != KILN_VALUE_METHOD && callable.tag != KILN_VALUE_BUILTIN &&
        callable.tag != KILN_VALUE_SETTLER)
        KILN_C99_REFUSE("own properties are not modelled on this callable");
    if (create) kiln_c99_builtin_store_epoch += 1;
    /* Direct C embedding fixtures exercise helpers without entering a realm. Keep that legacy
     * path local; every generated scalar/module entry creates or inherits a realm and therefore
     * uses only the shared registry below. */
    if (!kiln_realm_is_active()) {
        uint64_t store_code = callable.bits | ((uint64_t)callable.tag << 56);
        for (entry = kiln_c99_fixture_method_stores; entry != NULL; entry = entry->next) {
            if (entry->code == store_code) {
                *out = entry->properties;
                return KILN_C99_OK;
            }
        }
        if (!create) {
            KILN_C99_ABSENT("a callable the fixture registry has no store for", out);
        }
        entry = (kiln_method_store *)kiln_c99_gc_alloc(KILN_GC_METHOD_STORE, sizeof *entry);
        if (entry == NULL) return KILN_C99_OUT_OF_MEMORY;
        entry->code = store_code;
        entry->next = kiln_c99_fixture_method_stores;
        if (kiln_c99_new_object(&entry->properties) != KILN_C99_OK) {
            kiln_c99_gc_release(entry);
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_fixture_method_stores = entry;
        *out = entry->properties;
        return KILN_C99_OK;
    }
    /* Runtime callable identity is realm-wide, so its statics must be too. First perform the
     * allocation-free lookup promised above. */
    {
        kiln_method_store *published = NULL;
        int status = kiln_realm_callable_store(callable.tag, callable.bits, NULL, NULL,
                                               (void **)&published);
        if (status != KILN_C99_OK) return status;
        if (published != NULL) {
            *out = published->properties;
            return KILN_C99_OK;
        }
        if (!create) {
            KILN_C99_ABSENT("a callable the realm registry has no store for", out);
        }
    }
    /* Publication transfers the winning candidate and destructor to the realm. */
    entry = (kiln_method_store *)kiln_c99_gc_alloc(KILN_GC_METHOD_STORE, sizeof *entry);
    if (entry == NULL) return KILN_C99_OUT_OF_MEMORY;
    entry->code = callable.bits | ((uint64_t)callable.tag << 56);
    entry->next = NULL;
    if (kiln_c99_new_object(&entry->properties) != KILN_C99_OK) {
        kiln_c99_gc_release(entry);
        return KILN_C99_OUT_OF_MEMORY;
    }
    {
        kiln_method_store *published = NULL;
        int status = kiln_realm_callable_store(callable.tag, callable.bits, entry,
                                               kiln_c99_drop_callable_store,
                                               (void **)&published);
        if (status != KILN_C99_OK) {
            kiln_c99_gc_release(entry);
            return status;
        }
        if (published != entry) {
            kiln_c99_drop_callable_store(entry);
            entry = published;
        }
    }
    *out = entry->properties;
    return KILN_C99_OK;
}

/* Read a named property.  A missing one is undefined; a read *through* undefined or null is an
 * error, because that is a TypeError in the language and answering undefined would hide it. */
