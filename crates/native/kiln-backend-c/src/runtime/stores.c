
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
        const kiln_string *string = key.ptr == NULL ? NULL : (const kiln_string *)key.ptr;
        const char *bytes = string != NULL && !kiln_string_is_utf16(string)
                                ? (const char *)kiln_string_latin1(string)
                                : NULL;
        size_t key_len = string == NULL ? 0 : string->unit_len;
        if (bytes == NULL) return -1.0;
        number = kiln_string_to_number(bytes, key_len);
        if (!isfinite(number) || number < 0.0 || floor(number) != number) {
            return -1.0;
        }
        length = kiln_c99_double_to_js(number, buffer);
        if ((size_t)length != key_len || memcmp(buffer, bytes, key_len) != 0) {
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
        const kiln_native_constructor *constructor = &kiln_c99_native_constructors[index];
        kiln_value *prototype;
        if (constructor->method != callable.bits) continue;
        /* The realm's own prototype for this constructor (ADR 0038): the slot is in the realm
         * context, which does not move, so the pointer stays good across the allocations below. */
        prototype = &kiln_c99_realm()->native_prototypes[index];
        if (prototype->tag == KILN_VALUE_UNDEFINED) {
            kiln_value built;
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) return status;
            *prototype = built;
            status = kiln_c99_define_method(*prototype, "constructor", 11, callable);
            if (status != KILN_C99_OK) return status;
            if (constructor->parent != 0) {
                kiln_value parent;
                status = kiln_c99_callable_prototype(kiln_c99_method(constructor->parent),
                                                     &parent);
                if (status == KILN_C99_OK)
                    status = kiln_c99_set_prototype(*prototype, parent);
                if (status != KILN_C99_OK) return status;
            }
            switch (constructor->surface) {
            case KILN_C99_NATIVE_EVENT_EMITTER:
                status = kiln_c99_install_event_emitter_methods(*prototype);
                break;
            case KILN_C99_NATIVE_READABLE:
                status = kiln_c99_install_readable_methods(*prototype);
                break;
            case KILN_C99_NATIVE_WRITABLE:
                status = kiln_c99_install_writable_methods(*prototype);
                break;
            case KILN_C99_NATIVE_SERVER_RESPONSE:
                status = kiln_c99_install_server_response_methods(*prototype);
                break;
            default:
                status = kiln_c99_refuse("a native constructor whose prototype surface this "
                                         "backend does not install");
                break;
            }
            if (status != KILN_C99_OK) return status;
        }
        *out = *prototype;
        return KILN_C99_OK;
    }
    KILN_C99_REFUSE("a prototype for a method id that is not a registered native constructor");
}

/* `Array.prototype[Symbol.unscopables]` (ECMA-262 §23.1.3.32).
 *
 * A distinct realm-held object with a null prototype and 16 properties set to `true`:
 * `at`, `copyWithin`, `entries`, `fill`, `find`, `findIndex`, `findLast`,
 * `findLastIndex`, `flat`, `flatMap`, `includes`, `keys`, `toReversed`, `toSorted`,
 * `toSpliced`, `values`.
 * Its property descriptor on Array.prototype is non-writable, non-enumerable,
 * configurable (`w0e0c1`). Identity is observable across reads:
 * `Array.prototype[Symbol.unscopables] === Array.prototype[Symbol.unscopables]`. */
