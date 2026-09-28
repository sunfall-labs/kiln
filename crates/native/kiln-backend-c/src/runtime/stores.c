
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
 * constructor that is never collected keeps its prototype alive -- and nothing here collects.
 *
 * An undefined slot means "not created yet"; a program's own `f.prototype = undefined` is stored
 * as a hole instead (properties.c), because it has to stick: materialising a fresh object for it
 * let `class U extends f {}` define a class where the language throws (issue 28). */
static int kiln_c99_prototype_object(kiln_value function, kiln_value *out) {
    kiln_closure *closure = kiln_c99_closure_of(function);
    kiln_value prototype = kiln_c99_closure_prototype(closure);
    if (prototype.tag == KILN_VALUE_HOLE) {
        prototype = kiln_c99_undefined();
    } else if (prototype.tag == KILN_VALUE_UNDEFINED) {
        /* A region closure's is the realm's (realms issue 08): its slot is in the realm context. */
        kiln_value *slot = kiln_c99_closure_prototype_slot(closure);
        int status;
        if (slot == NULL) return KILN_C99_OUT_OF_MEMORY;
        status = kiln_c99_new_object(slot);
        if (status != KILN_C99_OK) {
            return status;
        }
        /* Defined rather than assigned: `constructor` is a real property and is never
         * enumerated, so `for (const k in new C())` visits what the constructor assigned and not
         * the link back to the class. */
        status = kiln_c99_define_method(*slot, "constructor", 11, function);
        if (status != KILN_C99_OK) {
            return status;
        }
        prototype = *slot;
    }
    *out = prototype;
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
    kiln_value store = kiln_c99_closure_store(closure);
    if (store.tag == KILN_VALUE_UNDEFINED) {
        kiln_value *slot = kiln_c99_closure_store_slot(closure);
        int status;
        if (slot == NULL) return KILN_C99_OUT_OF_MEMORY;
        status = kiln_c99_new_object(slot);
        if (status != KILN_C99_OK) {
            return status;
        }
        store = *slot;
    }
    *out = store;
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

/* Realms issue 08: give back the stack pages below the caller's frame. The bootstrap body is one
 * C function with a frame of tens of kilobytes, and once it has returned nothing lives below the
 * launcher's frame that called it -- but the pages it touched stayed resident for the realm's whole
 * life: 40 KB of every realm, a sixth of what a second realm cost. `MADV_DONTNEED` drops them, and
 * the next call that reaches that deep finds zeroed pages, as a fresh thread's stack would be. Two
 * pages are kept below the frame (the red zone and this call's own), and at most a mebibyte is
 * released, which bounds the call on the main thread's stack, whose mapping grows on demand (a
 * range beyond what is mapped is skipped by the kernel and reported, and the report is ignored).
 * Pages are the collector's concern only above the stack pointer, where its scan starts. */
static void kiln_c99_stack_release_below(void) {
#if defined(__linux__)
    /* Not declared under `-std=c11`, as `pthread_getattr_np` is not; 4 is the value on every
     * architecture this runtime builds for. */
    extern int madvise(void *, size_t, int);
#ifndef MADV_DONTNEED
#define MADV_DONTNEED 4
#endif
    uintptr_t page = (uintptr_t)kiln_c99_region_page();
    uintptr_t here = (uintptr_t)__builtin_frame_address(0);
    uintptr_t top;
    uintptr_t bottom;
    if (page == 0 || here < 4 * page) return;
    top = (here - 2 * page) & ~(page - 1);
    bottom = top > ((uintptr_t)1 << 20) ? top - ((uintptr_t)1 << 20) : page;
    (void)madvise((void *)bottom, (size_t)(top - bottom), MADV_DONTNEED);
#endif
}

/* The launcher, once the realm's bootstrap has run: what the bootstrap stored is the built-ins'
 * definition, so the realm takes its share of the summary back. A realm that finishes its
 * bootstrap never undoes what another realm -- or its own program -- stores later. */
static void kiln_c99_builtin_bootstrap_done(void) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL) return;
    /* The realm that built the shared region has made every closure it will hold (realms issue
     * 08): protect it and let every other realm read it. */
    kiln_c99_region_seal(heap);
    {
        kiln_value store;
        if (kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE),
                                    &store, 0) == KILN_C99_OK && store.tag == KILN_VALUE_OBJECT)
            kiln_c99_final_builtin_store(KILN_VALUE_BUILTIN,
                                         KILN_C99_BUILTIN_OBJECT_PROTOTYPE,
                                         kiln_c99_object_of(store));
    }
    {
        static const uint32_t wide_builtin_stores[] = {
            KILN_C99_BUILTIN_DATE_PROTOTYPE,
            KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE,
        };
        size_t index;
        for (index = 0; index < sizeof wide_builtin_stores / sizeof wide_builtin_stores[0];
             ++index) {
            kiln_value store;
            uint32_t code = wide_builtin_stores[index];
            if (kiln_c99_callable_store(kiln_c99_builtin(code), &store, 0) == KILN_C99_OK &&
                store.tag == KILN_VALUE_OBJECT)
                kiln_c99_sealed_layout_publish(KILN_VALUE_BUILTIN, code,
                                               kiln_c99_object_of(store));
        }
    }
    {
        kiln_value *global = NULL;
        if (kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_GLOBALS, &global) == KILN_C99_OK &&
            global != NULL && global->tag == KILN_VALUE_OBJECT)
            kiln_c99_sealed_layout_publish(KILN_VALUE_OBJECT, KILN_C99_SEALED_GLOBAL_CODE,
                                           kiln_c99_object_of(*global));
    }
    /* The bootstrap's frames are gone; the pages they touched need not stay the realm's. */
    kiln_c99_stack_release_below();
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
        if (kiln_c99_closure_store(closure).tag != KILN_VALUE_OBJECT && !create) {
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

/* Issue 30: whether this realm's program has written an own property onto `builtin` -- exactly
 * that built-in, nothing folded in. A read off a receiver whose members come from a method table
 * (a string's `trim`, an array's `join`, a number's `toFixed`) asks it before the table, since a
 * written member shadows the table's. No heap answers yes: the store is consulted, which costs a
 * lookup and is never wrong. The cold half of `kiln_c99_program_wrote_builtin`. */
static __attribute__((noinline)) int kiln_c99_realm_program_wrote(uint32_t builtin) {
    const kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL) return 1;
    if (heap->builtin_stores == 0) return 0;
    if (builtin >= 64u * KILN_C99_MODIFIED_BUILTIN_WORDS) return heap->program_builtin_beyond;
    return (int)((heap->program_builtins[builtin >> 6] >> (builtin & 63u)) & 1u);
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
    return (((uintptr_t)member >> 3) ^ (builtin * 0x9e3779b1u)) & 63u;
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

/* Issue 30: the prototypes whose members a receiver *kind* answers from a method table (a string's
 * `trim`, an array's `join`, a map's `get`, a regexp's `test`), and so the ones an emitted method
 * fast path -- which answers by name for whichever of these kinds the receiver turns out to be --
 * relies on. Object.prototype is not here: every name the fast paths answer is an own member of
 * one of these, which shadows it. */
static const uint32_t kiln_c99_kind_prototypes[] = {
    KILN_C99_BUILTIN_ARRAY_PROTOTYPE,       KILN_C99_BUILTIN_STRING_PROTOTYPE,
    KILN_C99_BUILTIN_NUMBER_PROTOTYPE,      KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE,
    KILN_C99_BUILTIN_REGEXP_PROTOTYPE,      KILN_C99_BUILTIN_MAP_PROTOTYPE,
    KILN_C99_BUILTIN_SET_PROTOTYPE,         KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE,
    KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE,    KILN_C99_BUILTIN_PROMISE_PROTOTYPE,
    KILN_C99_BUILTIN_DATE_PROTOTYPE,        KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE,
    KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE,  KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE,
    KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY_PROTOTYPE, KILN_C99_BUILTIN_INT16_ARRAY_PROTOTYPE,
    KILN_C99_BUILTIN_UINT16_ARRAY_PROTOTYPE, KILN_C99_BUILTIN_INT32_ARRAY_PROTOTYPE,
    KILN_C99_BUILTIN_UINT32_ARRAY_PROTOTYPE, KILN_C99_BUILTIN_FLOAT32_ARRAY_PROTOTYPE,
    KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE, KILN_C99_BUILTIN_BIG_INT64_ARRAY_PROTOTYPE,
    KILN_C99_BUILTIN_BIG_UINT64_ARRAY_PROTOTYPE,
};
/* Stands for "every kind prototype" in `member_answers`; no built-in has this number. */
#define KILN_C99_ANY_KIND_PROTOTYPE UINT32_C(0xfffffffe)

static int kiln_c99_realm_kind_member_compute(const kiln_gc_heap *heap, const char *member,
                                              size_t len) {
    size_t index;
    for (index = 0; index < sizeof kiln_c99_kind_prototypes / sizeof kiln_c99_kind_prototypes[0];
         ++index) {
        uint32_t builtin = kiln_c99_kind_prototypes[index];
        if (((heap->program_builtins[builtin >> 6] >> (builtin & 63u)) & 1u) != 0 &&
            kiln_c99_builtin_store_has_own(builtin, member, len)) {
            return 0;
        }
    }
    return 1;
}

static __attribute__((noinline)) KILN_C99_RARE_CALL int kiln_c99_realm_kind_member_answer(const char *member,
                                                                                         size_t len) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t at;
    int untouched;
    if (heap == NULL) return 0;
    if (heap->builtin_stores == 0) return 1;
    at = kiln_c99_member_answer_at(KILN_C99_ANY_KIND_PROTOTYPE, member);
    if (heap->member_answers[at].member == member &&
        heap->member_answers[at].builtin == KILN_C99_ANY_KIND_PROTOTYPE &&
        heap->member_answers[at].serial == heap->builtin_store_serial) {
        return heap->member_answers[at].untouched;
    }
    untouched = kiln_c99_realm_kind_member_compute(heap, member, len);
    heap->member_answers[at].member = member;
    heap->member_answers[at].builtin = KILN_C99_ANY_KIND_PROTOTYPE;
    heap->member_answers[at].serial = heap->builtin_store_serial;
    heap->member_answers[at].untouched = (uint8_t)untouched;
    return untouched;
}

