
/* Fixed-name readers still have byte-oriented tables.  A headed key can use those tables only
 * when every unit is ASCII; this adapter is exact in that case.  A wide or non-ASCII key takes
 * the headed property walk below rather than being converted or refused. */
static int kiln_c99_intrinsic_ascii_view(const kiln_string *key, char buffer[256],
                                         const char **bytes, size_t *length) {
    size_t index;
    if (key == NULL) return 0;
    /* Fixed built-in tables use @@ spellings only for Symbol and private keys. The same units
     * in a program string are an ordinary property name and must miss those tables. */
    if (key->unit_len >= 2 && kiln_string_unit_at(key, 0) == '@' &&
        kiln_string_unit_at(key, 1) == '@' && !kiln_c99_key_is_reserved(key)) return 0;
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

static int kiln_c99_store_has(kiln_value store, const kiln_string *key,
                              const kiln_string *ikey) {
    kiln_property property;
    return store.tag == KILN_VALUE_OBJECT &&
           kiln_c99_property_read_own_value(store, key, ikey, &property);
}

/* Resolve a constructor's species through its actual prototype chain. Accessors receive
 * the constructor that started the read, including an inherited intrinsic accessor. */
static int kiln_c99_function_species(kiln_host *host, kiln_value target,
                                      const kiln_string *key, const kiln_string *ikey,
                                      kiln_value *out, int depth, kiln_value *thrown) {
    kiln_value current = target;
    size_t steps;
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        kiln_value store = current;
        kiln_property property;
        int status;
        if (current.tag == KILN_VALUE_NULL) {
            KILN_C99_ABSENT("species is absent after every own descriptor in the prototype chain was checked", out);
        }
        if (current.tag == KILN_VALUE_PROXY) {
            return kiln_c99_get_with_receiver(host, current, kiln_c99_key_value(key, NULL),
                                              target, out, depth, thrown);
        }
        if (current.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_callable_store(current, &store, 0);
            if (status != KILN_C99_OK) return status;
        }
        if (store.tag == KILN_VALUE_OBJECT &&
            kiln_c99_property_read_own_value(store, key, ikey, &property)) {
            if (property.refused) KILN_C99_REFUSE("a species descriptor declared but not implemented");
            if (!property.accessor) { *out = property.value; return KILN_C99_OK; }
            if (property.getter.tag == KILN_VALUE_UNDEFINED) {
                KILN_C99_ABSENT("a species accessor without a getter", out);
            }
            return kiln_c99_call(host, out, property.getter, target, NULL, 0, depth, thrown);
        }
        if (current.tag == KILN_VALUE_BUILTIN) {
            kiln_typed_kind kind;
            if (current.bits == KILN_C99_BUILTIN_ARRAY ||
                current.bits == KILN_C99_BUILTIN_MAP ||
                current.bits == KILN_C99_BUILTIN_SET ||
                current.bits == KILN_C99_BUILTIN_PROMISE ||
                current.bits == KILN_C99_BUILTIN_REGEXP ||
                current.bits == KILN_C99_BUILTIN_ARRAY_BUFFER ||
                current.bits == KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER ||
                kiln_c99_builtin_typed_kind(current.bits, &kind)) {
                *out = target;
                return KILN_C99_OK;
            }
        }
        status = kiln_c99_invoke_method(host, &current, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                                        kiln_c99_undefined(), &current, 1, depth, thrown);
        if (status != KILN_C99_OK) return status;
    }
    KILN_C99_REFUSE("a species prototype chain exceeding the traversal limit");
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
        kiln_property property;
        if (kiln_c99_property_read_own_value(store, key, ikey, &property)) {
            *found = 1;
            return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
        }
    }
    return KILN_C99_OK;
}

/* Accessors installed on a native prototype must observe the original receiver, not the hidden
 * property-store object. Data members can use the ordinary projection above; Map/Set.size is the
 * first native accessor whose TypeScript getter needs its instance's internal collection slot. */
static int kiln_c99_prototype_store_read_with_receiver(
    kiln_host *host, uint64_t prototype, kiln_value receiver, const kiln_string *key,
    const kiln_string *ikey, kiln_value *out, int depth, kiln_value *thrown, int *found) {
    kiln_value store;
    kiln_property property;
    int status = kiln_c99_callable_store(kiln_c99_builtin(prototype), &store, 0);
    *found = 0;
    if (status != KILN_C99_OK || store.tag != KILN_VALUE_OBJECT) return status;
    if (!kiln_c99_property_read_own_value(store, key, ikey, &property)) return KILN_C99_OK;
    *found = 1;
    if (!property.accessor) return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
    if (property.getter.tag == KILN_VALUE_UNDEFINED) {
        KILN_C99_ABSENT("an own accessor descriptor whose getter is undefined", out);
    }
    return kiln_c99_call(host, out, property.getter, receiver, NULL, 0, depth, thrown);
}

/* Issue 30: a receiver whose members come from a method table -- a string's `trim`, an array's
 * `join`, a number's `toFixed` -- inherits them from a prototype built-in, and a member the program
 * wrote onto that built-in (`String.prototype.trim = f`) shadows the table's, exactly as it does
 * in the language. So before a table answers, the prototype's store is asked -- only once the
 * program has written onto it (`kiln_c99_program_wrote_builtin`), which in a realm that has never
 * written onto a built-in is one compare. */
static int kiln_c99_written_prototype_read(kiln_host *host, uint64_t prototype,
                                           const kiln_string *key, const kiln_string *ikey,
                                           kiln_value *out, int depth, kiln_value *thrown,
                                           int *found) {
    *found = 0;
    if (KILN_LIKELY(!kiln_c99_program_wrote_builtin((uint32_t)prototype))) return KILN_C99_OK;
    return kiln_c99_prototype_store_read(host, prototype, key, ikey, out, depth, thrown, found);
}
#define KILN_C99_WRITTEN_MEMBER_FIRST(prototype)                                                   \
    do {                                                                                           \
        int written_found_;                                                                        \
        int written_status_ = kiln_c99_written_prototype_read(host, (prototype), key, ikey, out,  \
                                                              depth, thrown, &written_found_);     \
        if (written_status_ != KILN_C99_OK || written_found_) return written_status_;              \
    } while (0)

/* The end of a primitive's chain: its wrapper prototype had no such member, so the lookup goes on
 * to Object.prototype, as `GetV` does -- `"x".hasOwnProperty`, `(1).isPrototypeOf`. What the
 * program stored there comes first, then the members the runtime answers for it. `*found` says
 * whether either had the name. */
static int kiln_c99_primitive_object_prototype_read(kiln_host *host, const kiln_string *key,
                                                    const kiln_string *ikey, const char *intrinsic_key,
                                                    size_t intrinsic_len, kiln_value *out, int depth,
                                                    kiln_value *thrown, int *found) {
    int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_OBJECT_PROTOTYPE, key, ikey,
                                               out, depth, thrown, found);
    uint64_t code;
    if (status != KILN_C99_OK || *found) return status;
    if (kiln_c99_same_name_key(key, "__proto__")) return KILN_C99_OK;
    code = kiln_c99_namespace_method(KILN_C99_BUILTIN_OBJECT_PROTOTYPE, intrinsic_key, intrinsic_len);
    if (code != 0) {
        *out = kiln_c99_method(code);
        *found = 1;
    }
    return KILN_C99_OK;
}

/* Copy a sealed field through the provider lease. Neither a key nor a child edge points into
 * the arena after this call; strings become this realm's ordinary GC strings, and child nodes
 * get their own weak-cached realm facade. The caller holds `target` across all allocations. */