int kiln_c99_get_array_unscopables(kiln_value *out) {
    /* The realm's (`kiln_c99_realm_state.array_unscopables`). */
    if (kiln_c99_realm()->array_unscopables.tag == KILN_VALUE_UNDEFINED) {
        kiln_value built;
        int status = kiln_c99_new_object(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        kiln_c99_object_of(built)->prototype = kiln_c99_null();
        static const struct {
            const char *name;
            size_t len;
        } names[] = {
            {"at", 2},
            {"copyWithin", 10},
            {"entries", 7},
            {"fill", 4},
            {"find", 4},
            {"findIndex", 9},
            {"findLast", 8},
            {"findLastIndex", 13},
            {"flat", 4},
            {"flatMap", 7},
            {"includes", 8},
            {"keys", 4},
            {"toReversed", 10},
            {"toSorted", 8},
            {"toSpliced", 9},
            {"values", 6}
        };
        for (size_t i = 0; i < sizeof names / sizeof *names; ++i) {
            status = kiln_c99_own_data(built, names[i].name, names[i].len, kiln_c99_boolean(1));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        kiln_c99_realm()->array_unscopables = built;
    }
    *out = kiln_c99_realm()->array_unscopables;
    return KILN_C99_OK;
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

/* ADR 0071: which built-ins the current realm has modified. A write that reaches a built-in's
 * own-property store, whether it creates the store or adds to one, marks that built-in in the
 * realm's heap (and moves the process-wide summary `kiln_c99_builtin_store_epoch`, which says
 * only that some realm has modified something). Reads pass with `create` clear and mark nothing.
 * A mark is never cleared: a modified built-in is the realm's own copy from then on (ADR 0070
 * primitive B). */
/* `kiln_c99_object_prototype_unmodified` names Object.prototype by its number. */
typedef char kiln_c99_object_prototype_is_18[KILN_C99_BUILTIN_OBJECT_PROTOTYPE == 18 ? 1 : -1];

static void kiln_c99_realm_note_builtin_store(uint32_t builtin) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL) return;
    if (builtin < 64u * KILN_C99_MODIFIED_BUILTIN_WORDS) {
        heap->modified_builtins[builtin >> 6] |= UINT64_C(1) << (builtin & 63u);
        heap->program_builtins[builtin >> 6] |= UINT64_C(1) << (builtin & 63u);
    } else {
        heap->modified_builtin_beyond = 1;
        heap->program_builtin_beyond = 1;
    }
}

/* A store onto a built-in's own-property store: the process-wide summary moves, and the realm
 * remembers its share of it. */
static void kiln_c99_realm_note_store(void) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    __atomic_add_fetch(&kiln_c99_builtin_store_epoch, 1u, __ATOMIC_RELAXED);
    if (heap != NULL) {
        heap->builtin_stores += 1;
        heap->builtin_store_serial += 1;
    }
}

/* The launcher, once the realm's bootstrap has run: what the bootstrap stored is the built-ins'
 * definition, so the realm takes its share of the summary back. A realm that finishes its
 * bootstrap never undoes what another realm -- or its own program -- stores later. */
static void kiln_c99_builtin_bootstrap_done(void) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL) return;
    __atomic_sub_fetch(&kiln_c99_builtin_store_epoch, heap->builtin_stores, __ATOMIC_RELAXED);
    heap->builtin_stores = 0;
    memset(heap->program_builtins, 0, sizeof heap->program_builtins);
    heap->program_builtin_beyond = 0;
}

/* The slow half of `kiln_c99_builtins_unmodified`, asked once some realm has stored onto a
 * built-in: whether the current realm has not, outside its bootstrap. No heap answers no. */
static __attribute__((noinline)) KILN_C99_RARE_CALL int kiln_c99_realm_builtins_unmodified_uncached(void) {
    const kiln_gc_heap *heap = kiln_c99_gc_heap();
    return heap != NULL && heap->builtin_stores == 0;
}
/* Calls nothing while the heap is cached: see the narrow answers below. */
static __attribute__((noinline, pure)) KILN_C99_RARE_CALL int kiln_c99_realm_builtins_unmodified(void) {
    const kiln_gc_heap *heap = kiln_c99_gc_heap_if_cached();
    if (KILN_UNLIKELY(heap == NULL)) return kiln_c99_realm_builtins_unmodified_uncached();
    return heap->builtin_stores == 0;
}