/* The cold half of `kiln_c99_kind_member_unmodified`: no kind prototype's store holds `member` in
 * a realm whose program has written onto that prototype. Remembered per member until the realm's
 * next store onto a built-in, like the member answer above. */
static __attribute__((noinline, pure)) KILN_C99_RARE_CALL int
kiln_c99_realm_kind_member_untouched(const char *member, size_t len) {
    const kiln_gc_heap *heap = kiln_c99_gc_heap_if_cached();
    size_t at;
    if (KILN_UNLIKELY(heap == NULL)) return kiln_c99_realm_kind_member_answer(member, len);
    if (heap->builtin_stores == 0) return 1;
    at = kiln_c99_member_answer_at(KILN_C99_ANY_KIND_PROTOTYPE, member);
    if (KILN_LIKELY(heap->member_answers[at].member == member &&
                    heap->member_answers[at].builtin == KILN_C99_ANY_KIND_PROTOTYPE &&
                    heap->member_answers[at].serial == heap->builtin_store_serial)) {
        return heap->member_answers[at].untouched;
    }
    return kiln_c99_realm_kind_member_answer(member, len);
}

/* Whether the program has written onto any prototype a receiver of kind `tag` finds members on
 * before the table answers: the kind's own prototype, and Object.prototype for the members a
 * table hands out on its behalf (`hasOwnProperty`). */