static int kiln_c99_shared_field_value(uint64_t lease, uint64_t index,
                                        const kiln_shared_field_info *field, kiln_value *out) {
    uint16_t small[64];
    uint16_t *units = small;
    uint64_t length = field->value_length;
    kiln_shared_field_info copied;
    int status;
    if (field->kind == KILN_SHARED_FIELD_UNDEFINED)
        KILN_C99_ABSENT("the sealed data property explicitly stores undefined", out);
    if (field->kind == KILN_SHARED_FIELD_NULL) { *out = kiln_c99_null(); return KILN_C99_OK; }
    if (field->kind == KILN_SHARED_FIELD_BOOLEAN) {
        *out = kiln_c99_boolean(field->bits != 0); return KILN_C99_OK;
    }
    if (field->kind == KILN_SHARED_FIELD_NUMBER) {
        *out = kiln_c99_value(KILN_VALUE_NUMBER, field->bits, NULL, 0); return KILN_C99_OK;
    }
    if (field->kind == KILN_SHARED_FIELD_NODE) {
        return kiln_c99_new_shared_facade((uint32_t)field->bits, field->generation, field->node, out);
    }
    if (field->kind != KILN_SHARED_FIELD_STRING && field->kind != KILN_SHARED_FIELD_BIGINT)
        return KILN_C99_INVALID_ARGUMENT;
    if (length > SIZE_MAX / sizeof(uint16_t)) return KILN_C99_OUT_OF_MEMORY;
    if (length > sizeof small / sizeof small[0]) {
        units = (uint16_t *)malloc((size_t)length * sizeof(uint16_t));
        if (units == NULL) return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_shared_graph_field(lease, index, &copied, NULL, 0, units, length);
    if (status != 0 || copied.kind != field->kind || copied.value_length != length) {
        if (units != small) free(units);
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (field->kind == KILN_SHARED_FIELD_STRING) {
        *out = kiln_c99_from_units(units, (size_t)length);
        status = out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
    } else {
        char *digits = (char *)malloc((size_t)length == 0 ? 1 : (size_t)length);
        void *handle = NULL;
        if (digits == NULL) status = KILN_C99_OUT_OF_MEMORY;
        else {
            for (uint64_t at = 0; at < length; ++at) digits[at] = (char)units[at];
            status = kiln_bigint_parse((const unsigned char *)digits, (size_t)length, 10, &handle)
                         == 0 ? KILN_C99_OK : KILN_C99_INVALID_ARGUMENT;
            free(digits);
            if (status == KILN_C99_OK) *out = kiln_c99_bigint_value(handle);
        }
    }
    if (units != small) free(units);
    return status;
}

/* The facade owns the strong lease for these sealed bytes. Every offset is checked here, even
 * though the Rust writer validated the arena before publication. A read may allocate a realm
 * string or child facade, so its caller keeps the facade live through that allocation. */
static int kiln_c99_shared_arena_bytes(const kiln_shared_arena_view *view, uint64_t offset,
                                       uint64_t length, const uint8_t **out) {
    if (view->base == NULL || offset > view->length || length > view->length - offset ||
        offset > SIZE_MAX || length > SIZE_MAX) return KILN_C99_INVALID_ARGUMENT;
    *out = view->base + (size_t)offset;
    return KILN_C99_OK;
}

static int kiln_c99_shared_arena_node(const kiln_shared_facade *facade,
                                      kiln_shared_node_record *out) {
    const uint8_t *bytes;
    uint64_t offset = (uint64_t)facade->node * sizeof *out;
    if (facade->node >= facade->arena.node_count ||
        kiln_c99_shared_arena_bytes(&facade->arena, offset, sizeof *out, &bytes) != KILN_C99_OK)
        return KILN_C99_INVALID_ARGUMENT;
    memcpy(out, bytes, sizeof *out);
    return (out->kind == KILN_SHARED_ARENA_RECORD || out->kind == KILN_SHARED_ARENA_ARRAY) &&
                   out->length <= UINT32_MAX && out->field_count <= SIZE_MAX
               ? KILN_C99_OK : KILN_C99_INVALID_ARGUMENT;
}

static int kiln_c99_shared_arena_field(const kiln_shared_facade *facade,
                                       const kiln_shared_node_record *node, uint64_t index,
                                       kiln_shared_field_record *out) {
    const uint8_t *bytes;
    uint64_t offset;
    if (index >= node->field_count || index > (UINT64_MAX - node->fields) / sizeof *out)
        return KILN_C99_INVALID_ARGUMENT;
    offset = node->fields + index * sizeof *out;
    if (kiln_c99_shared_arena_bytes(&facade->arena, offset, sizeof *out, &bytes) != KILN_C99_OK)
        return KILN_C99_INVALID_ARGUMENT;
    memcpy(out, bytes, sizeof *out);
    return KILN_C99_OK;
}

static int kiln_c99_shared_arena_key_equal(const kiln_shared_facade *facade,
                                            const kiln_shared_field_record *field,
                                            const kiln_string *key, int *equal) {
    const uint8_t *bytes;
    *equal = 0;
    if (field->key_length != key->unit_len) return KILN_C99_OK;
    if (field->key_length > UINT64_MAX / sizeof(uint16_t) ||
        kiln_c99_shared_arena_bytes(&facade->arena, field->key_offset,
                                    field->key_length * sizeof(uint16_t), &bytes) != KILN_C99_OK)
        return KILN_C99_INVALID_ARGUMENT;
    for (size_t at = 0; at < key->unit_len; ++at) {
        uint16_t unit;
        memcpy(&unit, bytes + at * sizeof unit, sizeof unit);
        if (unit != kiln_string_unit_at(key, at)) return KILN_C99_OK;
    }
    *equal = 1;
    return KILN_C99_OK;
}

/* Returns 0 with the matching field, 1 when absent, or a native error status. The immutable
 * open-addressed index has one-based field ordinals and uses the writer's UTF-16 FNV hash. */
static int kiln_c99_shared_arena_find(const kiln_shared_facade *facade,
                                      const kiln_shared_node_record *node,
                                      const kiln_string *key, uint64_t *out_index,
                                      kiln_shared_field_record *out_field) {
    uint64_t hash = UINT64_C(0xcbf29ce484222325);
    uint64_t capacity = node->index_capacity;
    const uint8_t *slots = NULL;
    for (size_t at = 0; at < key->unit_len; ++at)
        hash = (hash ^ (uint64_t)kiln_string_unit_at(key, at)) * UINT64_C(0x100000001b3);
    if (capacity != 0) {
        if ((capacity & (capacity - 1)) != 0 || node->field_count > capacity / 2 ||
            capacity > UINT64_MAX / sizeof(uint64_t) ||
            kiln_c99_shared_arena_bytes(&facade->arena, node->index,
                                        capacity * sizeof(uint64_t), &slots) != KILN_C99_OK)
            return KILN_C99_INVALID_ARGUMENT;
        for (uint64_t step = 0, slot = hash & (capacity - 1); step < capacity;
             ++step, slot = (slot + 1) & (capacity - 1)) {
            uint64_t entry;
            int equal;
            memcpy(&entry, slots + (size_t)slot * sizeof entry, sizeof entry);
            if (entry == 0) return 1;
            if (entry - 1 >= node->field_count ||
                kiln_c99_shared_arena_field(facade, node, entry - 1, out_field) != KILN_C99_OK ||
                kiln_c99_shared_arena_key_equal(facade, out_field, key, &equal) != KILN_C99_OK)
                return KILN_C99_INVALID_ARGUMENT;
            if (equal) { *out_index = entry - 1; return 0; }
        }
        return 1;
    }
    for (uint64_t index = 0; index < node->field_count; ++index) {
        int equal;
        if (kiln_c99_shared_arena_field(facade, node, index, out_field) != KILN_C99_OK ||
            kiln_c99_shared_arena_key_equal(facade, out_field, key, &equal) != KILN_C99_OK)
            return KILN_C99_INVALID_ARGUMENT;
        if (equal) { *out_index = index; return 0; }
    }
    return 1;
}

static int kiln_c99_shared_arena_value(kiln_shared_facade *facade, uint64_t index,
                                       const kiln_shared_field_record *field, kiln_value *out) {
    kiln_shared_field_info scalar = {0};
    const uint8_t *bytes;
    void *bigint = NULL;
    if (field->kind <= KILN_SHARED_FIELD_NUMBER) {
        scalar.kind = field->kind;
        scalar.bits = field->payload;
        return kiln_c99_shared_field_value(facade->lease, index, &scalar, out);
    }
    if (field->kind == KILN_SHARED_FIELD_NODE) {
        if (field->payload >= facade->arena.node_count) return KILN_C99_INVALID_ARGUMENT;
        return kiln_c99_new_shared_facade(facade->graph, facade->generation,
                                          (uint32_t)field->payload, out);
    }
    if (field->kind == KILN_SHARED_FIELD_STRING) {
        if ((field->payload & (sizeof(uint16_t) - 1)) != 0 ||
            field->value_length > SIZE_MAX / sizeof(uint16_t) ||
            field->value_length > UINT64_MAX / sizeof(uint16_t) ||
            kiln_c99_shared_arena_bytes(&facade->arena, field->payload,
                                        field->value_length * sizeof(uint16_t), &bytes) != KILN_C99_OK)
            return KILN_C99_INVALID_ARGUMENT;
        *out = kiln_c99_from_units((const uint16_t *)bytes, (size_t)field->value_length);
        return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
    }
    if (field->kind == KILN_SHARED_FIELD_BIGINT) {
        if (kiln_c99_shared_arena_bytes(&facade->arena, field->payload,
                                        field->value_length, &bytes) != KILN_C99_OK)
            return KILN_C99_INVALID_ARGUMENT;
        if (kiln_bigint_parse(bytes, (size_t)field->value_length, 10, &bigint) != 0)
            return KILN_C99_INVALID_ARGUMENT;
        *out = kiln_c99_bigint_value(bigint);
        return KILN_C99_OK;
    }
    if (field->kind == KILN_SHARED_ARENA_EXTERNAL) {
        kiln_shared_field_info external;
        if (kiln_shared_graph_field(facade->lease, index, &external, NULL, 0, NULL, 0) != 0 ||
            external.kind != KILN_SHARED_FIELD_NODE) return KILN_C99_INVALID_ARGUMENT;
        return kiln_c99_shared_field_value(facade->lease, index, &external, out);
    }
    return KILN_C99_INVALID_ARGUMENT;
}

static int kiln_c99_shared_own_read(kiln_value target, const kiln_string *key,
                                    kiln_value *out, int *found, int *enumerable) {
    kiln_shared_facade *facade = (kiln_shared_facade *)(uintptr_t)target.bits;
    kiln_shared_node_record node;
    kiln_shared_field_record field;
    uint64_t index;
    int lookup;
    int status = KILN_C99_OK;
    *found = 0;
    if (facade == NULL || facade->lease == 0 || facade->arena.base == NULL ||
        facade->arena.layout_version != KILN_SHARED_ARENA_LAYOUT_VERSION ||
        kiln_c99_shared_arena_node(facade, &node) != KILN_C99_OK)
        return KILN_C99_INVALID_ARGUMENT;
    /* A sealed graph has only string keys. A Symbol whose private key has the same code units
     * must not read the string field. */
    if (kiln_c99_key_is_symbol(key)) goto done;
    if (node.kind == KILN_SHARED_ARENA_ARRAY && kiln_c99_same_name_key(key, "length")) {
        *out = kiln_c99_number((double)node.length);
        *found = 1;
        if (enumerable != NULL) *enumerable = 0;
        goto done;
    }
    if (key == NULL) { status = KILN_C99_INVALID_ARGUMENT; goto done; }
    lookup = kiln_c99_shared_arena_find(facade, &node, key, &index, &field);
    if (lookup == 1) goto done;
    if (lookup != 0) { status = KILN_C99_INVALID_ARGUMENT; goto done; }
    *found = 1;
    if (enumerable != NULL) *enumerable = field.enumerable != 0;
    status = kiln_c99_shared_arena_value(facade, index, &field, out);
done:
    /* The facade's lease is the view's lifetime. Keep its pointer in this C frame until every
     * allocation above has returned; the conservative collector scans this frame and registers. */
    kiln_c99_gc_keep_alive(facade);
    return status;
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
    case KILN_VALUE_SHARED: {
        kiln_shared_facade *facade = (kiln_shared_facade *)(uintptr_t)target.bits;
        kiln_shared_info node;
        int found;
        int status = kiln_c99_shared_own_read(target, key, out, &found, NULL);
        uint64_t code;
        if (status != KILN_C99_OK || found) return status;
        if (kiln_shared_graph_info(facade->lease, &node) != 0) return KILN_C99_INVALID_ARGUMENT;
        if (node.null_prototype) {
            KILN_C99_ABSENT("the sealed record's own field list is complete and its prototype "
                            "is null", out);
        }
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        if (node.array) {
            status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_ARRAY_PROTOTYPE,
                                                   key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
            if (kiln_c99_same_name_key(key, "constructor")) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY); return KILN_C99_OK;
            }
            if (kiln_c99_same_name_key(key, "@@iterator") ||
                kiln_c99_same_name_key(key, "values")) {
                *out = kiln_c99_method(KILN_C99_METHOD_ARRAY_VALUES); return KILN_C99_OK;
            }
            if (kiln_c99_same_name_key(key, "@@unscopables"))
                return kiln_c99_get_array_unscopables(out);
            code = kiln_c99_array_method(intrinsic_key, intrinsic_len);
            if (code != 0) { *out = kiln_c99_method(code); return KILN_C99_OK; }
        }
        status = kiln_c99_primitive_object_prototype_read(host, key, ikey, intrinsic_key,
                                                            intrinsic_len, out, depth, thrown,
                                                            &found);
        if (status != KILN_C99_OK || found) return status;
        if (kiln_c99_same_name_key(key, "__proto__")) {
            *out = kiln_c99_builtin(node.array ? KILN_C99_BUILTIN_ARRAY_PROTOTYPE
                                               : KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
            return KILN_C99_OK;
        }
        if (!node.array && kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT); return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "toLocaleString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING); return KILN_C99_OK;
        }
        if (node.array && kiln_c99_is_array_member(intrinsic_key, intrinsic_len))
            KILN_C99_REFUSE("an Array.prototype member not yet implemented for sealed arrays");
        KILN_C99_ABSENT("the sealed field list and the reading realm's prototype method tables "
                        "contain no such key", out);
    }
    case KILN_VALUE_OBJECT: {
        kiln_value current = target;
        kiln_value getter;
        size_t steps;
        int walk;
        /* Canonicalized once for the whole walk (ADR 0047), unless a compiled site already holds
         * the canonical pointer. NULL is a spelling the realm has never stored, so no level of
         * the chain can hold it and the walk is not made at all -- unless the check mode is on,
         * which wants every level's scan run by bytes. */
        if (ikey == NULL) ikey = kiln_c99_interned_key(key);
        walk = ikey != NULL || kiln_c99_keys_checked() || kiln_c99_key_index_key(key) >= 0.0 ||
               (kiln_c99_gc_heap() != NULL && kiln_c99_gc_heap()->foreign_dictionary_keys);
        /* Own properties first, then the chain. Iterative and bounded rather than recursive:
         * the walk is on the C stack, and a program can build a chain long enough to exhaust
         * it. */
        for (steps = 0; walk && steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
            /* A constructor's statics live in its own store, so a chain that reaches one
             * continues *into* that store rather than stopping at the function. */
            if (current.tag == KILN_VALUE_FUNCTION) {
                current = kiln_c99_closure_store(kiln_c99_closure_of(current));
            }
            if (current.tag != KILN_VALUE_OBJECT) {
                if (current.tag != KILN_VALUE_UNDEFINED && current.tag != KILN_VALUE_NULL) {
                    return kiln_c99_get_key(host, current, key, ikey, out, depth, thrown);
                }
                break;
            }
            getter = kiln_c99_undefined();
            {
                kiln_property property;
                if (!kiln_c99_property_read_own_value(current, key, ikey, &property)) {
                    current = kiln_c99_object_prototype_read(current);
                    continue;
                }
                /* A published name with nothing behind it stops the program and lets the
                 * instruction say which one. See `kiln_property::refused`. */
                if (property.refused) {
                    KILN_C99_REFUSE("a member this backend declares and has not implemented");
                }
                if (!property.accessor) {
                    *out = property.value;
                    return KILN_C99_OK;
                }
                getter = property.getter;
                /* Reading an accessor is a call, and the receiver is the object the read
                 * started from -- not the one the accessor was found on. */
                if (getter.tag == KILN_VALUE_UNDEFINED) {
                    KILN_C99_ABSENT("an accessor with no getter, whose read is undefined by "
                                    "definition",
                                    out);
                }
                return kiln_c99_call(host, out, getter, target, NULL, 0, depth, thrown);
            }
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
        kiln_value target_prototype = kiln_c99_object_prototype_read(target);
        if (target_prototype.tag == KILN_VALUE_BUILTIN &&
            target_prototype.bits == KILN_C99_BUILTIN_ITERATOR_PROTOTYPE) {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_ITERATOR_PROTOTYPE,
                                                       key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
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
        }
        /* The chain ends at `Object.prototype`, whose members every object has -- unless the
         * program asked for a bare dictionary with `Object.create(null)`, which is exactly what
         * that asks for. */
        if (target_prototype.tag != KILN_VALUE_NULL) {
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
            KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
            inherited = kiln_c99_namespace_method(KILN_C99_BUILTIN_OBJECT_PROTOTYPE,
                                                  intrinsic_key, intrinsic_len);
            if (inherited != 0) {
                *out = kiln_c99_method(inherited);
                return KILN_C99_OK;
            }
            {
                int found;
                int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_OBJECT_PROTOTYPE,
                                                           key, ikey, out, depth, thrown, &found);
                if (status != KILN_C99_OK || found) return status;
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
            target_prototype.tag != KILN_VALUE_NULL) {
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
        if ((kiln_c99_same_name_key(key, "name") ||
             kiln_c99_same_name_key(key, "length")) &&
            kiln_c99_store_has(kiln_c99_closure_store(closure), key, ikey)) {
            /* `name` and `length` begin as closure metadata, but both are configurable own
             * properties. A program can redefine either in the callable's store, and reads must
             * then observe that descriptor just like any other own property. */
            return kiln_c99_get_key(host, kiln_c99_closure_store(closure), key, ikey, out,
                                    depth, thrown);
        }
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
        if (kiln_c99_same_name_key(key, "@@species")) {
            return kiln_c99_function_species(host, target, key, ikey, out, depth, thrown);
        }
        if (kiln_c99_store_has(kiln_c99_closure_store(closure), key, ikey)) {
            /* An assigned property wins, exactly as an own property shadows an
             * inherited one -- which is what `Function.prototype.call` is. */
            return kiln_c99_get_key(host, kiln_c99_closure_store(closure), key, ikey, out,
                                    depth, thrown);
        }
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE);
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
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
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
        if (kiln_c99_closure_store(closure).tag != KILN_VALUE_OBJECT) {
            KILN_C99_ABSENT("a callable with no property store, so it has no own member to "
                            "answer",
                            out);
        }
        return kiln_c99_get_key(host, kiln_c99_closure_store(closure), key, ikey, out, depth, thrown);
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
        int found = 0;
        size_t index;
        int status = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown, &found);
        if (status != KILN_C99_OK || found) return status;
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
        if (kiln_c99_same_name_key(key, "@@toStringTag")) {
            *out = kiln_c99_copy_text("RegExp", 6);
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
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_REGEXP_PROTOTYPE);
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
        /* RegExp instances are ordinary objects after their own slots and RegExp.prototype.
         * In particular an unknown name such as `length` is a real absent property, which code
         * like kind-of tests directly; refusing it breaks the ordinary `=== undefined` branch. */
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        {
            int found;
            int status = kiln_c99_primitive_object_prototype_read(host, key, ikey, intrinsic_key,
                                                                  intrinsic_len, out, depth,
                                                                  thrown, &found);
            if (status != KILN_C99_OK || found) return status;
        }
        KILN_C99_ABSENT("not an own RegExp slot, RegExp.prototype member, or Object.prototype "
                        "member; the checked stores and method tables are complete",
                        out);
    }
    case KILN_VALUE_ARRAY_BUFFER:
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        /* A `SharedArrayBuffer` (realms issue 06): its own accessors, then what the shared
         * bootstrap defined on its prototype (`arraybuffer.ts`). */
        if (kiln_c99_typed_buffer_of(target)->shared != NULL) {
            kiln_typed_buffer *shared = kiln_c99_typed_buffer_of(target);
            if (kiln_c99_same_name_key(key, "byteLength")) {
                *out = kiln_c99_number((double)kiln_c99_shared_length(shared));
                return KILN_C99_OK;
            }
            if (kiln_c99_same_name_key(key, "growable")) {
                *out = kiln_c99_boolean(shared->shared_growable != 0);
                return KILN_C99_OK;
            }
            if (kiln_c99_same_name_key(key, "maxByteLength")) {
                *out = kiln_c99_number((double)(shared->shared_growable ? shared->shared_capacity
                                                                         : kiln_c99_shared_length(shared)));
                return KILN_C99_OK;
            }
            if (kiln_c99_same_name_key(key, "constructor")) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER);
                return KILN_C99_OK;
            }
            if (kiln_c99_same_name_key(key, "@@toStringTag")) {
                *out = kiln_c99_copy_text("SharedArrayBuffer", 17);
                return KILN_C99_OK;
            }
            {
                int found;
                int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER_PROTOTYPE,
                                                           key, ikey, out, depth, thrown, &found);
                if (status != KILN_C99_OK || found) return status;
            }
        }
        if (kiln_c99_same_name_key(key, "byteLength")) {
            *out = kiln_c99_number((double)kiln_c99_typed_buffer_of(target)->byte_length);
            return KILN_C99_OK;
        }
        /* The internal slot `byteLength` sits beside: until 2026-09-26 this read answered
         * `undefined`, the silent-undefined class, because nothing here could detach. */
        if (kiln_c99_same_name_key(key, "detached")) {
            *out = kiln_c99_boolean(kiln_c99_typed_buffer_of(target)->detached != 0);
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
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE, key,
                                                       ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
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
        if (view->properties.tag == KILN_VALUE_OBJECT) {
            int found;
            int own_status = kiln_c99_store_get_key(host, target, key, ikey, out, depth,
                                                    thrown, &found);
            if (own_status != KILN_C99_OK || found) return own_status;
        }
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
        if (view->is_buffer) {
            /* `Buffer.prototype` -- the object the shared bootstrap assigned to the `Buffer`
             * built-in's store (kiln-builtins/src/buffer.ts) -- comes before the typed-array
             * chain, and before the one C method below, which the bootstrap captured for its own
             * `toString` before it assigned the prototype. An own member there is the answer;
             * anything else falls through to what a `Uint8Array` has. */
            kiln_value proto;
            int proto_status = kiln_c99_buffer_prototype(host, &proto, depth, thrown);
            if (proto_status != KILN_C99_OK) return proto_status;
            if (kiln_c99_store_has(proto, key, ikey))
                return kiln_c99_get_key(host, proto, key, ikey, out, depth, thrown);
        }
        if (view->is_buffer && kiln_c99_same_name_key(key, "toString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_BUFFER_TO_STRING);
            return KILN_C99_OK;
        }
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, kiln_c99_typed_prototype(view->kind),
                                                       key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
            status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE,
                                                   key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                                &intrinsic_len);
            uint64_t method;
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
            *out = method == 0 ? kiln_c99_undefined() : kiln_c99_method(method);
        }
        return KILN_C99_OK;
    }
    case KILN_VALUE_ARRAY: {
        uint64_t code;
        double element_index = kiln_c99_key_index_key(key);
        kiln_array *elements = kiln_c99_array_of(target);
        /* Fixed names and numeric destructuring keys use ordinary Get too. Holes continue
         * through the prototype lookup below instead of hiding inherited index properties. */
        if (element_index >= 0.0 && element_index < (double)elements->count &&
            elements->elements[(size_t)element_index] != KILN_VAL_HOLE) {
            *out = kiln_val_to_boundary(elements->elements[(size_t)element_index]);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "length")) {
            *out = kiln_c99_number((double)kiln_c99_array_of(target)->count);
            return KILN_C99_OK;
        }
        /* Own members and a subclass prototype precede the intrinsic tables even when the
         * program has never modified Array.prototype. Site-less method reads use this path. */
        if (kiln_c99_array_of(target)->properties.tag != KILN_VALUE_UNDEFINED) {
            int found;
            int status = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
        }
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY);
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
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_ARRAY_PROTOTYPE);
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        code = kiln_c99_array_method(intrinsic_key, intrinsic_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
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
        {
            /* A member defined on `Array.prototype` itself rather than answered by the table:
             * one the shared bootstrap installs (the parallel family, ADR 0069) or a program's
             * own polyfill. It lives in the prototype built-in's store, which a read of
             * `Array.prototype.x` already consults; an instance has to reach it too, or
             * `[].x()` is "undefined is not a function" about a method `Array.prototype.x`
             * demonstrably has. */
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_ARRAY_PROTOTYPE, key,
                                                       ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
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
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE);
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
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE, key,
                                                       ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
            status = kiln_c99_primitive_object_prototype_read(host, key, ikey, intrinsic_key,
                                                              intrinsic_len, out, depth, thrown,
                                                              &found);
            if (status != KILN_C99_OK || found) return status;
        }
    }
        /* `GetV` boxes a primitive and then performs an ordinary prototype lookup. A name the
         * wrapper prototype does not contain is absent, not a backend refusal. Keeping that
         * distinction here makes named and computed access share one semantic seam and lets
         * optional chaining compose with arbitrary primitive-valued fields. */
        KILN_C99_ABSENT("not a member of the wrapper prototype this primitive boxes to", out);
    case KILN_VALUE_STRING: {
        uint64_t code;
        /* `length` is the string's own; every other name here is String.prototype's, where a
         * member the program wrote comes first. */
        if (KILN_UNLIKELY(kiln_c99_builtin_store_epoch != 0) &&
            !kiln_c99_same_name_key(key, "length")) {
            KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_STRING_PROTOTYPE);
        }
        if (kiln_c99_same_name_key(key, "@@iterator")) {
            *out = kiln_c99_method(KILN_C99_METHOD_ARRAY_VALUES);
            return KILN_C99_OK;
        }
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
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        code = kiln_c99_string_method(intrinsic_key, intrinsic_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_STRING_PROTOTYPE,
                                                       key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
        }
        if (kiln_c99_is_string_member(intrinsic_key, intrinsic_len)) {
            KILN_C99_REFUSE("a String.prototype member this backend has not implemented");
        }
        {
            int found;
            int status = kiln_c99_primitive_object_prototype_read(host, key, ikey, intrinsic_key,
                                                                  intrinsic_len, out, depth, thrown,
                                                                  &found);
            if (status != KILN_C99_OK || found) return status;
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
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_PROMISE_PROTOTYPE);
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
        /* A Map/Set subclass has an ordinary prototype in front of the native collection
         * surface. Consult the instance store and that chain first, including for names the
         * built-in also defines: `class LRU extends Map { get(key) { ... } }` must call its
         * method, and an own `size` getter must override the native accessor. Previously the
         * native `size`, `constructor`, and method rows returned before the chain was examined,
         * silently making subclasses behave like plain Maps. */
        {
            int found;
            int status = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown,
                                                &found);
            if (status != KILN_C99_OK || found) return status;
        }
        /* A weak collection has no `size` at all, so its name falls through to the absent
         * answer below rather than stopping the program. */
        if (!collection->is_weak && kiln_c99_same_name_key(key, "size")) {
            int found;
            uint64_t prototype = collection->is_map ? KILN_C99_BUILTIN_MAP_PROTOTYPE
                                                    : KILN_C99_BUILTIN_SET_PROTOTYPE;
            int status = kiln_c99_prototype_store_read_with_receiver(
                host, prototype, target, key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
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
        KILN_C99_WRITTEN_MEMBER_FIRST(
            collection->is_weak
                ? (collection->is_map ? KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE : KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE)
                : (collection->is_map ? KILN_C99_BUILTIN_MAP_PROTOTYPE : KILN_C99_BUILTIN_SET_PROTOTYPE));
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
            KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
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
            if (kiln_c99_same_name_key(key, "@@toStringTag")) {
                const char *tag = collection->is_weak
                                      ? (collection->is_map ? "WeakMap" : "WeakSet")
                                      : (collection->is_map ? "Map" : "Set");
                size_t tag_len = collection->is_weak ? 7 : 3;
                *out = kiln_c99_copy_ascii(tag, tag_len);
                return KILN_C99_OK;
            }
            if (kiln_c99_same_name_key(key, "toLocaleString")) {
                *out = kiln_c99_method(KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING);
                return KILN_C99_OK;
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
        int found;
        int status = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown, &found);
        if (status != KILN_C99_OK || found) return status;
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        if (kiln_c99_same_name_key(key, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_DATE);
            return KILN_C99_OK;
        }
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_DATE_PROTOTYPE, key,
                                                       ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
        }
        if (kiln_c99_same_name_key(key, "getTime")) {
            *out = kiln_c99_method(KILN_C99_METHOD_GET_TIME);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "setTime")) {
            *out = kiln_c99_method(KILN_C99_METHOD_SET_TIME);
            return KILN_C99_OK;
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
        int found;
        int status = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown, &found);
        if (status != KILN_C99_OK || found) return status;
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        uint64_t code;
        if (kiln_c99_same_name_key(key, "@@asyncIterator")) {
            /* Only an async generator has one, and answering for a sync one would make
             * `for await` take the wrong loop. */
            if (!kiln_c99_generator_of(target)->async_generator) {
                KILN_C99_ABSENT("a synchronous generator has no @@asyncIterator method; only an "
                                "async generator defines it",
                                out);
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
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE);
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
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
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
            if (kiln_c99_store_has(store, key, ikey))
                return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
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
            if (kiln_c99_same_name_key(key, "prototype")) {
                /* BoundFunctionCreate defines no `prototype`, so the name is inherited from the
                 * bound function's [[Prototype]], which is its target's: a derived class's parent
                 * answers with its own, `Function.prototype` with nothing. Refusing here stopped
                 * `class U extends f.bind() {}` where the language throws a TypeError (issue 28). */
                kiln_value parent;
                int found = 0;
                int status = kiln_c99_invoke_method(host, &parent,
                                                    KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                                                    kiln_c99_undefined(), &target, 1,
                                                    depth, thrown);
                if (status != KILN_C99_OK) return status;
                if (parent.tag == KILN_VALUE_BUILTIN &&
                    parent.bits == KILN_C99_BUILTIN_FUNCTION_PROTOTYPE) {
                    status = kiln_c99_prototype_store_read(host,
                                                           KILN_C99_BUILTIN_FUNCTION_PROTOTYPE,
                                                           key, ikey, out, depth, thrown, &found);
                    if (status != KILN_C99_OK || found) return status;
                }
                if (parent.tag == KILN_VALUE_NULL ||
                    (parent.tag == KILN_VALUE_BUILTIN &&
                     parent.bits == KILN_C99_BUILTIN_FUNCTION_PROTOTYPE)) {
                    KILN_C99_ABSENT("a bound function owns no `prototype` (BoundFunctionCreate "
                                    "defines none), and neither a null [[Prototype]] nor "
                                    "`Function.prototype` without one in its store has it",
                                    out);
                }
                return kiln_c99_get_key(host, parent, key, ikey, out, depth, thrown);
            }
            int found = 0;
            stored = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown, &found);
            if (stored != KILN_C99_OK || found) return stored;
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
        {
            int found = 0;
            stored = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown, &found);
            if (stored != KILN_C99_OK || found) return stored;
        }
        /* `parseInt.name`. A method has no other property worth answering, and a wrong answer
         * here reads as a missing function three frames later. */
        if (kiln_c99_same_name_key(key, "name")) {
            const char *method = kiln_c99_method_name(target.bits);
            *out = method == NULL ? kiln_c99_from_units(NULL, 0)
                                  : kiln_c99_copy_text(method, strlen(method));
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name_key(key, "length")) {
            uint32_t length = kiln_c99_method_declared_length(target.bits);
            if (length != UINT32_MAX) {
                *out = kiln_c99_number((double)length);
                return KILN_C99_OK;
            }
            KILN_C99_REFUSE("the declared length of this native method is not in the arity authority");
        }
        /* The own slots above are synthetic (name/length and supported prototype); whatever the
         * program added to Function.prototype is inherited by every native function as well. */
        {
            int found;
            int status = kiln_c99_prototype_store_read(host,
                                                       KILN_C99_BUILTIN_FUNCTION_PROTOTYPE,
                                                       key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
            /* Native callables inherit Object.prototype through Function.prototype too. */
            status = kiln_c99_prototype_store_read(host,
                                                  KILN_C99_BUILTIN_OBJECT_PROTOTYPE,
                                                  key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                                &intrinsic_len);
            uint64_t inherited = kiln_c99_namespace_method(
                KILN_C99_BUILTIN_OBJECT_PROTOTYPE, intrinsic_key, intrinsic_len);
            if (inherited != 0) {
                *out = kiln_c99_method(inherited);
                return KILN_C99_OK;
            }
        }
        KILN_C99_ABSENT("not the native method's name, supported length or constructor prototype, "
                        "nor a property in its own or callable-prototype stores",
                        out);
    }
    case KILN_VALUE_BUILTIN: {
        (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key, &intrinsic_len);
        kiln_value store;
        int stored = kiln_c99_callable_store(target, &store, 0);
        if (stored != KILN_C99_OK) return stored;
        {
            int found = 0;
            stored = kiln_c99_store_get_key(host, target, key, ikey, out, depth, thrown, &found);
            if (stored != KILN_C99_OK || found) return stored;
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
                if (concrete && kiln_c99_same_name_key(key, "constructor")) {
                    *out = kiln_c99_builtin(kiln_c99_typed_constructor(typed_kind));
                    return KILN_C99_OK;
                }
                if (concrete && kiln_c99_same_name_key(key, "BYTES_PER_ELEMENT")) {
                    *out = kiln_c99_number((double)kiln_c99_typed_width(typed_kind));
                    return KILN_C99_OK;
                }
                {
                    int found;
                    int status = kiln_c99_prototype_store_read(host, target.bits, key,
                                                               ikey, out, depth, thrown, &found);
                    if (status != KILN_C99_OK || found) return status;
                    if (concrete) {
                        status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE,
                                                               key, ikey, out, depth, thrown, &found);
                        if (status != KILN_C99_OK || found) return status;
                    }
                }
                uint64_t method;
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
        if (code == 0) {
            int found = 0;
            int status = kiln_c99_prototype_store_read(host, target.bits,
                                                       key, ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
            if (target.bits != KILN_C99_BUILTIN_OBJECT_PROTOTYPE) {
                status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_OBJECT_PROTOTYPE,
                                                       key, ikey, out, depth, thrown, &found);
                if (status != KILN_C99_OK || found) return status;
            }
        }
        /* `RangeError.name` is how `error.constructor.name` gets its answer, which is the ordinary
         * way a program asks what kind of error it caught. A namespace is not a constructor and
         * has no name to give. */
        if (kiln_c99_same_name_key(key, "constructor")) {
            if (target.bits == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_TYPED_ARRAY);
                return KILN_C99_OK;
            }
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
                          {KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE, KILN_C99_BUILTIN_ARRAY_BUFFER},
                          {KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER_PROTOTYPE, KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER}};
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
        if (target.bits == KILN_C99_BUILTIN_DATE_PROTOTYPE &&
            !kiln_c99_is_symbol_key(intrinsic_key, intrinsic_len)) {
            if (kiln_c99_is_date_member(intrinsic_key, intrinsic_len)) {
                KILN_C99_REFUSE("a Date.prototype locale member this backend has not implemented");
            }
            /* date.ts installs every other Date.prototype member into this prototype's own
             * store, checked above. A remaining spelling such as the nonstandard
             * `getUTCTime` read by @date-fns/utc is genuinely absent. */
            KILN_C99_ABSENT("not a Date.prototype member; date.ts installs the complete supported \
                            Date.prototype surface", out);
        }
        if (kiln_c99_same_name_key(key, "prototype")) {
            if (target.bits == KILN_C99_BUILTIN_TYPED_ARRAY) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE);
                return KILN_C99_OK;
            }
            kiln_value store;
            if (kiln_c99_callable_store(target, &store, 0) == KILN_C99_OK &&
                kiln_c99_store_has(store, key, ikey))
                return kiln_c99_get_key(host, store, key, ikey, out, depth, thrown);
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
                {KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER, KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER_PROTOTYPE},
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
        if (target.bits == KILN_C99_BUILTIN_SET &&
            !kiln_c99_is_symbol_key(intrinsic_key, intrinsic_len)) {
            /* Set's constructor has no ordinary string-named statics beyond name, length and
             * prototype (handled above). In particular, feature tests such as kind-of's
             * `Set.isBuffer` must receive undefined instead of a backend refusal. */
            KILN_C99_ABSENT("Set constructor has no other string-named statics", out);
        }
        if (target.bits == KILN_C99_BUILTIN_BIGINT &&
            !kiln_c99_is_symbol_key(intrinsic_key, intrinsic_len)) {
            /* BigInt's only ordinary string statics are asIntN and asUintN, handled through the
             * shared surface above. Other names such as the feature probe BigInt.isBuffer are
             * absent by the complete constructor definition. */
            KILN_C99_ABSENT("BigInt constructor has no other string-named statics", out);
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
            if ((owned || kiln_c99_has_surface(target.bits)) &&
                !kiln_c99_backlog_names(kiln_c99_builtin_owner_name(target.bits), intrinsic_key, intrinsic_len)) {
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
                if (kiln_c99_same_name_key(key, "detached")) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Method get ArrayBuffer.prototype.detached called on incompatible receiver");
                }
                if (kiln_c99_same_name_key(key, "resizable")) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Method get ArrayBuffer.prototype.resizable called on incompatible receiver");
                }
                if (kiln_c99_same_name_key(key, "maxByteLength")) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Method get ArrayBuffer.prototype.maxByteLength called on incompatible receiver");
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
            if (target.bits == KILN_C99_BUILTIN_REGEXP_PROTOTYPE) {
                if (kiln_c99_same_name_key(key, "flags")) {
                    *out = kiln_c99_copy_text("", 0);
                    return KILN_C99_OK;
                }
                if (kiln_c99_same_name_key(key, "source")) {
                    *out = kiln_c99_copy_text("(?:)", 4);
                    return KILN_C99_OK;
                }
                if (kiln_c99_same_name_key(key, "@@toStringTag")) {
                    *out = kiln_c99_copy_text("RegExp", 6);
                    return KILN_C99_OK;
                }
            }
            if ((target.bits == KILN_C99_BUILTIN_REGEXP_PROTOTYPE && (
                    kiln_c99_same_name_key(key, "global") ||
                    kiln_c99_same_name_key(key, "ignoreCase") ||
                    kiln_c99_same_name_key(key, "multiline") ||
                    kiln_c99_same_name_key(key, "dotAll") ||
                    kiln_c99_same_name_key(key, "hasIndices") ||
                    kiln_c99_same_name_key(key, "unicode") ||
                    kiln_c99_same_name_key(key, "unicodeSets") ||
                    kiln_c99_same_name_key(key, "sticky") ||
                    kiln_c99_same_name_key(key, "name"))) ||
                (target.bits == KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER_PROTOTYPE &&
                    kiln_c99_same_name_key(key, "name")) ||
                (target.bits == KILN_C99_BUILTIN_ITERATOR_PROTOTYPE &&
                    kiln_c99_same_name_key(key, "name"))) {
                KILN_C99_ABSENT("not a member of this prototype: prototype is an object without a name", out);
            }
            if (target.bits == KILN_C99_BUILTIN_SET_PROTOTYPE && kiln_c99_same_name_key(key, "size")) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Method get Set.prototype.size called on incompatible receiver");
            }
            if (target.bits == KILN_C99_BUILTIN_MAP_PROTOTYPE && kiln_c99_same_name_key(key, "size")) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Method get Map.prototype.size called on incompatible receiver");
            }
            if (target.bits == KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER_PROTOTYPE) {
                if (kiln_c99_same_name_key(key, "constructor")) {
                    *out = kiln_c99_builtin(KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER);
                    return KILN_C99_OK;
                }
                if (kiln_c99_same_name_key(key, "byteLength")) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Method get SharedArrayBuffer.prototype.byteLength called on incompatible receiver");
                }
                if (kiln_c99_same_name_key(key, "maxByteLength")) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Method get SharedArrayBuffer.prototype.maxByteLength called on incompatible receiver");
                }
                if (kiln_c99_same_name_key(key, "growable")) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Method get SharedArrayBuffer.prototype.growable called on incompatible receiver");
                }
            }
            if (target.bits == KILN_C99_BUILTIN_OBJECT_PROTOTYPE && kiln_c99_same_name_key(key, "__proto__")) {
                *out = kiln_c99_null();
                return KILN_C99_OK;
            }
            if (target.bits == KILN_C99_BUILTIN_ITERATOR_PROTOTYPE) {
                /* Its TypeScript-installed extensions and Object.prototype members have already
                 * been read above, as have the methods returned by the shared iterator table.
                 * This prototype is a distinct node in Array/String/Map/Set iterator chains; a
                 * name absent from that authority is an ordinary absent property (not a missing
                 * method on a table-driven built-in). Keep the authority check explicit so a
                 * future helper added to the table cannot silently fall through here. */
                if (kiln_c99_iterator_prototype_method(intrinsic_key, intrinsic_len) != 0) {
                    KILN_C99_REFUSE("an Iterator.prototype table member not returned by its method lookup");
                }
                KILN_C99_ABSENT("not an Iterator.prototype member: absent from the shared helper table and installed store", out);
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
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_BIGINT_PROTOTYPE);
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
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_BIGINT_PROTOTYPE, key,
                                                       ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
            status = kiln_c99_primitive_object_prototype_read(host, key, ikey, intrinsic_key,
                                                              intrinsic_len, out, depth, thrown,
                                                              &found);
            if (status != KILN_C99_OK || found) return status;
        }
        KILN_C99_ABSENT("not a member of BigInt.prototype, its store or Object.prototype; "
                        "BUILTIN_SURFACE holds the ones that are",
                        out);
    }
    case KILN_VALUE_NUMBER: {
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_NUMBER_PROTOTYPE);
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
        {
            int found;
            int status = kiln_c99_prototype_store_read(host, KILN_C99_BUILTIN_NUMBER_PROTOTYPE, key,
                                                       ikey, out, depth, thrown, &found);
            if (status != KILN_C99_OK || found) return status;
            status = kiln_c99_primitive_object_prototype_read(host, key, ikey, intrinsic_key,
                                                              intrinsic_len, out, depth, thrown,
                                                              &found);
            if (status != KILN_C99_OK || found) return status;
        }
        KILN_C99_ABSENT("not a member of Number.prototype, its store or Object.prototype", out);
    }
    case KILN_VALUE_SYMBOL: {
        KILN_C99_WRITTEN_MEMBER_FIRST(KILN_C99_BUILTIN_SYMBOL_PROTOTYPE);
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
        {
            int found;
            int status;
            (void)kiln_c99_intrinsic_ascii_view(key, intrinsic_buffer, &intrinsic_key,
                                                &intrinsic_len);
            status = kiln_c99_primitive_object_prototype_read(host, key, ikey, intrinsic_key,
                                                              intrinsic_len, out, depth, thrown,
                                                              &found);
            if (status != KILN_C99_OK || found) return status;
        }
        KILN_C99_ABSENT("not a member of Symbol.prototype, its store or Object.prototype", out);
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
    if (kiln_c99_kiln_shared_hook_read(target, key, out)) return KILN_C99_OK;
    if (kiln_c99_is_node_hidden_key_key(key)) {
        KILN_C99_ABSENT("a hidden runtime slot, which no program names", out);
    }
    if (target.tag == KILN_VALUE_PROXY) {
        kiln_value key_value = kiln_c99_key_value(key, &ikey);
        if (key_value.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        return kiln_c99_proxy_get(host, target, key_value, target, out, depth, thrown);
    }
    status = kiln_c99_get_own_kind(host, target, key, ikey, out, depth, thrown);
    /* Namespace descriptors retain their export cell instead of copying its value. This makes
     * reads observe later writes and lets the same object be published before a cyclic module is
     * evaluated. */
    if (status == KILN_C99_OK && target.tag == KILN_VALUE_OBJECT &&
        kiln_c99_object_of(target)->module_namespace && out->tag == KILN_VALUE_CELL) {
        kiln_val *cell = kiln_c99_cell_of(*out);
        if (cell == NULL) KILN_C99_REFUSE("namespace export cell has no storage");
        status = kiln_c99_val_check_tdz(*cell, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_val_to_boundary(*cell);
    }
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
    view.flags = ikey != NULL ? KILN_STRING_LATIN1 | kiln_c99_key_domain(ikey)
                              : kiln_c99_fixed_key_flags(key, key_len);
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
    /* A sealed store projects values and visibility from realm-owned storage. No dictionary
     * index or shaped slot can identify one of its properties for this site. */
    if (object->sealed != NULL) return 0;
    if (kiln_c99_key_is_reserved(key) || kiln_c99_same_name_key(key, "__proto__")) {
        return 0;
    }
    if (object->shape != NULL && canonical != NULL) {
        uint32_t slot;
        if (kiln_c99_shape_find_slot(object->shape, canonical, &slot)) {
            /* An accessor's slot holds its pair, not a value a site may hand back. */
            if (object->shape->accessor_mask & (1u << slot)) return 0;
            if (kiln_c99_site_may_learn(site, object->shape)) {
                kiln_c99_site_remember_own(site, object->shape, slot);
            }
            return 1;
        }
    }
    for (index = 0; index < object->count && object->properties != NULL; ++index) {
        if (kiln_c99_same_key_view(&object->properties[index], key)) {
            kiln_c99_site_set_dictionary(site, kiln_c99_site_encode_dictionary_index(index), 0);
            return 1;
        }
    }
    if (object->prototype.tag == KILN_VALUE_OBJECT) {
        const kiln_object *parent = kiln_c99_object_of(object->prototype);
        if (parent->sealed != NULL) return 0;
        for (index = 0; index < parent->count && parent->properties != NULL; ++index) {
            if (kiln_c99_same_key_view(&parent->properties[index], key)) {
                kiln_c99_site_set_dictionary(site, 0, kiln_c99_site_encode_dictionary_index(index));
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
    if (kiln_c99_kiln_shared_hook_read(target, key, out)) return KILN_C99_OK;
    if (site == NULL || target.tag != KILN_VALUE_OBJECT) {
        return kiln_c99_get_key(host, target, key, canonical, out, depth, thrown);
    }
    {
        const kiln_object *object = kiln_c99_object_of(target);
        /* Namespace slots hold live binding cells. Ordinary dictionary hits return a slot's
         * raw payload; the namespace read must unwrap it and enforce the temporal dead zone. */
        if (object->sealed != NULL || object->module_namespace)
            return kiln_c99_get_key(host, target, key, canonical, out, depth, thrown);
        if (object->shape != NULL) {
            const kiln_c99_pic_entry *entry = kiln_c99_site_entry(site, object->shape);
            if (entry != NULL && entry->slot < object->slot_capacity) {
                *out = kiln_val_to_boundary(object->slots[entry->slot]);
                return KILN_C99_OK;
            }
            {
                uint32_t slot;
                if (canonical != NULL && kiln_c99_shape_find_slot(object->shape, canonical, &slot)) {
                    /* An own accessor: the generic read below calls it. */
                    if (object->shape->accessor_mask & (1u << slot)) goto generic;
                    if (slot < object->slot_capacity) {
                        *out = kiln_val_to_boundary(object->slots[slot]);
                        if (kiln_c99_site_count(site) < KILN_C99_PIC_SIZE) {
                            kiln_c99_site_remember_own(site, object->shape, slot);
                        }
                        return KILN_C99_OK;
                    }
                }
            }
            if (object->properties == NULL) {
                kiln_value current = object->prototype;
                while (current.tag == KILN_VALUE_OBJECT) {
                    const kiln_object *proto_obj = kiln_c99_object_of(current);
                    if (proto_obj->sealed != NULL) goto generic;
                    if (proto_obj->shape != NULL) {
                        uint32_t proto_slot;
                        if (canonical != NULL &&
                            kiln_c99_shape_find_slot(proto_obj->shape, canonical, &proto_slot)) {
                            if (proto_obj->shape->accessor_mask & (1u << proto_slot)) break;
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
        uint32_t own = kiln_c99_site_decode_dictionary_index(kiln_c99_site_own(site));
        if (own < object->count && object->properties != NULL &&
            kiln_c99_same_key_view(&object->properties[own], key)) {
            if (!object->properties[own].accessor) {
                *out = object->properties[own].value;
                return KILN_C99_OK;
            }
        } else if (kiln_c99_site_proto(site) != 0 &&
                   object->prototype.tag == KILN_VALUE_OBJECT) {
            size_t index = 0;
            while (index < object->count && object->properties != NULL &&
                   !kiln_c99_same_key_view(&object->properties[index], key)) ++index;
            if (index == object->count) {
                const kiln_object *parent = kiln_c99_object_of(object->prototype);
                if (parent->sealed != NULL) goto generic;
                uint32_t at = kiln_c99_site_decode_dictionary_index(kiln_c99_site_proto(site));
                if (parent->properties != NULL && at < parent->count &&
                    kiln_c99_same_key_view(&parent->properties[at], key) &&
                    !parent->properties[at].accessor) {
                    *out = parent->properties[at].value;
                    return KILN_C99_OK;
                }
            }
        }
    generic:
        status = kiln_c99_get_key(host, target, key, canonical, out, depth, thrown);
        if (status == KILN_C99_OK && kiln_c99_site_misses(site) < KILN_C99_SITE_GIVE_UP &&
            !kiln_c99_site_learn(site, object, key, canonical)) {
            kiln_c99_site_missed(site);
        }
        return status;
    }
}

/* ---- Tier 2 of the parallel built-ins: the reads a pool thread makes (parallel issue 07) ----
 *
 * Out-of-line halves of the readers in `prelude_types.h`. Each answers what the ordinary read
 * (`kiln_c99_get_own_kind` above, `kiln_c99_val_read_property_key`) answers for the same value
 * and key, or declines with 0 wherever that read could run code, allocate or write -- and where
 * it gives an answer this walk does not reproduce exactly. Nothing here writes: no cache is
 * taught (the worker's own site cache is on its stack), no key is interned
 * (`kiln_c99_interned_key_in` only looks), no object is converted to dictionary mode. */

/* A name that falls off the end of an ordinary object's chain. The ordinary read answers some
 * names there by kind -- `constructor`, `__proto__` (an accessor), `Object.prototype`'s methods,
 * whatever the realm's `Object.prototype` store holds -- and those decline; any other name is
 * absent, which is undefined. */
static int kiln_c99_par_absent(const kiln_string *key, const kiln_string *ikey, kiln_val *out) {
    char buffer[256];
    const char *bytes = NULL;
    size_t len = 0;
    if (kiln_c99_same_name_key(key, "constructor") || kiln_c99_same_name_key(key, "__proto__")) {
        return 0;
    }
    if (kiln_c99_intrinsic_ascii_view(key, buffer, &bytes, &len) &&
        kiln_c99_namespace_method(KILN_C99_BUILTIN_OBJECT_PROTOTYPE, bytes, len) != 0) {
        return 0;
    }
    /* A key the realm never interned is in no store; one it did, the store is scanned live. */
    if (!kiln_c99_object_prototype_unmodified() && ikey != NULL &&
        !kiln_c99_object_prototype_lacks(ikey)) {
        return 0;
    }
    *out = KILN_VAL_UNDEFINED;
    return 1;
}

/* `object[key]` by name, over ordinary objects only: each level's own data property, then its
 * prototype, to the end of the chain. `ikey` is the interned key, or NULL for a spelling the realm
 * never interned (which no object stores). A level that is not an ordinary object, an accessor, a
 * refused member, a shaped object with a property list beside its shape: decline. `site`, when
 * given, learns the receiver's own data slot. */
static int kiln_c99_par_walk(kiln_c99_par_site *site, kiln_val object, const kiln_string *ikey,
                             const kiln_string *key, kiln_val *out) {
    kiln_val current = object;
    size_t steps;
    if (key == NULL || kiln_c99_key_is_reserved(key) || kiln_c99_key_index_key(key) >= 0.0) {
        return 0;
    }
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        const kiln_object *obj;
        if (!KILN_VAL_IS_OBJECT(current)) return 0;
        obj = (const kiln_object *)KILN_VAL_PTR(current);
        if (kiln_c99_gc_payload_kind(obj) != KILN_GC_OBJECT || obj->arguments_brand ||
            obj->error_brand || obj->module_namespace) {
            return 0;
        }
        if (obj->shape != NULL) {
            uint32_t slot;
            if (ikey != NULL && kiln_c99_shape_find_slot(obj->shape, ikey, &slot)) {
                if (slot >= obj->slot_capacity || slot >= 32u ||
                    (obj->shape->accessor_mask & (1u << slot)) != 0) {
                    return 0;
                }
                *out = obj->slots[slot];
                if (steps == 0 && site != NULL) {
                    site->shape = obj->shape;
                    site->slot = slot;
                }
                return 1;
            }
            /* The ordinary read converts such an object to a list first; that is a write. */
            if (obj->properties != NULL) return 0;
        } else {
            size_t index;
            for (index = 0; index < obj->count && obj->properties != NULL; ++index) {
                const kiln_property *property = &obj->properties[index];
                if (property->key == ikey || kiln_c99_same_key_view(property, key)) {
                    if (property->refused || property->accessor) return 0;
                    *out = kiln_val_from_boundary(property->value);
                    return 1;
                }
            }
        }
        switch (obj->prototype.tag) {
        case KILN_VALUE_OBJECT:
            current = kiln_val_from_boundary(obj->prototype);
            continue;
        case KILN_VALUE_NULL:
            /* `Object.create(null)` itself answers every absent name undefined. */
            if (steps == 0) {
                *out = KILN_VAL_UNDEFINED;
                return 1;
            }
            return kiln_c99_par_absent(key, ikey, out);
        case KILN_VALUE_UNDEFINED:
            return kiln_c99_par_absent(key, ikey, out);
        default:
            return 0;
        }
    }
    return 0;
}

static int kiln_c99_par_get_slow(const kiln_c99_par_env *env, kiln_c99_par_site *site,
                                 kiln_val object, const kiln_string *ikey, kiln_val *out) {
    (void)env;
    return kiln_c99_par_walk(site, object, ikey, ikey, out);
}

/* `object[key]` when the inline half did not answer. A number key: an array's hole or a key past
 * its end (absent, and neither `Array.prototype` nor `Object.prototype` names an index while the
 * built-ins are unmodified, which the call site checked), a view's key outside it (undefined), a
 * string's code unit (a one-unit string from the static table, below 256) or a key past it, an
 * ordinary object's dense element. A string key: an ordinary object's property by name. */
static int kiln_c99_par_index_slow(const kiln_c99_par_env *env, kiln_val object, kiln_val key,
                                   kiln_val *out) {
    if (KILN_VAL_IS_NUMBER(key)) {
        double d;
        memcpy(&d, &key, sizeof d);
        if ((object & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {
            const void *payload = KILN_VAL_PTR(object);
            uint32_t kind = kiln_c99_gc_payload_kind(payload);
            if (kind == KILN_GC_ARRAY) {
                const kiln_array *arr = (const kiln_array *)payload;
                /* A number names an element, or a property whose name is a number's spelling;
                 * an array with no named properties holds neither beyond its elements. */
                if (arr->properties.tag != KILN_VALUE_UNDEFINED) return 0;
                if (d >= 0.0 && d < (double)arr->count && d == floor(d) &&
                    arr->elements[(size_t)d] != KILN_VAL_HOLE) {
                    *out = arr->elements[(size_t)d];
                    return 1;
                }
                *out = KILN_VAL_UNDEFINED;
                return 1;
            }
            if (kind == KILN_GC_TYPED_ARRAY) {
                const kiln_typed_array *view = (const kiln_typed_array *)payload;
                if (view->kind > KILN_TYPED_FLOAT64) return 0;
                /* A number is always a canonical numeric key: an integer index inside the view
                 * reads it, every other reads undefined, never the prototype. */
                if (!view->buffer->detached && d >= 0.0 && d < (double)view->length &&
                    d == floor(d)) {
                    *out = kiln_c99_val_from_double(kiln_typed_load(view, (size_t)d));
                    return 1;
                }
                *out = KILN_VAL_UNDEFINED;
                return 1;
            }
            return 0;
        }
        if (KILN_VAL_IS_STRING(object)) {
            const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(object);
            if (d >= 0.0 && d < (double)str->unit_len && d == floor(d)) {
                uint16_t unit = kiln_string_unit_at(str, (size_t)d);
                if (unit > 0xFF) return 0;
                *out = kiln_c99_val_single_char((unsigned char)unit);
                return 1;
            }
            *out = KILN_VAL_UNDEFINED;
            return 1;
        }
        if (KILN_VAL_IS_OBJECT(object)) {
            const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(object);
            if (kiln_c99_gc_payload_kind(obj) == KILN_GC_OBJECT && obj->shape != NULL &&
                !obj->arguments_brand && !obj->error_brand && !obj->module_namespace &&
                obj->elements != NULL && d >= 0.0 && d < (double)obj->element_count &&
                d < 100000.0 && d == floor(d) && obj->elements[(size_t)d] != KILN_VAL_HOLE) {
                *out = obj->elements[(size_t)d];
                return 1;
            }
        }
        return 0;
    }
    if (KILN_VAL_IS_STRING(key) && KILN_VAL_IS_OBJECT(object)) {
        const kiln_string *name = (const kiln_string *)KILN_VAL_PTR(key);
        return kiln_c99_par_walk(NULL, object, kiln_c99_interned_key_in(env->heap, name), name,
                                 out);
    }
    return 0;
}

/* On the realm thread, before the pool starts: build what the readers above reach lazily (the
 * built-in member index `kiln_c99_namespace_method` searches), and answer the heap they read. */
static struct kiln_gc_heap *kiln_c99_par_prepare(void) {
    int owned = 0;
    (void)kiln_c99_surface_method(KILN_C99_BUILTIN_OBJECT_PROTOTYPE, NULL, 0, &owned);
    return (struct kiln_gc_heap *)kiln_c99_gc_heap();
}

/* On the realm thread, before the pool starts (tier 3): the canonical key of a member a tier-3
 * callback defines, interned now as the sequential definition would intern it, so a worker never
 * has to. NULL only when memory is refused; the site then runs sequentially. */
static const kiln_string *kiln_c99_par_intern(const kiln_string *key) {
    return kiln_c99_intern_key(key);
}