static int kiln_c99_realm_marked_builtin(const kiln_gc_heap *heap, uint32_t builtin) {
    if (builtin >= 64u * KILN_C99_MODIFIED_BUILTIN_WORDS) return heap->modified_builtin_beyond;
    return (heap->modified_builtins[builtin >> 6] >> (builtin & 63u)) & 1u;
}

/* Whether a cached member of `builtin` may have changed in the current realm: the realm has
 * modified that built-in, or a prototype built-in its lookup falls through to (Object.prototype,
 * Function.prototype, %TypedArray%.prototype). Asked only once some realm has modified some
 * built-in; a realm without a heap answers yes, which only costs a cache hit. */
static int kiln_c99_realm_builtin_modified(uint32_t builtin) {
    const kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL) return 1;
    return kiln_c99_realm_marked_builtin(heap, builtin) ||
           kiln_c99_realm_marked_builtin(heap, KILN_C99_BUILTIN_OBJECT_PROTOTYPE) ||
           kiln_c99_realm_marked_builtin(heap, KILN_C99_BUILTIN_FUNCTION_PROTOTYPE) ||
           kiln_c99_realm_marked_builtin(heap, KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE);
}


static int kiln_c99_callable_store_lookup(kiln_value callable, kiln_value *out, int create);

/* The own-property store of a callable or built-in. Object.prototype's store, once it exists, is
 * also recorded on the realm's heap so an absent path ending there can consult it live
 * (`kiln_c99_object_prototype_lacks`). */
static int kiln_c99_callable_store(kiln_value callable, kiln_value *out, int create) {
    int status = kiln_c99_callable_store_lookup(callable, out, create);
    if (status == KILN_C99_OK && callable.tag == KILN_VALUE_BUILTIN &&
        callable.bits == KILN_C99_BUILTIN_OBJECT_PROTOTYPE && out->tag == KILN_VALUE_OBJECT) {
        kiln_gc_heap *heap = kiln_c99_gc_heap();
        if (heap != NULL) heap->object_prototype_store = kiln_c99_object_of(*out);
    }
    return status;
}