static int kiln_c99_realm_tag_prototypes_pristine(const kiln_gc_heap *heap, uint8_t tag) {
    uint64_t mask0 = UINT64_C(1) << KILN_C99_BUILTIN_OBJECT_PROTOTYPE, mask1 = 0;
    if (heap == NULL) return 0;
    if (heap->builtin_stores == 0) return 1;
#define KILN_C99_TAG_PROTO(id)                                                                      \
    do {                                                                                           \
        if ((id) < 64) mask0 |= UINT64_C(1) << (id);                                               \
        else mask1 |= UINT64_C(1) << ((id) - 64);                                                  \
    } while (0)
    switch (tag) {
    case KILN_VALUE_STRING: KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_STRING_PROTOTYPE); break;
    case KILN_VALUE_NUMBER: KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_NUMBER_PROTOTYPE); break;
    case KILN_VALUE_BOOLEAN: KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE); break;
    case KILN_VALUE_ARRAY: KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_ARRAY_PROTOTYPE); break;
    case KILN_VALUE_REGEXP: KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_REGEXP_PROTOTYPE); break;
    case KILN_VALUE_COLLECTION:
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_MAP_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_SET_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE);
        break;
    case KILN_VALUE_TYPED_ARRAY:
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_INT16_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_UINT16_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_INT32_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_UINT32_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_FLOAT32_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_BIG_INT64_ARRAY_PROTOTYPE);
        KILN_C99_TAG_PROTO(KILN_C99_BUILTIN_BIG_UINT64_ARRAY_PROTOTYPE);
        break;
    default: return 0;
    }