static int kiln_c99_callable_store_lookup(kiln_value callable, kiln_value *out, int create) {
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
    if (create) {
        kiln_c99_realm_note_store();
        if (callable.tag == KILN_VALUE_BUILTIN) kiln_c99_realm_note_builtin_store((uint32_t)callable.bits);
    }
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

/* Issue 20: the speculations ask about the built-ins they rely on, not about the realm as a whole.
 * `Math.foo = 1` is a store onto Math and nothing else; a body that inlined `Math.sqrt` or that
 * iterates an array has no reason to leave its fast path for it. Both answers below read the
 * realm's *program* record (`program_builtins`, what it stored after its bootstrap), so they are
 * never wrong where `kiln_c99_realm_builtins_unmodified` would have said yes. No heap answers no. */

/* Both answers are the cold half of a guard that runs on every call once any realm has stored onto
 * any built-in, and they preserve every register (`KILN_C99_RARE_CALL`): so their common path --
 * the heap cached, the answer in the record or remembered -- calls nothing, and anything that has
 * to call (finding the heap, reading a store) is in a function of its own that preserves every
 * register too, so only that rare path pays for the saves. Measured on a
 * class-poly loop in a realm that wrote `JSON.foo`: a slow half that called `kiln_c99_gc_heap` saved
 * and restored all sixteen vector registers per guard, 190 instructions per method call. */

/* The object-granular answer: none of the built-ins in the mask (bit `n` of word `n / 64`), nor a
 * prototype built-in every lookup can fall through to (Object.prototype, Function.prototype,
 * %TypedArray%.prototype), has had an own property written by this realm's program. `beyond` asks
 * about the numbers past the bitmap as well. */
static int kiln_c99_realm_builtins_untouched_in(const kiln_gc_heap *heap, uint64_t mask0,
                                                uint64_t mask1, int beyond) {
    if (heap == NULL) return 0;
    if (heap->builtin_stores == 0) return 1;
    mask0 |= (UINT64_C(1) << KILN_C99_BUILTIN_OBJECT_PROTOTYPE) |
             (UINT64_C(1) << KILN_C99_BUILTIN_FUNCTION_PROTOTYPE) |
             (UINT64_C(1) << KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE);
    return (heap->program_builtins[0] & mask0) == 0 && (heap->program_builtins[1] & mask1) == 0 &&
           !(beyond && heap->program_builtin_beyond);
}
typedef char kiln_c99_folded_prototypes_fit_word_0[KILN_C99_BUILTIN_FUNCTION_PROTOTYPE < 64 &&
                                                           KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE < 64
                                                       ? 1
                                                       : -1];
static __attribute__((noinline)) KILN_C99_RARE_CALL int kiln_c99_realm_builtins_untouched_uncached(uint64_t mask0,
                                                                              uint64_t mask1,
                                                                              int beyond) {
    return kiln_c99_realm_builtins_untouched_in(kiln_c99_gc_heap(), mask0, mask1, beyond);
}
static __attribute__((noinline, pure)) KILN_C99_RARE_CALL int
kiln_c99_realm_builtins_untouched(uint64_t mask0, uint64_t mask1, int beyond) {
    const kiln_gc_heap *heap = kiln_c99_gc_heap_if_cached();
    if (KILN_UNLIKELY(heap == NULL)) {
        return kiln_c99_realm_builtins_untouched_uncached(mask0, mask1, beyond);
    }
    return kiln_c99_realm_builtins_untouched_in(heap, mask0, mask1, beyond);
}

/* Whether `builtin`'s own-property store holds `member` (spelt `len` bytes). A built-in's own
 * members are its table's until the program writes one: every read consults the store first
 * (`read.c`, the top of the built-in case), and a delete of a table member changes nothing. So a
 * store without the name means the member is the table's. Reads only; converts nothing. */
static int kiln_c99_builtin_store_has_own(uint32_t builtin, const char *member, size_t len) {
    kiln_value store;
    const kiln_object *object;
    size_t index;
    if (kiln_c99_callable_store(kiln_c99_builtin(builtin), &store, 0) != KILN_C99_OK) return 1;
    if (store.tag != KILN_VALUE_OBJECT) return 0;
    object = kiln_c99_object_of(store);
    if (object->shape != NULL) return kiln_c99_shape_slot_named(object->shape, member, len) != UINT32_MAX;
    for (index = 0; index < object->count; ++index) {
        if (kiln_c99_same_key(&object->properties[index], member, len)) return 1;
    }
    return 0;
}

/* Where `member_answers` keeps the answer about (`builtin`, `member`). */
static inline size_t kiln_c99_member_answer_at(uint32_t builtin, const char *member) {
    return (((uintptr_t)member >> 3) ^ builtin) & 7u;
}

/* The member-granular answer's common path, or -1 when it has to read the store: yes while the
 * realm's program has not modified the built-in at all, and a remembered answer while no store
 * onto any built-in has happened since it was taken. */
static inline int kiln_c99_realm_member_known(const kiln_gc_heap *heap, uint32_t builtin,
                                              const char *member) {
    size_t at;
    if (heap->builtin_stores == 0) return 1;
    if (builtin < 64u * KILN_C99_MODIFIED_BUILTIN_WORDS
            ? ((heap->program_builtins[builtin >> 6] >> (builtin & 63u)) & 1u) == 0
            : !heap->program_builtin_beyond) {
        return 1;
    }
    at = kiln_c99_member_answer_at(builtin, member);
    if (heap->member_answers[at].member == member && heap->member_answers[at].builtin == builtin &&
        heap->member_answers[at].serial == heap->builtin_store_serial) {
        return heap->member_answers[at].untouched;
    }
    return -1;
}

static __attribute__((noinline)) KILN_C99_RARE_CALL int kiln_c99_realm_member_answer(uint32_t builtin,
                                                                 const char *member, size_t len) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t at;
    int untouched;
    if (heap == NULL) return 0;
    untouched = kiln_c99_realm_member_known(heap, builtin, member);
    if (untouched >= 0) return untouched;
    untouched = !kiln_c99_builtin_store_has_own(builtin, member, len);
    at = kiln_c99_member_answer_at(builtin, member);
    heap->member_answers[at].member = member;
    heap->member_answers[at].builtin = builtin;
    heap->member_answers[at].serial = heap->builtin_store_serial;
    heap->member_answers[at].untouched = (uint8_t)untouched;
    return untouched;
}

/* The member-granular answer, for a speculation on one own member of one built-in (`Math.sqrt`,
 * `Number.isInteger`, `Promise.resolve`, `Promise.prototype.constructor`): the member is still
 * the table's in this realm. A member found on the built-in itself does not depend on its
 * prototype, so nothing is folded in. The answer is remembered until the realm next stores onto
 * a built-in (`builtin_store_serial`), so a realm that wrote `Math.foo` reads the store once per
 * store, and compares a few words per call after that. */
static __attribute__((noinline, pure)) KILN_C99_RARE_CALL int
kiln_c99_realm_builtin_member_untouched(uint32_t builtin, const char *member, size_t len) {
    const kiln_gc_heap *heap = kiln_c99_gc_heap_if_cached();
    int known;
    if (KILN_UNLIKELY(heap == NULL)) return kiln_c99_realm_member_answer(builtin, member, len);
    known = kiln_c99_realm_member_known(heap, builtin, member);
    if (KILN_LIKELY(known >= 0)) return known;
    return kiln_c99_realm_member_answer(builtin, member, len);
}

/* Canonical headed store walk shared by arrays, collections and promises. The raw helper in
 * keys.c remains a Latin1 adapter for fixed C names; arbitrary property keys enter here so the
 * receiver's store and its prototype chain compare UTF-16 units exactly once. */
static int kiln_c99_store_get_key(kiln_host *host, kiln_value target,
                                  const kiln_string *key, const kiln_string *canonical,
                                  kiln_value *out, int depth, kiln_value *thrown, int *found) {
    kiln_value *slot = kiln_c99_exotic_store(target);
    kiln_value current;
    size_t steps;
    if (found == NULL || out == NULL || key == NULL) return KILN_C99_INVALID_ARGUMENT;
    *found = 0;
    if (slot == NULL) return KILN_C99_OK;
    current = *slot;
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        kiln_object *object;
        size_t index;
        if (current.tag == KILN_VALUE_FUNCTION) {
            current = kiln_c99_closure_of(current)->properties;
        }
        if (current.tag != KILN_VALUE_OBJECT) return KILN_C99_OK;
        object = kiln_c99_object_of(current);
        if (object->shape != NULL) kiln_c99_ensure_dictionary(object);
        index = kiln_c99_own_index_key(object, canonical, key);
        if (index != object->count) {
            *found = 1;
            if (object->properties[index].refused) {
                return kiln_c99_refuse_named_key(
                    "a member this backend declares and has not implemented", key);
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

/* Read a named property.  A missing one is undefined; a read *through* undefined or null is an
 * error, because that is a TypeError in the language and answering undefined would hide it. */

static int kiln_c99_object_prototype_lacks(const kiln_string *key) {
    const kiln_gc_heap *heap = kiln_c99_gc_heap();
    const kiln_object *store = heap != NULL ? heap->object_prototype_store : NULL;
    size_t index;
    if (store == NULL) return 1;
    if (store->shape != NULL) {
        uint32_t slot;
        for (slot = 0; slot < store->shape->property_count; ++slot) {
            if (store->shape->keys[slot] == key) return 0;
        }
    }
    for (index = 0; index < store->count && store->properties != NULL; ++index) {
        if (store->properties[index].key == key) return 0;
    }
    return 1;
}