#undef KILN_C99_TAG_PROTO
    return (heap->program_builtins[0] & mask0) == 0 && (heap->program_builtins[1] & mask1) == 0;
}
typedef char kiln_c99_tag_prototypes_fit_the_bitmap[KILN_C99_BUILTIN_BIG_UINT64_ARRAY_PROTOTYPE < 128 &&
                                                            KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE < 128
                                                        ? 1
                                                        : -1];



/* The cold half of `kiln_c99_tag_method_pristine`: a site's method remembered for a receiver
 * *kind* (every string answers `trim` with the same method code) is the table's, and it is still
 * the answer while no prototype the kind's lookup walks before the table holds a member of that
 * name the program wrote. Object-granular first -- a realm that wrote onto none of them answers
 * at once -- then by the method's name, remembered per name like the member answers above, so a
 * realm that polyfilled `Array.prototype.last` keeps `join` cached. A written member means the
 * site asks by name, which consults the stores first (`read.c`). */
static int kiln_c99_realm_tag_method_compute(const kiln_gc_heap *heap, uint64_t code) {
    const char *name = kiln_c99_method_name(code);
    size_t len;
    if (name == NULL) return 0;
    len = strlen(name);
    if (!kiln_c99_realm_kind_member_compute(heap, name, len)) return 0;
    return ((heap->program_builtins[0] >> KILN_C99_BUILTIN_OBJECT_PROTOTYPE) & 1u) == 0 ||
           !kiln_c99_builtin_store_has_own(KILN_C99_BUILTIN_OBJECT_PROTOTYPE, name, len);
}
static int kiln_c99_realm_tag_method_pristine_in(kiln_gc_heap *heap, uint8_t tag, uint64_t code) {
    size_t at;
    int pristine;
    if (heap == NULL) return 0;
    if (kiln_c99_realm_tag_prototypes_pristine(heap, tag)) return 1;
    at = (size_t)((code * 0x9e3779b97f4a7c15u) >> 58);
    if (heap->method_answers[at].code == code && heap->method_answers[at].tag == tag &&
        heap->method_answers[at].serial == heap->builtin_store_serial) {
        return heap->method_answers[at].pristine;
    }
    pristine = kiln_c99_realm_tag_method_compute(heap, code);
    heap->method_answers[at].code = code;
    heap->method_answers[at].tag = tag;
    heap->method_answers[at].serial = heap->builtin_store_serial;
    heap->method_answers[at].pristine = (uint8_t)pristine;
    return pristine;
}
static __attribute__((noinline, pure)) KILN_C99_RARE_CALL int kiln_c99_realm_tag_method_pristine(uint8_t tag,
                                                                                               uint64_t code) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_if_cached();
    if (KILN_UNLIKELY(heap == NULL)) heap = kiln_c99_gc_heap();
    return kiln_c99_realm_tag_method_pristine_in(heap, tag, code);
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
            current = kiln_c99_closure_store(kiln_c99_closure_of(current));
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
