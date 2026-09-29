/* Makes room for `needed` slots, exactly as the generic per-key path grows: doubling from the
 * inline slots or from whatever heap array is there, new slots undefined. */
static int kiln_c99_grow_slots(kiln_object *dest, size_t needed) {
    size_t new_cap;
    kiln_val *new_slots;
    size_t fill_i;
    if (needed <= dest->slot_capacity) {
        return KILN_C99_OK;
    }
    new_cap = dest->slot_capacity == 0 ? KILN_C99_INLINE_SLOTS : dest->slot_capacity * 2;
    while (needed > new_cap) new_cap *= 2;
    new_slots = kiln_c99_object_slots_resize(dest, new_cap);
    if (new_slots == NULL) return KILN_C99_OUT_OF_MEMORY;
    for (fill_i = dest->slot_capacity; fill_i < new_cap; ++fill_i) {
        new_slots[fill_i] = KILN_VAL_UNDEFINED;
    }
    dest->slots = new_slots;
    dest->slot_capacity = new_cap;
    return KILN_C99_OK;
}

/* The bridge for `DefineNamedProperty` (tail.rs, the only caller): `{ ...base, f: i, g: i * 2 }`
 * defines `f` and `g` straight onto the literal's own accumulator, which the generic path (a
 * boundary round trip through `kiln_value` for both the key and the value, `kiln_c99_property_key`,
 * `kiln_c99_set_function_name`, then `kiln_c99_put` interning the key again and walking the whole
 * target-kind ladder -- proxy, function, method, array, collection, promise -- before it even
 * reaches the shape it is going to write into) pays for on every key of every object literal.
 *
 * Returns 1 when it fully handled the definition (`*status` says whether that succeeded), 0 to
 * fall back to the generic path unchanged. A miss is the common shape of "this write needs the
 * generic path's care" -- a dictionary-mode or non-extensible or frozen or accessor-bearing
 * object, a key the shape tree refuses (the fast-property limit), `__proto__` (which the generic
 * path relinks the prototype for instead of defining, and only when the key is *this* literal
 * spelling of it -- computed and symbol keys never reach here to begin with), or a value that is
 * a function and so needs `kiln_c99_set_function_name`'s attention -- and is never a correctness
 * problem: every one of them is a plain `return 0` before anything is mutated.
 *
 * `target` is trusted to be the literal's own fresh object without asking `kiln_c99_gc_kind_of_ptr`
 * -- the compiler emits `DefineNamedProperty` nowhere but here (`kiln_ir::compile::object_literal`),
 * always immediately after `NewObject` or `SpreadObject` built the very object on top of the stack
 * -- but the kind is still confirmed with `kiln_c99_gc_payload_kind`'s pointer arithmetic rather
 * than trusted blindly, because a wrong guess here would read a `kiln_promise` or `kiln_collection`
 * payload as a `kiln_object` and corrupt it: cheap enough not to be worth skipping, unlike the
 * slab-table lookup `kiln_c99_gc_kind_of_ptr` would otherwise cost on every key. */
/* The site remembers the one thing this operation computes: which shape the target arrived
 * with, and which shape and slot the key sends it to. The transition tree is append-only and
 * its shapes are never freed, so `(shape, key) -> (next shape, slot)` is a pure function and
 * the memo holds for as long as the program runs; the key itself need not be compared, because
 * a site's key is a literal that never changes. A miss -- another shape at the same site --
 * takes the general path below and re-teaches the site. */
static inline int kiln_c99_define_named_cached(kiln_object *object, kiln_val value,
                                               const kiln_c99_site *site) {
    const kiln_c99_pic_entry *entry;
    if (site == NULL || object->shape == NULL) return 0;
    entry = kiln_c99_site_entry(site, object->shape);
    if (entry == NULL) return 0;
    if (entry->transition_to == NULL) {
        if (entry->slot >= object->slot_capacity) return 0;
        object->slots[entry->slot] = value;
        return 1;
    }
    if (entry->slot >= object->slot_capacity &&
        kiln_c99_grow_slots(object, (size_t)entry->slot + 1) != KILN_C99_OK) {
        return 0;
    }
    object->slots[entry->slot] = value;
    object->shape = (kiln_shape *)entry->transition_to;
    return 1;
}

static inline void kiln_c99_define_named_teach(kiln_c99_site *site, const kiln_shape *from,
                                               const kiln_shape *to, uint32_t slot) {
    if (site == NULL) return;
    kiln_c99_site_remember_transition(site, from, to, slot);
}

static int kiln_c99_define_named_fast(kiln_val target, kiln_val key, kiln_val value, int *status,
                                      kiln_c99_site *site) {
    kiln_object *object;
    const kiln_string *keystr;
    const kiln_string *ikey;
    uint32_t slot;
    kiln_shape *next;

    if (!KILN_VAL_IS_STRING(key) || KILN_VAL_IS_CLOSURE(value) || !KILN_VAL_IS_OBJECT(target)) {
        return 0;
    }
    object = (kiln_object *)KILN_VAL_PTR(target);
    if (object == NULL || kiln_c99_gc_payload_kind(object) != KILN_GC_OBJECT) {
        return 0;
    }
    if (object->shape == NULL || object->properties != NULL || object->frozen ||
        object->not_extensible || object->accessor_count != 0) {
        return 0;
    }
    kiln_c99_prototype_touched(object);
    if (kiln_c99_define_named_cached(object, value, site)) {
        *status = KILN_C99_OK;
        return 1;
    }
    keystr = (const kiln_string *)KILN_VAL_PTR(key);
    if (keystr == NULL || keystr->unit_len == 0) {
        return 0;
    }
    if (kiln_c99_key_is_reserved(keystr)) {
        /* A `@@`-prefixed key must come out non-enumerable (own_slot_ikey's rule), which the
         * generic path gets right; not worth teaching this one. Reachable only via a computed
         * key that happens to spell one -- a literal key can never start with `@`. */
        return 0;
    }
    if (kiln_c99_same_name_key(keystr, "__proto__")) {
        /* This literal spelling of the key relinks the prototype instead of defining a property
         * (kiln_c99_put_mode_ikey's own `__proto__` special case, reached the same way whether
         * this fast path exists or not); leave it there rather than duplicate the check. */
        return 0;
    }
    ikey = kiln_c99_intern_key(keystr);
    if (ikey == NULL) {
        *status = KILN_C99_OUT_OF_MEMORY;
        return 1;
    }
    if (kiln_c99_shape_find_slot(object->shape, ikey, &slot)) {
        /* Redefining an attribute-bearing slot changes its attributes: the generic path's. */
        if (slot >= object->slot_capacity || (object->shape->special_mask & (1u << slot)) != 0) {
            return 0;
        }
        kiln_c99_define_named_teach(site, object->shape, NULL, slot);
        object->slots[slot] = value;
        *status = KILN_C99_OK;
        return 1;
    }
    int is_double = KILN_VAL_IS_NUMBER(value);
    if (kiln_c99_shape_transition(object->shape, ikey, is_double, &next)) {
        if ((size_t)next->slot_index >= object->slot_capacity) {
            int grown = kiln_c99_grow_slots(object, (size_t)next->slot_index + 1);
            if (grown != KILN_C99_OK) {
                *status = grown;
                return 1;
            }
        }
        kiln_c99_define_named_teach(site, object->shape, next, next->slot_index);
        object->slots[next->slot_index] = value;
        object->shape = next;
        *status = KILN_C99_OK;
        return 1;
    }
    /* The shape tree refused the transition (the fast-property limit): the generic path's
     * `kiln_c99_ensure_dictionary` is what a key past that point needs. */
    return 0;
}

/* The bridge for `SpreadObject` (tail.rs, the only caller): `{ ...base }` where both `base` and
 * the literal being built are the shape's fresh, empty-starting-point case that
 * `kiln_c99_spread_object` already special-cases below (see the comment on that branch) --
 * everything this checks duplicates that branch's conditions exactly, on raw `kiln_val`s instead
 * of the boundary `kiln_value` `SpreadObject`'s generic path converts both operands through
 * first. That conversion is one `kiln_val_to_boundary` per operand, and for an object each of
 * those resolves the payload's exact kind via `kiln_c99_gc_kind_of_ptr`'s slab-table lookup --
 * work this bulk-copy case never needed, since it only ever wants to know "is this a plain
 * KILN_GC_OBJECT", which `kiln_c99_gc_payload_kind`'s pointer arithmetic answers directly.
 *
 * Returns 1 when it fully handled the spread (`*status` says whether that succeeded), 0 to fall
 * back to the generic `kiln_c99_spread_object` unchanged -- which every case this does not
 * recognize needs, including a source that is not a plain object at all (an array, a string, a
 * function, a proxy, `undefined`), so a miss here is never a correctness question, only which
 * function does the (still fully re-checked) work. */
static int kiln_c99_spread_object_fast(kiln_val target, kiln_val source, int *status) {
    kiln_object *object;
    kiln_object *dest;
    kiln_shape *sshape;
    size_t scount;
    size_t k;

    if (!KILN_VAL_IS_OBJECT(source) || !KILN_VAL_IS_OBJECT(target)) {
        return 0;
    }
    object = (kiln_object *)KILN_VAL_PTR(source);
    if (object == NULL || kiln_c99_gc_payload_kind(object) != KILN_GC_OBJECT) {
        return 0;
    }
    dest = (kiln_object *)KILN_VAL_PTR(target);
    if (dest == NULL || kiln_c99_gc_payload_kind(dest) != KILN_GC_OBJECT) {
        return 0;
    }
    if (object->properties != NULL || object->shape == NULL || object->elements != NULL ||
        object->accessor_count != 0) {
        return 0;
    }
    kiln_c99_prototype_touched(dest);
    if (dest->properties != NULL || dest->shape == NULL || dest->shape->property_count != 0 ||
        dest->elements != NULL || dest->frozen || dest->not_extensible ||
        dest->accessor_count != 0) {
        return 0;
    }
    sshape = object->shape;
    scount = sshape->property_count;
    if (sshape->special_mask != 0) {
        /* A copy is plain data: a hidden, read-only or accessor slot is the per-key loop's. */
        return 0;
    }
    if (sshape->has_symbol_key) {
        /* A symbol key needs the validation kiln_c99_spread_object's per-key loop gives it. */
        return 0;
    }
    if (scount > dest->slot_capacity) {
        int grown = kiln_c99_grow_slots(dest, scount);
        if (grown != KILN_C99_OK) {
            *status = grown;
            return 1;
        }
    }
    if (scount > 0) {
        memcpy(dest->slots, object->slots, scount * sizeof(kiln_val));
    }
    dest->shape = sshape;
    *status = KILN_C99_OK;
    return 1;
}

/* `__kilnCloneData(value)`: the structured clone of a value made only of plain data, or
 * `undefined` for anything else (ADR 0065, a *measured* primitive; ADR 0060 kind 5 beside the
 * authoritative clone in `kiln-builtins/src/structured_clone.ts`, which calls this first and
 * takes over whenever it answers `undefined`).
 *
 * "Plain data" is exactly the part of the algorithm with no observable step: an ordinary object
 * with `Object.prototype` as its prototype, in shape mode (so every own property is an
 * enumerable, writable data property with a string key, and none is an index), and an array with
 * no named properties; their leaves are primitives, which are immutable and shared. Nothing here
 * can run program code, so a bail half way through has done nothing a program can see, and the
 * TypeScript clone that runs next starts from the beginning. Shared references and cycles map
 * through a table of source to copy, as `seen` does there.
 *
 * Measured on `bench/concurrency/js/inrealm.mjs`: a ~1 MB graph (6,000 records sharing 13) took
 * 35 ms through the TypeScript walk against node's 5 ms; a small record 3.9 us against 1.4 us.
 *
 * The copies are reachable from this walk's C frames (the collector scans the machine stack) or
 * from a copy that is, from the moment they are made until the root is returned. */
/* A message is usually a handful of objects, so the table starts on the C stack and moves to
 * the heap only past `KILN_C99_CLONE_INLINE` entries. */
#define KILN_C99_CLONE_INLINE 32

typedef struct kiln_c99_clone_arena {
    kiln_c99_par_arena storage;
    kiln_gc_block *large_head;
    size_t large_count;
    size_t large_bytes;
    void **side_bases;
    size_t side_count;
    size_t side_capacity;
    kiln_c99_graph_capsule capsule;
    int certified;
    int adopted;
} kiln_c99_clone_arena;

typedef struct kiln_c99_clone_table {
    kiln_val *keys;
    kiln_val *values;
    size_t capacity;
    size_t count;
    int candidate_graph;
    kiln_c99_clone_arena *arena;
    kiln_val inline_keys[KILN_C99_CLONE_INLINE];
    kiln_val inline_values[KILN_C99_CLONE_INLINE];
} kiln_c99_clone_table;

#define KILN_C99_CLONE_MAX_DEPTH 512

static size_t kiln_c99_clone_hash(kiln_val key, size_t mask) {
    uint64_t h = (uint64_t)key * UINT64_C(0x9E3779B97F4A7C15);
    return (size_t)(h >> 17) & mask;
}

static int kiln_c99_clone_table_put(kiln_c99_clone_table *table, kiln_val key, kiln_val value) {
    size_t at;
    if ((table->count + 1) * 2 > table->capacity) {
        size_t grown = table->capacity * 2;
        kiln_val *keys = (kiln_val *)calloc(grown, sizeof *keys);
        kiln_val *values = (kiln_val *)malloc(grown * sizeof *values);
        size_t i;
        if (keys == NULL || values == NULL) {
            free(keys);
            free(values);
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (i = 0; i < table->capacity; ++i) {
            if (table->keys[i] != 0) {
                size_t slot = kiln_c99_clone_hash(table->keys[i], grown - 1);
                while (keys[slot] != 0) slot = (slot + 1) & (grown - 1);
                keys[slot] = table->keys[i];
                values[slot] = table->values[i];
            }
        }
        if (table->keys != table->inline_keys) {
            free(table->keys);
            free(table->values);
        }
        table->keys = keys;
        table->values = values;
        table->capacity = grown;
    }
    at = kiln_c99_clone_hash(key, table->capacity - 1);
    while (table->keys[at] != 0) at = (at + 1) & (table->capacity - 1);
    table->keys[at] = key;
    table->values[at] = value;
    table->count += 1;
    return KILN_C99_OK;
}

static int kiln_c99_clone_table_get(const kiln_c99_clone_table *table, kiln_val key, kiln_val *out) {
    size_t at;
    if (table->capacity == 0) return 0;
    at = kiln_c99_clone_hash(key, table->capacity - 1);
    while (table->keys[at] != 0) {
        if (table->keys[at] == key) {
            *out = table->values[at];
            return 1;
        }
        at = (at + 1) & (table->capacity - 1);
    }
    return 0;
}

static int kiln_c99_clone_arena_track_side(kiln_c99_clone_arena *arena, void *base) {
    if (arena->side_count == arena->side_capacity) {
        size_t capacity = arena->side_capacity == 0 ? 4 : arena->side_capacity * 2;
        void **grown;
        if (capacity < arena->side_capacity || capacity > SIZE_MAX / sizeof *grown)
            return KILN_C99_OUT_OF_MEMORY;
        grown = (void **)realloc(arena->side_bases, capacity * sizeof *grown);
        if (grown == NULL) return KILN_C99_OUT_OF_MEMORY;
        arena->side_bases = grown;
        arena->side_capacity = capacity;
    }
    arena->side_bases[arena->side_count++] = base;
    return KILN_C99_OK;
}

/* A string too large for a nursery chunk stays in the same private ownership group. The
 * receiver adopts its GC block alongside the chunks; before that, only this arena can free it.
 * Charge the full allocation against the arena's budget, including its block header. */
static kiln_string *kiln_c99_clone_arena_large_string(kiln_c99_clone_arena *arena,
                                                       size_t size) {
    kiln_gc_block *block;
    size_t total;
    if (size > SIZE_MAX - sizeof *block || arena->large_count == SIZE_MAX ||
        arena->large_bytes > SIZE_MAX - size) return NULL;
    total = sizeof *block + size;
    if (arena->storage.taken > arena->storage.budget ||
        total > arena->storage.budget - arena->storage.taken) return NULL;
    block = (kiln_gc_block *)malloc(total);
    if (block == NULL) return NULL;
    block->kind = KILN_GC_STRING | KILN_GC_FLAG_LARGE;
    block->mark = 0;
    block->size = size;
    block->prev = NULL;
    block->next = arena->large_head;
    if (block->next != NULL) block->next->prev = block;
    arena->large_head = block;
    arena->large_count += 1;
    arena->large_bytes += size;
    arena->storage.budget -= total;
    return (kiln_string *)(block + 1);
}

/* Before adoption the builder owns every side buffer and chunk. After adoption each array's
 * finalizer owns its side buffer and the destination heap owns the chunks. */
static void kiln_c99_clone_arena_dispose(kiln_c99_clone_arena *arena) {
    size_t index;
    if (arena->adopted && arena->storage.current != NULL) abort();
    if (!arena->adopted)
        for (index = 0; index < arena->side_count; ++index) free(arena->side_bases[index]);
    free(arena->side_bases);
    arena->side_bases = NULL;
    arena->side_count = arena->side_capacity = 0;
    if (!arena->adopted) {
        kiln_gc_block *block = arena->large_head;
        while (block != NULL) {
            kiln_gc_block *next = block->next;
            free(block);
            block = next;
        }
        kiln_c99_par_arena_seal(&arena->storage);
        kiln_c99_arena_chunks_free(arena->storage.filled);
    }
    arena->storage.filled = NULL;
    if (arena->certified) kiln_c99_graph_capsule_dispose(&arena->capsule);
    memset(arena, 0, sizeof *arena);
}

static int kiln_c99_clone_arena_object(kiln_c99_clone_arena *arena, size_t capacity,
                                       kiln_value *out) {
    size_t tail = capacity > KILN_C99_INLINE_SLOTS ? capacity : 0;
    kiln_object *object;
    if (capacity > KILN_SHAPE_MAX_FAST_PROPERTIES) return KILN_C99_CLONE_BAIL;
    if (tail > (SIZE_MAX - sizeof *object) / sizeof(kiln_val)) return KILN_C99_OUT_OF_MEMORY;
    object = (kiln_object *)kiln_c99_par_alloc(&arena->storage, KILN_GC_OBJECT,
                                               sizeof *object + tail * sizeof(kiln_val));
    if (object == NULL) return KILN_C99_OUT_OF_MEMORY;
    kiln_c99_object_init(object, tail);
    *out = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)object, NULL, 0);
    return KILN_C99_OK;
}

/* Dictionary keys can be created at run time and therefore cannot be named by a process shape.
 * Keep their fixed property records inside the movable block; its keys and values are copied
 * into the same private arena below. Growth after adoption moves only the records to malloc. */
static int kiln_c99_clone_arena_dictionary(kiln_c99_clone_arena *arena, size_t count,
                                           kiln_value *out) {
    kiln_object *object;
    if (count == 0 || count > (SIZE_MAX - sizeof *object) / sizeof(kiln_property))
        return KILN_C99_CLONE_BAIL;
    object = (kiln_object *)kiln_c99_par_alloc(&arena->storage, KILN_GC_OBJECT,
                                               sizeof *object + count * sizeof(kiln_property));
    if (object == NULL) return KILN_C99_OUT_OF_MEMORY;
    kiln_c99_object_init(object, 0);
    object->shape = NULL;
    object->properties = (kiln_property *)(object + 1);
    object->count = count;
    object->capacity = count;
    *out = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)object, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_clone_arena_array(kiln_c99_clone_arena *arena, size_t capacity,
                                      int is_numeric, kiln_value *out) {
    size_t inline_capacity = sizeof(((kiln_array *)0)->inline_elements) / sizeof(kiln_val);
    size_t in_block = capacity > inline_capacity && capacity <= KILN_C99_ARRAY_INBLOCK_MAX
                          ? capacity : 0;
    kiln_array *array;
    array = (kiln_array *)kiln_c99_par_alloc(&arena->storage, KILN_GC_ARRAY,
                                             sizeof *array + in_block * sizeof(kiln_val));
    if (array == NULL) return KILN_C99_OUT_OF_MEMORY;
    kiln_c99_array_init(array, is_numeric);
    if (in_block != 0) {
        array->elements = (kiln_val *)(array + 1);
        array->capacity = in_block;
    } else if (capacity != 0 && capacity <= inline_capacity) {
        array->elements = array->inline_elements;
        array->capacity = inline_capacity;
    } else if (capacity != 0) {
        kiln_c99_candidate_array_elements *header;
        int status;
        if (capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_val))
            return KILN_C99_OUT_OF_MEMORY;
        header = (kiln_c99_candidate_array_elements *)malloc(
            sizeof *header + capacity * sizeof(kiln_val));
        if (header == NULL) return KILN_C99_OUT_OF_MEMORY;
        status = kiln_c99_clone_arena_track_side(arena, header);
        if (status != KILN_C99_OK) {
            free(header);
            return status;
        }
        header->candidate_id = arena->storage.candidate_id;
        header->owner = array;
        header->capacity = capacity;
        header->generation = 1;
        array->elements = (kiln_val *)(header + 1);
        array->capacity = capacity;
        array->candidate_elements = 1;
    }
    *out = kiln_c99_value(KILN_VALUE_ARRAY, (uint64_t)(uintptr_t)array, NULL, 0);
    return KILN_C99_OK;
}

/* Exactly the representation whose reads cannot invoke program code. The clone and the Rust
 * message encoder's native descriptor callback share this guard. */
static int kiln_c99_clone_safe_object(const kiln_object *source) {
    kiln_shape *shape = source->shape;
    size_t index;
    /* Sealed intrinsic stores are not plain-message graphs: their function values and process
     * descriptors must never be encoded as an empty realm-owned dictionary. */
    if (source->sealed != NULL || source->elements != NULL || source->accessor_count != 0 ||
        (source->prototype.tag != KILN_VALUE_UNDEFINED &&
         source->prototype.tag != KILN_VALUE_NULL) || source->arguments_brand ||
        source->error_brand || source->module_namespace ||
        (shape != NULL && (source->properties != NULL || shape->has_symbol_key ||
                           shape->special_mask != 0 ||
                           shape->property_count > source->slot_capacity ||
                           (shape->property_count != 0 && source->slots == NULL)))) return 0;
    if (shape == NULL) {
        if (source->count > source->capacity ||
            (source->count != 0 && source->properties == NULL)) return 0;
        for (index = 0; index < source->count; ++index) {
            const kiln_property *property = &source->properties[index];
            if (property->key == NULL ||
                (property->key->flags & (KILN_STRING_SYMBOL_KEY |
                                         KILN_STRING_PRIVATE_KEY)) != 0 ||
                !property->enumerable || !property->writable ||
                !property->configurable || property->accessor || property->refused) return 0;
        }
    }
    return 1;
}

static int kiln_c99_clone_safe_array(const kiln_array *source) {
    if (source->properties.tag != KILN_VALUE_UNDEFINED || source->match_index != -1) return 0;
    /* A hole is part of the array's own layout, not an edge to traverse. The copy preserves
     * KILN_VAL_HOLE, and its receiver still answers `index in array` as false. */
    return 1;
}

/* Actor messages need to find transfer tickets before posting. For a native graph containing
 * only plain data, the sender's TypeScript walk would make an entire temporary copy and the
 * provider message codec would then copy it again. This read-only certificate lets the actor
 * pass the original to that codec. A rejected graph takes the existing ticket-aware walk; in
 * particular, accessors and Proxy traps must not run during this check and then run again when
 * the message is encoded. The seen set handles shared references and cycles without changing
 * their identity. No kiln allocation or collection occurs while inspecting these raw slots. */
static int kiln_c99_clone_plain_val_for_message(kiln_val value, kiln_gc_ptrset *seen,
                                                 unsigned depth) {
    void *ptr;
    size_t index;
    if (KILN_VAL_IS_NUMBER(value) || KILN_VAL_IS_IMMEDIATE(value) ||
        KILN_VAL_IS_STRING(value)) return 1;
    if ((value & KILN_TAG_MASK) == KILN_TAG_BIGINT_PREFIX &&
        (value & UINT64_C(0xFFFF800000000000)) == KILN_TAG_BIGINT_PREFIX) return 1;
    if (depth > KILN_C99_CLONE_MAX_DEPTH ||
        (!KILN_VAL_IS_OBJECT(value) &&
         (value & KILN_TAG_MASK) != KILN_TAG_BUFFER_PREFIX)) return 0;
    ptr = KILN_VAL_PTR(value);
    if (ptr == NULL) return 0;
    if (kiln_c99_gc_ptrset_contains(seen, ptr)) return 1;
    if (KILN_VAL_IS_OBJECT(value)) {
        const kiln_object *object = (const kiln_object *)ptr;
        const kiln_shape *shape;
        if (kiln_c99_gc_payload_kind(ptr) != KILN_GC_OBJECT ||
            !kiln_c99_clone_safe_object(object) || object->shape == NULL ||
            object->properties != NULL) return 0;
        shape = object->shape;
        if (shape->property_count > object->slot_capacity ||
            (shape->property_count != 0 && object->slots == NULL) ||
            shape->accessor_mask != 0) return 0;
        if (kiln_c99_gc_ptrset_insert(seen, ptr) != KILN_C99_OK) return 0;
        for (index = 0; index < shape->property_count; ++index) {
            const kiln_string *key = shape->keys[index];
            if (key == NULL || kiln_c99_key_index_key(key) >= 0.0 ||
                kiln_c99_same_name_key(key, "__proto__") ||
                !kiln_c99_clone_plain_val_for_message(object->slots[index], seen,
                                                       depth + 1)) return 0;
        }
        return 1;
    }
    if (kiln_c99_gc_payload_kind(ptr) == KILN_GC_ARRAY) {
        const kiln_array *array = (const kiln_array *)ptr;
        if (!kiln_c99_clone_safe_array(array) || array->count > array->capacity ||
            (array->count != 0 && array->elements == NULL)) return 0;
        if (kiln_c99_gc_ptrset_insert(seen, ptr) != KILN_C99_OK) return 0;
        for (index = 0; index < array->count; ++index) {
            if (!kiln_c99_clone_plain_val_for_message(array->elements[index], seen,
                                                       depth + 1)) return 0;
        }
        return 1;
    }
    return 0;
}

static int kiln_c99_clone_plain_graph_for_message(kiln_value source) {
    kiln_gc_ptrset seen = {0};
    int plain;
    /* The TypeScript preparation uses Object/Array/Map built-ins. Once a realm changes one,
     * its observable preparation path must run rather than bypassing the replacement. */
    if (!kiln_c99_realm_builtins_unmodified() ||
        (source.tag != KILN_VALUE_OBJECT && source.tag != KILN_VALUE_ARRAY)) return 0;
    plain = kiln_c99_clone_plain_val_for_message(kiln_val_from_boundary(source), &seen, 0);
    free(seen.table);
    return plain;
}

/* The shape of a copy of the list-mode `source`: the transition walk from `root`, the fresh
 * copy's empty shape, over the source's keys in order -- the shape an object literal with those
 * keys has, and shared with every object built that way (ADR 0071). An object is in list mode
 * for reasons that are its history rather than its keys (a deleted property, an attribute once
 * redefined), and copying the dictionary made every read of the copy a key comparison, where
 * node's deserializer builds a fast object. NULL when the tree will not hold the keys -- an
 * index-like key, which enumerates before the others, or the fast-property limit -- and the copy
 * stays in list mode like its source. */
static kiln_shape *kiln_c99_clone_list_shape(const kiln_object *source, kiln_shape *root) {
    kiln_shape *walk = root;
    size_t i;
    for (i = 0; walk != NULL && i < source->count; ++i) {
        const kiln_property *property = &source->properties[i];
        const kiln_string *ikey = kiln_c99_intern_key(property->key);
        kiln_shape *next = NULL;
        if (ikey == NULL || kiln_c99_key_index_key(ikey) >= 0.0 ||
            !kiln_c99_shape_transition(walk, ikey, property->value.tag == KILN_VALUE_NUMBER,
                                       &next)) {
            return NULL;
        }
        walk = next;
    }
    return walk;
}

/* A private arena is deliberately absent from the sender's GC index. The ordinary boxed-to-
 * boundary conversion checks that index under stress, so construct these two known arena kinds
 * directly when a dictionary record needs a boundary value. */
static kiln_value kiln_c99_clone_arena_boundary(kiln_val value) {
    if (KILN_VAL_IS_OBJECT(value))
        return kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)KILN_VAL_PTR(value),
                              NULL, 0);
    if ((value & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)
        return kiln_c99_value(KILN_VALUE_ARRAY, (uint64_t)(uintptr_t)KILN_VAL_PTR(value),
                              NULL, 0);
    return kiln_val_to_boundary(value);
}

/* One value: `*out` is its copy, or the status is KILN_C99_CLONE_BAIL. */
static int kiln_c99_clone_data_val(kiln_c99_clone_table *table, kiln_val value, int depth,
                                   kiln_val *out) {
    void *ptr;
    uint32_t kind;
    int status;
    if (table->candidate_graph && KILN_VAL_IS_STRING(value)) {
        const kiln_string *source = (const kiln_string *)KILN_VAL_PTR(value);
        kiln_string *copy;
        kiln_val cloned;
        size_t width;
        if (source == NULL ||
            (source->flags & (KILN_STRING_SYMBOL_KEY | KILN_STRING_PRIVATE_KEY)) != 0)
            return KILN_C99_CLONE_BAIL;
        if (kiln_c99_clone_table_get(table, value, out)) return KILN_C99_OK;
        if (table->arena != NULL) {
            size_t storage;
            uint32_t flags = kiln_string_is_utf16(source) ? KILN_STRING_UTF16
                                                            : KILN_STRING_LATIN1;
            if (!kiln_string_storage_size(source->unit_len, flags, &storage))
                return KILN_C99_OUT_OF_MEMORY;
            copy = storage > SIZE_MAX - sizeof(kiln_gc_block) ||
                           storage + sizeof(kiln_gc_block) >
                               (table->arena->storage.heap->nursery.capacity >> 2)
                       ? kiln_c99_clone_arena_large_string(table->arena, storage)
                       : (kiln_string *)kiln_c99_par_alloc(&table->arena->storage,
                                                           KILN_GC_STRING, storage);
            if (copy != NULL) {
                copy->unit_len = source->unit_len;
                copy->flags = flags;
                copy->data = copy + 1;
            }
        } else {
            copy = kiln_c99_gc_alloc_units(source->unit_len,
                                           kiln_string_is_utf16(source) ? KILN_STRING_UTF16
                                                                         : KILN_STRING_LATIN1);
        }
        if (copy == NULL) return KILN_C99_OUT_OF_MEMORY;
        width = kiln_string_is_utf16(source) ? sizeof(uint16_t) : sizeof(uint8_t);
        if (source->unit_len != 0)
            memcpy((void *)copy->data, source->data, source->unit_len * width);
        cloned = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, copy);
        status = kiln_c99_clone_table_put(table, value, cloned);
        if (status != KILN_C99_OK) return status;
        *out = cloned;
        return KILN_C99_OK;
    }
    if (KILN_VAL_IS_NUMBER(value) || value == KILN_VAL_UNDEFINED || value == KILN_VAL_NULL ||
        KILN_VAL_IS_BOOLEAN(value) || KILN_VAL_IS_STRING(value) ||
        (value & KILN_TAG_MASK) == KILN_TAG_BIGINT_PREFIX) {
        /* Primitives, and strings and BigInts are immutable: the copy is the value. The BigInt
         * test is on the whole top half, so a Temporal value (a narrower prefix inside the same
         * range) is not mistaken for one. */
        if ((value & KILN_TAG_MASK) == KILN_TAG_BIGINT_PREFIX &&
            (value & UINT64_C(0xFFFF800000000000)) != KILN_TAG_BIGINT_PREFIX) {
            return KILN_C99_CLONE_BAIL;
        }
        *out = value;
        return KILN_C99_OK;
    }
    if (!KILN_VAL_IS_OBJECT(value) && (value & KILN_TAG_MASK) != KILN_TAG_BUFFER_PREFIX) {
        return KILN_C99_CLONE_BAIL;
    }
    ptr = KILN_VAL_PTR(value);
    if (ptr == NULL || depth > KILN_C99_CLONE_MAX_DEPTH) return KILN_C99_CLONE_BAIL;
    if (kiln_c99_clone_table_get(table, value, out)) return KILN_C99_OK;
    kind = kiln_c99_gc_payload_kind(ptr);
    if (KILN_VAL_IS_OBJECT(value) && kind == KILN_GC_OBJECT) {
        kiln_object *source = (kiln_object *)ptr;
        kiln_shape *shape = source->shape;
        kiln_shape *source_shape = shape;
        kiln_value made;
        kiln_object *copy;
        kiln_val copy_val;
        size_t count;
        size_t i;
        if (!kiln_c99_clone_safe_object(source)) return KILN_C99_CLONE_BAIL;
        if (table->arena != NULL) {
            if (shape == NULL || (shape->shared & 1u) == 0) {
                size_t members = shape == NULL ? source->count : shape->property_count;
                status = members == 0
                             ? kiln_c99_clone_arena_object(table->arena, 0, &made)
                             : kiln_c99_clone_arena_dictionary(table->arena, members,
                                                               &made);
                shape = members == 0 ? kiln_c99_shape_root() : NULL;
            } else {
                status = kiln_c99_clone_arena_object(table->arena, shape->property_count, &made);
            }
        } else {
            if (shape == NULL) shape = kiln_c99_clone_list_shape(source, kiln_c99_shape_root());
            status = shape == NULL ? kiln_c99_new_object(&made)
                                   : kiln_c99_new_object_with_capacity(&made, shape->property_count);
        }
        if (status != KILN_C99_OK) return status;
        copy_val = kiln_val_from_boundary(made);
        copy = (kiln_object *)KILN_VAL_PTR(copy_val);
        status = kiln_c99_clone_table_put(table, value, copy_val);
        if (status != KILN_C99_OK) return status;
        if (shape != NULL) {
            count = shape->property_count;
            if (count > copy->slot_capacity) {
                status = kiln_c99_grow_slots(copy, count);
                if (status != KILN_C99_OK) return status;
            }
            /* Nothing the program wrote can run while this walks, so the source's shape and
             * slots now are the ones it has at the end. A list-mode source's values go into the
             * slots its keys were given, which are in the same order. */
            if (source->shape != NULL) {
                if (count > 0) memcpy(copy->slots, source->slots, count * sizeof(kiln_val));
            } else {
                for (i = 0; i < count; ++i) {
                    copy->slots[i] = kiln_val_from_boundary(source->properties[i].value);
                }
            }
            copy->shape = shape;
            for (i = 0; i < count; ++i) {
                kiln_val item = copy->slots[i];
                kiln_val cloned;
                if (KILN_VAL_IS_NUMBER(item) ||
                    (!table->candidate_graph && KILN_VAL_IS_STRING(item)) ||
                    item == KILN_VAL_UNDEFINED || item == KILN_VAL_NULL ||
                    KILN_VAL_IS_BOOLEAN(item)) {
                    continue;
                }
                status = kiln_c99_clone_data_val(table, item, depth + 1, &cloned);
                if (status != KILN_C99_OK) return status;
                copy->slots[i] = cloned;
            }
        } else {
            count = source_shape == NULL ? source->count : source_shape->property_count;
            if (count > 0) {
                kiln_property *properties = copy->properties;
                if (table->arena == NULL) {
                    properties = (kiln_property *)malloc(count * sizeof *properties);
                    if (properties == NULL) return KILN_C99_OUT_OF_MEMORY;
                }
                if (source_shape == NULL) {
                    memcpy(properties, source->properties, count * sizeof *properties);
                } else {
                    for (i = 0; i < count; ++i) {
                        memset(&properties[i], 0, sizeof properties[i]);
                        properties[i].key = source_shape->keys[i];
                        properties[i].enumerable = 1;
                        properties[i].value = kiln_val_to_boundary(source->slots[i]);
                    }
                }
                for (i = 0; i < count; ++i) {
                    properties[i].writable = 1;
                    properties[i].configurable = 1;
                }
                copy->properties = properties;
                copy->count = count;
                copy->capacity = count;
                copy->shape = NULL;
            }
            for (i = 0; i < count; ++i) {
                if (table->arena != NULL) {
                    kiln_val cloned_key;
                    status = kiln_c99_clone_data_val(
                        table, KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX,
                                                copy->properties[i].key), depth + 1,
                        &cloned_key);
                    if (status != KILN_C99_OK) return status;
                    copy->properties[i].key = (const kiln_string *)KILN_VAL_PTR(cloned_key);
                }
                kiln_value item = copy->properties[i].value;
                kiln_val cloned;
                if (item.tag != KILN_VALUE_OBJECT && item.tag != KILN_VALUE_ARRAY &&
                    !(table->candidate_graph && item.tag == KILN_VALUE_STRING)) {
                    if (item.tag == KILN_VALUE_NUMBER ||
                        (!table->candidate_graph && item.tag == KILN_VALUE_STRING) ||
                        item.tag == KILN_VALUE_BOOLEAN || item.tag == KILN_VALUE_NULL ||
                        item.tag == KILN_VALUE_UNDEFINED || item.tag == KILN_VALUE_BIGINT) {
                        continue;
                    }
                    return KILN_C99_CLONE_BAIL;
                }
                status = kiln_c99_clone_data_val(table, kiln_val_from_boundary(item), depth + 1, &cloned);
                if (status != KILN_C99_OK) return status;
                /* Read the array again: the walk allocated, and the object it belongs to is
                 * the one that owns it, so the pointer is the same, but this keeps the store
                 * next to the only load that can see it. */
                copy->properties[i].value = table->arena != NULL
                                                ? kiln_c99_clone_arena_boundary(cloned)
                                                : kiln_val_to_boundary(cloned);
            }
        }
        *out = copy_val;
        return KILN_C99_OK;
    }
    if ((value & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && kind == KILN_GC_ARRAY) {
        kiln_array *source = (kiln_array *)ptr;
        kiln_value made;
        kiln_array *copy;
        kiln_val copy_val;
        size_t count = source->count;
        size_t i;
        if (!kiln_c99_clone_safe_array(source)) return KILN_C99_CLONE_BAIL;
        status = table->arena != NULL
                     ? kiln_c99_clone_arena_array(table->arena, count, source->is_numeric, &made)
                     : kiln_c99_new_array_with_capacity(&made, count, source->is_numeric);
        if (status != KILN_C99_OK) return status;
        copy_val = kiln_val_from_boundary(made);
        copy = (kiln_array *)KILN_VAL_PTR(copy_val);
        if (count > 0) memcpy(copy->elements, source->elements, count * sizeof(kiln_val));
        copy->count = count;
        status = kiln_c99_clone_table_put(table, value, copy_val);
        if (status != KILN_C99_OK) return status;
        if (!copy->is_numeric) {
            for (i = 0; i < count; ++i) {
                kiln_val item = copy->elements[i];
                kiln_val cloned;
                if (KILN_VAL_IS_NUMBER(item) ||
                    (!table->candidate_graph && KILN_VAL_IS_STRING(item)) ||
                    item == KILN_VAL_HOLE || item == KILN_VAL_UNDEFINED ||
                    item == KILN_VAL_NULL || KILN_VAL_IS_BOOLEAN(item)) {
                    continue;
                }
                status = kiln_c99_clone_data_val(table, item, depth + 1, &cloned);
                if (status != KILN_C99_OK) return status;
                copy->elements[i] = cloned;
            }
        }
        *out = copy_val;
        return KILN_C99_OK;
    }
    return KILN_C99_CLONE_BAIL;
}

static int kiln_c99_clone_data_graph(kiln_value *out, kiln_value source, int candidate_graph,
                                     kiln_c99_clone_arena *arena) {
    kiln_c99_clone_table table;
    kiln_val root = KILN_VAL_UNDEFINED;
    int status;
    memset(table.inline_keys, 0, sizeof table.inline_keys);
    table.keys = table.inline_keys;
    table.values = table.inline_values;
    table.capacity = KILN_C99_CLONE_INLINE;
    table.count = 0;
    table.candidate_graph = candidate_graph;
    table.arena = arena;
    status = kiln_c99_clone_data_val(&table, kiln_val_from_boundary(source), 0, &root);
    if (table.keys != table.inline_keys) {
        free(table.keys);
        free(table.values);
    }
    if (status != KILN_C99_OK) return status;
    if (arena != NULL) {
        /* The private arena is deliberately absent from the sender's heap table. The ordinary
         * boxed-to-boundary conversion checks that table under GC stress, so construct only
         * the two root kinds this arena allocator can have without consulting the sender. */
        if (KILN_VAL_IS_OBJECT(root))
            *out = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)KILN_VAL_PTR(root),
                                  NULL, 0);
        else if ((root & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)
            *out = kiln_c99_value(KILN_VALUE_ARRAY, (uint64_t)(uintptr_t)KILN_VAL_PTR(root),
                                  NULL, 0);
        else
            return KILN_C99_CLONE_BAIL;
    } else {
        *out = kiln_val_to_boundary(root);
    }
    return KILN_C99_OK;
}

/* Candidate-copy mode owns every string value it reaches. A future private arena can reuse
 * the same graph walk with an arena allocator; this helper does not publish the copy to JS. */
static int kiln_c99_clone_data_candidate(kiln_value *out, kiln_value source) {
    return kiln_c99_clone_data_graph(out, source, 1, NULL);
}

static int kiln_c99_clone_data_arena(kiln_value *out, kiln_value source,
                                     kiln_c99_clone_arena *arena) {
    return kiln_c99_clone_data_graph(out, source, 1, arena);
}

/* The view indexes only this arena's chunks. It is never registered as a realm heap, and its
 * region table is discarded after the outgoing closure certificate has been built. The arena
 * keeps ownership of the chunks until receiver adoption, including across sender collections. */
static int kiln_c99_clone_arena_prepare(kiln_c99_clone_arena *arena, kiln_gc_heap *sender,
                                         kiln_value source, size_t budget) {
    kiln_gc_heap *view;
    kiln_nursery_chunk *first;
    kiln_value root;
    int status;
    if (arena == NULL || sender == NULL) return KILN_C99_INVALID_ARGUMENT;
    memset(arena, 0, sizeof *arena);
    if (!kiln_c99_graph_candidate_open(&arena->storage, sender, budget))
        return KILN_C99_OUT_OF_MEMORY;
    status = kiln_c99_clone_data_arena(&root, source, arena);
    if (status != KILN_C99_OK) {
        kiln_c99_clone_arena_dispose(arena);
        return status;
    }
    kiln_c99_par_arena_seal(&arena->storage);
    first = arena->storage.filled;
    if (first == NULL) {
        kiln_c99_clone_arena_dispose(arena);
        return KILN_C99_CLONE_BAIL;
    }
    view = (kiln_gc_heap *)calloc(1, sizeof *view);
    if (view == NULL) {
        kiln_c99_clone_arena_dispose(arena);
        return KILN_C99_OUT_OF_MEMORY;
    }
    view->epoch = 1;
    view->nursery.capacity = sender->nursery.capacity;
    view->nursery.bitmap_words = sender->nursery.bitmap_words;
    if (!kiln_c99_heap_adopt_chunks(view, first)) {
        free(view->nursery.regions.table);
        free(view);
        kiln_c99_clone_arena_dispose(arena);
        return KILN_C99_OUT_OF_MEMORY;
    }
    /* adopt_chunks reverses the list. Keep its new head even if a later large-block index
     * insertion fails, so dispose still owns every chunk. */
    arena->storage.filled = view->nursery.full;
    view->head = arena->large_head;
    {
        kiln_gc_block *large;
        for (large = arena->large_head; large != NULL; large = large->next) {
            if (kiln_c99_gc_ptrset_insert(&view->large_table, large) != KILN_C99_OK) {
                free(view->large_table.table);
                free(view->nursery.regions.table);
                free(view);
                kiln_c99_clone_arena_dispose(arena);
                return KILN_C99_OUT_OF_MEMORY;
            }
            kiln_c99_gc_bound(view, large, sizeof *large + large->size);
            view->count += 1;
            view->bytes += large->size;
        }
    }
    first = arena->storage.filled;
    if (!kiln_c99_graph_capsule_prepare_with_large(view, first, root, &arena->capsule,
                                                    arena->large_head)) {
        free(view->large_table.table);
        free(view->nursery.regions.table);
        free(view);
        kiln_c99_clone_arena_dispose(arena);
        return KILN_C99_CLONE_BAIL;
    }
    arena->certified = 1;
    free(view->large_table.table);
    free(view->nursery.regions.table);
    free(view);
    return KILN_C99_OK;
}

/* All fallible receiver work precedes the ownership change. The copied arena has no sender
 * heap membership or incoming sender edge, so no sender-heap scan or unlink is needed. */
static int kiln_c99_clone_arena_adopt(kiln_c99_clone_arena *arena, kiln_gc_heap *receiver,
                                       uint64_t reservation) {
    if (arena == NULL || receiver == NULL || !arena->certified || arena->adopted ||
        !kiln_c99_graph_capsule_receiver_preflight(receiver, &arena->capsule.needs, reservation))
        return 0;
    arena->capsule.detached = 1;
    kiln_c99_graph_capsule_adopt(receiver, &arena->capsule, reservation);
    arena->adopted = 1;
    arena->storage.filled = NULL;
    arena->large_head = NULL;
    return 1;
}

static void kiln_c99_prepared_graph_drop(void *payload) {
    kiln_c99_clone_arena *arena = (kiln_c99_clone_arena *)payload;
    kiln_c99_clone_arena_dispose(arena);
    free(arena);
}

/* The JavaScript handle is only a weak key. It contains no graph pointer, and its collection
 * releases an abandoned arena before the handle's block is swept. */
static int kiln_c99_prepared_graph_create(kiln_gc_heap *heap, kiln_value handle,
                                           kiln_value source, size_t budget) {
    kiln_c99_clone_arena *arena;
    int status;
    if (heap == NULL || handle.tag != KILN_VALUE_OBJECT ||
        kiln_c99_gc_weak_resource_find(heap, handle) != NULL)
        return KILN_C99_INVALID_ARGUMENT;
    arena = (kiln_c99_clone_arena *)calloc(1, sizeof *arena);
    if (arena == NULL) return KILN_C99_OUT_OF_MEMORY;
    status = kiln_c99_clone_arena_prepare(arena, heap, source, budget);
    if (status != KILN_C99_OK) {
        free(arena);
        return status;
    }
    if (!kiln_c99_gc_weak_resource_register(heap, handle, arena,
                                              kiln_c99_prepared_graph_drop)) {
        kiln_c99_prepared_graph_drop(arena);
        return KILN_C99_OUT_OF_MEMORY;
    }
    return KILN_C99_OK;
}

static kiln_c99_clone_arena *kiln_c99_prepared_graph_find(kiln_gc_heap *heap,
                                                            kiln_value handle) {
    kiln_gc_weak_resource *entry = kiln_c99_gc_weak_resource_find(heap, handle);
    return entry != NULL && entry->dispose == kiln_c99_prepared_graph_drop
               ? (kiln_c99_clone_arena *)entry->payload : NULL;
}

static int kiln_c99_prepared_graph_has(kiln_gc_heap *heap, kiln_value handle) {
    kiln_c99_clone_arena *arena = kiln_c99_prepared_graph_find(heap, handle);
    return arena != NULL && arena->certified && !arena->adopted &&
           !arena->capsule.detached && arena->capsule.chunk != NULL;
}

static kiln_c99_graph_capsule *kiln_c99_prepared_graph_capsule(kiln_gc_heap *heap,
                                                               kiln_value handle) {
    kiln_c99_clone_arena *arena = kiln_c99_prepared_graph_find(heap, handle);
    return arena != NULL && kiln_c99_prepared_graph_has(heap, handle)
               ? &arena->capsule : NULL;
}

/* Receiver publication already owns the chunks. Clearing the arena's ownership before releasing
 * the weak handle makes its destructor free metadata and side-buffer records only. */
static void kiln_c99_prepared_graph_commit(kiln_gc_heap *heap, kiln_value handle,
                                            kiln_c99_graph_capsule *capsule) {
    kiln_c99_clone_arena *arena = kiln_c99_prepared_graph_find(heap, handle);
    if (arena == NULL || &arena->capsule != capsule || !capsule->adopted ||
        capsule->detached || capsule->chunk != NULL) abort();
    arena->adopted = 1;
    arena->storage.filled = NULL;
    kiln_c99_gc_weak_resource_release(heap, handle);
}

static int kiln_c99_clone_data_raw(kiln_value *out, const kiln_value *args, size_t argc) {
    int status;
    *out = kiln_c99_undefined();
    if (argc == 0) return KILN_C99_OK;
    if (argc > 1 && args[1].tag == KILN_VALUE_NUMBER &&
        kiln_c99_as_number(args[1]) == 1.0)
        return KILN_C99_OK; /* The shared Rust worker codec handles plain messages. */
    status = kiln_c99_clone_data_graph(out, args[0], 0, NULL);
    return status == KILN_C99_CLONE_BAIL ? KILN_C99_OK : status;
}

/* Representation callbacks for the provider's shared Rust encoder. They only inspect own
 * plain-data storage; no JavaScript getter, proxy trap, or heap allocation runs here. Values
 * stay rooted in the invoking generated frame for this synchronous walk. */
static int kiln_c99_worker_plain_describe(void *context, uint64_t handle,
                                           kiln_worker_plain_node *out) {
    kiln_val value = (kiln_val)handle;
    void *pointer;
    (void)context;
    memset(out, 0, sizeof *out);
    if (value == KILN_VAL_UNDEFINED) { out->kind = 0; return 1; }
    if (value == KILN_VAL_NULL) { out->kind = 1; return 1; }
    if (value == KILN_VAL_FALSE || value == KILN_VAL_TRUE) {
        out->kind = value == KILN_VAL_TRUE ? 3 : 2;
        return 1;
    }
    if (KILN_VAL_IS_NUMBER(value) || value == KILN_QNAN_BASE) {
        union { uint64_t bits; double number; } decoded = { .bits = value };
        out->kind = 4;
        out->number = decoded.number;
        return 1;
    }
    if (KILN_VAL_IS_STRING(value)) {
        pointer = KILN_VAL_PTR(value);
        if (pointer == NULL) return 0;
        out->kind = 5;
        out->text = (const kiln_string *)pointer;
        return 1;
    }
    if (KILN_VAL_IS_OBJECT(value) || (value & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {
        int is_object = KILN_VAL_IS_OBJECT(value);
        pointer = KILN_VAL_PTR(value);
        if (pointer == NULL || kiln_c99_gc_payload_kind(pointer) !=
            (is_object ? KILN_GC_OBJECT : KILN_GC_ARRAY)) return 0;
        if (is_object) {
            kiln_object *object = (kiln_object *)pointer;
            if (!kiln_c99_clone_safe_object(object)) return 0;
            out->kind = 8;
            out->length = object->shape == NULL ? object->count : object->shape->property_count;
        } else {
            kiln_array *array = (kiln_array *)pointer;
            if (!kiln_c99_clone_safe_array(array)) return 0;
            out->kind = 9;
            out->length = array->count;
        }
        if (out->length > 9007199254740991ull) return 0;
        out->identity = (size_t)(uintptr_t)pointer;
        return 1;
    }
    return 0;
}

static int kiln_c99_worker_plain_field(void *context, uint64_t handle, size_t index,
                                        kiln_worker_plain_child *out) {
    kiln_val value = (kiln_val)handle;
    (void)context;
    memset(out, 0, sizeof *out);
    /* The shared codec calls `field` only after `describe` accepted this compound handle.
     * Its children are already boxed machine words in array elements and shaped slots. */
    if ((value & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {
        kiln_array *array = (kiln_array *)KILN_VAL_PTR(value);
        kiln_val item;
        if (index >= array->count) return 0;
        item = array->elements[index];
        if (item == KILN_VAL_HOLE) out->hole = 1;
        else out->value = (uint64_t)item;
        return 1;
    }
    if (KILN_VAL_IS_OBJECT(value)) {
        kiln_object *object = (kiln_object *)KILN_VAL_PTR(value);
        kiln_shape *shape = object->shape;
        const kiln_string *key;
        if (index >= (shape == NULL ? object->count : shape->property_count)) return 0;
        key = shape == NULL ? object->properties[index].key : shape->keys[index];
        if (key == NULL || kiln_c99_key_index_key(key) >= 0.0) return 0;
        out->key = key;
        out->value = shape == NULL
            ? (uint64_t)kiln_val_from_boundary(object->properties[index].value)
            : (uint64_t)object->slots[index];
        return 1;
    }
    return 0;
}

static int kiln_c99_worker_post_plain(uint64_t port, kiln_value value, kiln_value *out) {
    int status;
    *out = kiln_c99_undefined();
    if (value.tag != KILN_VALUE_OBJECT && value.tag != KILN_VALUE_ARRAY) return KILN_C99_OK;
    status = kiln_worker_post_plain(port, (uint64_t)kiln_val_from_boundary(value), NULL,
                                    kiln_c99_worker_plain_describe, kiln_c99_worker_plain_field);
    if (status < 0) return KILN_C99_INVALID_ARGUMENT;
    if (status > 0) *out = kiln_c99_boolean(1);
    return KILN_C99_OK;
}

static int kiln_c99_spread_object(kiln_host *host, kiln_value target, kiln_value source,
                                  int depth, kiln_value *thrown) {
    kiln_value named = kiln_c99_undefined();
    kiln_object *object;
    size_t index;
    if (source.tag == KILN_VALUE_UNDEFINED || source.tag == KILN_VALUE_NULL) {
        return KILN_C99_OK;
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        KILN_C99_REFUSE("an object spread into something that is not an object");
    }
    if (source.tag == KILN_VALUE_PROXY) {
        return kiln_c99_spread_proxy(host, target, source, depth, thrown);
    }
    if (source.tag == KILN_VALUE_SHARED) {
        kiln_value keys;
        kiln_array *names;
        int status = kiln_c99_own_keys(host, source, &keys, depth, thrown);
        if (status != KILN_C99_OK) return status;
        names = kiln_c99_array_of(keys);
        for (index = 0; index < names->count; ++index) {
            kiln_value key = kiln_val_to_boundary(names->elements[index]);
            kiln_value value;
            status = kiln_c99_get_computed(host, source, key, &value, depth, thrown);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_put_mode_key(host, target, (const kiln_string *)key.ptr, NULL,
                                           value, 0, depth, thrown);
            if (status != KILN_C99_OK) return status;
        }
        return KILN_C99_OK;
    }
    /* A primitive has no own enumerable properties, so spreading one contributes nothing:
     * `{...5}` is `{}`, not an error. A string is the exception and is handled below, because
     * its indices *are* own properties. */
    if (source.tag == KILN_VALUE_NUMBER || source.tag == KILN_VALUE_BOOLEAN ||
        source.tag == KILN_VALUE_SYMBOL || source.tag == KILN_VALUE_BIGINT) {
        return KILN_C99_OK;
    }
    if (source.tag == KILN_VALUE_ARRAY || source.tag == KILN_VALUE_TYPED_ARRAY ||
        source.tag == KILN_VALUE_STRING) {
        /* An indexed value spreads as its own enumerable properties, and those are its indices:
         * `{...[7, 8]}` is `{"0": 7, "1": 8}` and `{..."hi"}` is `{"0": "h", "1": "i"}`.
         *
         * A different operation from array spread, which reads the iterator -- which is why a
         * hole is *skipped* here instead of becoming undefined. `[...[1, , 3]]` has three
         * elements and `{...[1, , 3]}` has two properties. */
        size_t count = 0;
        size_t at;
        if (source.tag == KILN_VALUE_ARRAY) {
            kiln_array *elements = kiln_c99_array_of(source);
            count = elements->count;
        } else if (source.tag == KILN_VALUE_TYPED_ARRAY) {
            count = kiln_c99_typed_array_of(source)->length;
        } else {
            uint16_t *units = NULL;
            int status = kiln_c99_to_units(source, &units, &count);
            free(units);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        for (at = 0; at < count; ++at) {
            kiln_value element;
            kiln_value name;
            int status;
            if (source.tag == KILN_VALUE_ARRAY &&
                kiln_c99_array_of(source)->elements[at] == KILN_VAL_HOLE) {
                continue;
            }
            status = kiln_c99_get_computed(host, source, kiln_c99_number((double)at), &element,
                                           depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            /* The key has to outlive this frame -- a property stores the pointer rather than
             * copying the bytes -- which is why it is built as a value rather than sprintf'd
             * into a local buffer, exactly as a computed write builds one. */
            name = kiln_c99_text(kiln_c99_number((double)at));
            if (name.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            /* The headed form, as in the computed write above: the byte adapter would read
             * the header's words as key bytes. */
            status = kiln_c99_put_mode_key(host, target, (const kiln_string *)name.ptr, NULL,
                                           element, 0, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        /* ...and then whatever the program hung on the value by name, which is own and
         * enumerable like any other property: `a.total = 1` belongs in the result, and
         * `Object.keys` already reported it while the spread beside it dropped it. `length` is
         * not among them, because it is not enumerable. A string and a typed array have no
         * named store to walk, so for those this falls straight through. */
        if (source.tag == KILN_VALUE_ARRAY) {
            named = kiln_c99_array_of(source)->properties;
        }
    } else if (source.tag == KILN_VALUE_OBJECT) {
        named = source;
    } else if (source.tag == KILN_VALUE_FUNCTION || source.tag == KILN_VALUE_METHOD ||
               source.tag == KILN_VALUE_BOUND) {
        /* A function is an object, and a static a program hung on one is an own enumerable
         * property. `name`, `length` and `prototype` are not in this store -- they are derived
         * from the closure, and all three are non-enumerable exactly as the specification marks
         * them -- so `{ ...f }` is what the program put on `f` and nothing of the function's
         * own. Asked of the same helper a read of a static uses, so the two cannot disagree
         * about where a function keeps its properties. */
        int stored = kiln_c99_callable_store(source, &named, 0);
        if (stored != KILN_C99_OK) {
            return stored;
        }
    } else {
        /* Deliberately not `KILN_VALUE_BUILTIN`, even though it has a store: a Node builtin
         * namespace publishes *enumerable* exports, which live in the interpreter's ABI table
         * and not in the store a program writes into. Reading only the store would answer `{}`
         * where the interpreter answers seventeen names -- a silent divergence, which is worse
         * than the refusal it would replace. */
        KILN_C99_REFUSE("an object spread of a value with no enumerable property store");
    }
    /* Both stores are created on first use, so a function that never carried a property and an
     * array with no names have nothing here rather than an empty object. */
    if (named.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_OK;
    }
    object = kiln_c99_object_of(named);
    /* The common case a benchmark makes common: `{ ...base, f: i }` where `base` is a fresh
     * shaped object and the literal being built is too. Ordinarily every key goes through
     * `kiln_c99_put` -- interning the name again, walking a shape transition one key at a time,
     * and round-tripping the value through `kiln_value` (which for a string allocates a fresh
     * copy) -- work the language does not actually require, because a spread copies references,
     * not fresh values, and this source's whole key *path* is already a shape the target could
     * simply adopt.
     *
     * That substitution is only safe when nothing about either object asks for slower handling:
     * the target must still be the shape's fresh, empty starting point (no keys, no properties,
     * no elements, extensible, no accessor could have intercepted a write because none exists to
     * intercept), and the source must have nothing that needs per-key attention -- no integer
     * keys (handled separately below), no accessors or other attribute-bearing slots (a source
     * with any leaves shape mode just below, for the list walk), and no symbol keys
     * (spelled "@@..." in a shape's keys, and needing the validation the loop below gives them).
     * Anything else -- a target with keys already, list-mode sources, arrays, proxies -- never
     * reaches here in the first place. */
    /* A source with a hidden, read-only or accessor slot is copied by the list walk below,
     * which knows which of its properties a spread sees and how. */
    if (object->shape != NULL && object->shape->special_mask != 0) {
        kiln_c99_ensure_dictionary(object);
    }
    if (object->properties == NULL && object->shape != NULL && object->elements == NULL &&
        object->accessor_count == 0 && target.tag == KILN_VALUE_OBJECT) {
        kiln_object *dest = kiln_c99_object_of(target);
        if (dest->properties == NULL && dest->shape != NULL && dest->shape->property_count == 0 &&
            dest->elements == NULL && !dest->frozen && !dest->not_extensible &&
            dest->accessor_count == 0) {
            kiln_shape *sshape = object->shape;
            size_t scount = sshape->property_count;
            size_t k;
            int has_symbol_key = 0;
            for (k = 0; k < scount; ++k) {
                const kiln_string *skey = sshape->keys[k];
                if (kiln_c99_key_is_reserved(skey)) {
                    has_symbol_key = 1;
                    break;
                }
            }
            if (!has_symbol_key) {
                /* Grow exactly as the generic per-key path does (`kiln_c99_define_computed`'s
                 * slot-growth site): double from the inline slots, or from whatever heap array
                 * is already there, until the source's slot count fits. */
                {
                    int grown = kiln_c99_grow_slots(dest, scount);
                    if (grown != KILN_C99_OK) return grown;
                }
                /* Slots are NaN-boxed `kiln_val`s, so this `memcpy` copies references exactly as
                 * a spread must: a nested object or array or string is shared with the source,
                 * not cloned, and mutating the copy later does not touch the source's slot
                 * because the two objects hold independent slot arrays from here on. Adopting
                 * the source's shape is what makes the copied slots meaningful -- a shape is
                 * nothing but the key path that says what each slot index means -- and every
                 * later write to the target transitions from there exactly as if its keys had
                 * been added one at a time, because shapes reached by the same key order are
                 * the same shape pointer regardless of how an object arrived at one. */
                if (scount > 0) {
                    memcpy(dest->slots, object->slots, scount * sizeof(kiln_val));
                }
                dest->shape = sshape;
                return KILN_C99_OK;
            }
        }
        /* A shaped target that already has keys -- `{ a: 1, ...base }`, or the trailing
         * `{ ...base, f: i }` that the compiler builds as a second literal spread in -- takes
         * each source key as the target's own slot when it has one, or as one transition of
         * its shape, which is all the generic put does for it minus the interning and the
         * trip through `kiln_value`: the shape's keys are the interned pointers already, and
         * a spread copies references. A key the shape tree will not take -- the fast-property
         * limit -- leaves the rest to the generic path, which redefines what was copied to the
         * same value. */
        if (dest->properties == NULL && dest->shape != NULL && dest->elements == NULL &&
            !dest->frozen && !dest->not_extensible && dest->accessor_count == 0 &&
            dest->shape->special_mask == 0) {
            kiln_shape *sshape = object->shape;
            size_t scount = sshape->property_count;
            size_t k;
            int plain = 1;
            for (k = 0; k < scount; ++k) {
                const kiln_string *skey = sshape->keys[k];
                if (kiln_c99_key_is_reserved(skey)) {
                    plain = 0;
                    break;
                }
            }
            for (k = 0; plain && k < scount; ++k) {
                const kiln_string *ikey = sshape->keys[k];
                kiln_val sval = k < object->slot_capacity ? object->slots[k] : KILN_VAL_UNDEFINED;
                uint32_t slot;
                kiln_shape *next;
                if (kiln_c99_shape_find_slot(dest->shape, ikey, &slot)) {
                    if (slot >= dest->slot_capacity) {
                        break;
                    }
                    dest->slots[slot] = sval;
                } else {
                    int is_double = KILN_VAL_IS_NUMBER(sval);
                    if (kiln_c99_shape_transition(dest->shape, ikey, is_double, &next)) {
                        int grown = kiln_c99_grow_slots(dest, (size_t)next->slot_index + 1);
                        if (grown != KILN_C99_OK) return grown;
                        dest->slots[next->slot_index] = sval;
                        dest->shape = next;
                    } else {
                        break;
                    }
                }
            }
            if (plain && k == scount) {
                return KILN_C99_OK;
            }
        }
    }
    if (object->properties == NULL && object->shape != NULL) {
        if (object->elements != NULL) {
            for (index = 0; index < object->element_count; ++index) {
                if (object->elements[index] == KILN_VAL_HOLE) continue;
                char num_buf[32];
                size_t nlen = kiln_c99_double_to_js((double)index, num_buf);
                int status = kiln_c99_put(host, target, num_buf, nlen, kiln_val_to_boundary(object->elements[index]), depth, thrown);
                if (status != KILN_C99_OK) return status;
            }
        }
        size_t length = object->shape->property_count;
        for (index = 0; index < length && object->shape != NULL && index < object->shape->property_count; ++index) {
            const kiln_string *skey = object->shape->keys[index];
            size_t skey_len = kiln_c99_key_len(skey);
            kiln_value sval;
            int status;
            if (kiln_c99_key_is_reserved(skey)) {
                kiln_value symbol;
                if (!kiln_c99_symbol_of_key_view(skey, &symbol)) {
                    continue;
                }
            }
            sval = (object->slots != NULL && index < object->slot_capacity)
                       ? kiln_val_to_boundary(object->slots[index])
                       : kiln_c99_undefined();
            status = kiln_c99_put_mode_key(host, target, skey, skey, sval, 0, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return KILN_C99_OK;
    }
    /* CopyDataProperties takes the own-key list before invoking any getter. A getter can delete
     * an earlier key and shift the dictionary, append another key, or replace a later value.
     * Keep the original key order in a GC-traced array, then re-read each current descriptor by
     * key: deleted keys disappear, newly added keys stay out, and later values remain live. */
    {
        kiln_value keys;
        size_t length;
        int status = kiln_c99_property_key_snapshot(object, &keys);
        if (status != KILN_C99_OK) return status;
        length = kiln_c99_array_of(keys)->count;
        for (index = 0; index < length; ++index) {
            const kiln_string *key = (const kiln_string *)
                kiln_val_to_boundary(kiln_c99_array_of(keys)->elements[index]).ptr;
            kiln_property property;
            kiln_value copied;
            kiln_value symbol;
            if (!kiln_c99_property_read_own(object, key, key, &property)) continue;
            if (!property.enumerable) {
                /* A symbol-keyed property is stored non-enumerable here whatever the program
                 * asked for: `kiln_c99_own_slot` marks every `@@` key that way, in one place, so
                 * that none of the walks over own *string* keys has to know about the prefix.
                 * That rule cannot tell a key the program made enumerable -- `{ [s]: 1 }` --
                 * from one it hid with a descriptor, and a spread is the one enumeration that
                 * has to tell them apart, because it is the one that carries symbol keys across.
                 *
                 * So this says it cannot. Copying every symbol would take the hidden ones too,
                 * and skipping them silently drops what the interpreter kept -- which is the
                 * failure this whole helper exists to stop. A machine slot is not a symbol and
                 * is not affected: it is skipped, as it should be. */
                if (!kiln_c99_symbol_of_key_view(property.key, &symbol) ||
                    property.symbol_hidden) {
                    continue;
                }
            }
            if (property.accessor) {
                if (property.getter.tag == KILN_VALUE_UNDEFINED) {
                    /* Setter-only: reading it answers `undefined`, and `undefined` is what a
                     * spread copies. Skipping the key instead left the property *absent* --
                     * `"gone" in { ...source }` said false where node says true, because
                     * `CopyDataProperties` creates a property for every enumerable own key it
                     * was given, whatever the read produced. */
                    status = kiln_c99_put_mode_key(host, target, property.key,
                                                   property.key,
                                                   kiln_c99_undefined(), 0, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                    continue;
                }
                /* The receiver is the *source*, not the store it was found in: a getter a
                 * function's statics declare has to see the function. */
                status = kiln_c99_call(host, &copied, property.getter, source,
                                       NULL, 0, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            } else {
                copied = property.value;
            }
            status = kiln_c99_put_mode_key(host, target, property.key,
                                           property.key, copied, 0, depth,
                                           thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    return KILN_C99_OK;
}

/* ---- Tier 3 of the parallel built-ins: building values on a pool thread (parallel issue 08) --
 *
 * A tier-3 callback (`parallel::heap_verdict`, ADR 0069) returns what it builds: an object
 * literal, `{ ...r, total: r.qty * r.price }`, an array literal, a string made with `+` or a
 * template. Its clone (`parallel_heap.rs`) builds each through the functions below, in the
 * worker's arena (`kiln_c99_par_alloc`, heap.c), and each builds exactly what the sequential
 * lowering builds for the same values or declines with 0:
 *
 *   - an object starts as `kiln_c99_new_object` starts one (root shape, sixteen inline slots,
 *     `Object.prototype`), and every member is defined in source order by the transition the
 *     sequential definition takes (`kiln_c99_define_named_fast`, `kiln_c99_define_computed`):
 *     the same key, the same double bit for the same value, so the same shape -- and so the same
 *     property order, which `Object.keys` and `JSON.stringify` read. A member that would leave
 *     shape mode, need slots past the inline ones, name a function, redefine a slot the
 *     sequential path would take to a dictionary, or need a realm-local edge the realm has not
 *     made yet (`kiln_c99_par_shape_step`) declines;
 *   - `{ ...r }` is `kiln_c99_spread_object_fast`'s copy -- the source's slots and its shape --
 *     under the same conditions, into a literal that has nothing yet; null and undefined spread
 *     nothing, as the language says; anything else declines;
 *   - an array literal is `kiln_c99_new_array_with_capacity`'s layout: its elements inline up to
 *     eight, inside its own block up to sixty-four, and more declines (the sequential array
 *     would `malloc` them);
 *   - a string is flat, its units the units `ToString` and `+` give (`kiln_c99_double_to_js` for a
 *     number), Latin1 unless a part is UTF-16.
 *
 * None of them reads anything but its arguments and the frozen heap, and none writes anything
 * but the block it allocated and, for a new shared transition, the process's shape tree. */

/* `ikey` as a name a pool thread may define: a canonical key that is not an index (which goes to
 * elements), not a reserved `@@` spelling, not `__proto__` and not empty -- the cases the
 * sequential fast path hands to its generic one. */
static int kiln_c99_par_key_plain(const kiln_string *ikey) {
    return ikey != NULL && ikey->unit_len != 0 && !kiln_c99_key_is_reserved(ikey) &&
           kiln_c99_key_index_key(ikey) < 0.0 && !kiln_c99_same_name_key(ikey, "__proto__");
}

static int kiln_c99_par_object(kiln_c99_par_arena *arena, kiln_val *out) {
    kiln_object *object = (kiln_object *)kiln_c99_par_alloc(arena, KILN_GC_OBJECT, sizeof *object);
    size_t slot;
    if (object == NULL) return 0;
    object->properties = NULL;
    object->count = 0;
    object->capacity = 0;
    object->prototype = kiln_c99_undefined();
    object->frozen = 0;
    object->not_extensible = 0;
    object->module_namespace = 0;
    object->arguments_brand = 0;
    object->error_brand = 0;
    object->layout_token = 0;
    object->sealed = NULL;
    object->candidate_properties = 0;
    object->candidate_slots = 0;
    object->accessor_count = 0;
    object->read_only_seen = 0;
    object->shape = kiln_c99_shape_root();
    object->slots = object->inline_slots;
    object->slot_capacity = KILN_C99_INLINE_SLOTS;
    for (slot = 0; slot < KILN_C99_INLINE_SLOTS; ++slot) object->inline_slots[slot] = KILN_VAL_UNDEFINED;
    object->elements = NULL;
    object->element_count = 0;
    object->element_capacity = 0;
    *out = KILN_VAL_TAG_PTR(KILN_TAG_OBJECT_PREFIX, object);
    return 1;
}

/* Define `ikey` (canonical) on `target`, an object this worker built, as an object literal's
 * member does. `site` is the worker's own cache for this definition: the shape it last left from
 * and the one it went to, for a value of the same kind. */
static int kiln_c99_par_define(kiln_c99_par_arena *arena, kiln_c99_par_site *site, kiln_val target,
                               const kiln_string *ikey, kiln_val value) {
    kiln_object *object;
    kiln_shape *next;
    uint32_t slot;
    uint32_t number = KILN_VAL_IS_NUMBER(value) ? 1u : 0u;
    (void)arena;
    if (!KILN_VAL_IS_OBJECT(target) || KILN_VAL_IS_CLOSURE(value)) return 0;
    object = (kiln_object *)KILN_VAL_PTR(target);
    if (kiln_c99_gc_payload_kind(object) != KILN_GC_OBJECT || object->shape == NULL ||
        object->properties != NULL || object->elements != NULL || object->frozen ||
        object->not_extensible || object->accessor_count != 0) {
        return 0;
    }
    if (site != NULL && site->shape == object->shape && site->to != NULL && site->number == number) {
        if (site->slot >= object->slot_capacity) return 0;
        object->slots[site->slot] = value;
        object->shape = (kiln_shape *)site->to;
        return 1;
    }
    if (!kiln_c99_par_key_plain(ikey)) return 0;
    if (kiln_c99_shape_find_slot(object->shape, ikey, &slot)) {
        /* The member is there already (`{ ...r, qty: 1 }` over an `r` with a `qty`): its value
         * changes in place, unless the slot carries attributes, or holds doubles and the value is
         * not one -- the sequential definition then leaves shape mode. */
        if (slot >= object->slot_capacity || slot >= 32u ||
            (object->shape->special_mask & (1u << slot)) != 0 ||
            ((object->shape->double_slots_mask & (1u << slot)) != 0 && !number)) {
            return 0;
        }
        next = object->shape;
    } else {
        if (!kiln_c99_par_shape_step(object->shape, ikey, (int)number, &next)) return 0;
        slot = next->slot_index;
        if (slot >= object->slot_capacity) return 0;
    }
    if (site != NULL) {
        site->shape = object->shape;
        site->to = next;
        site->slot = slot;
        site->number = number;
    }
    object->slots[slot] = value;
    object->shape = next;
    return 1;
}

/* A member whose key is computed (`{ [k]: v }`): a string key the realm has interned, defined as
 * above. A key the realm never interned names no transition anywhere, and interning it is the
 * realm thread's; a number, a symbol or an object key declines too. */
static int kiln_c99_par_define_computed(kiln_c99_par_arena *arena, kiln_val target, kiln_val key,
                                        kiln_val value) {
    const kiln_string *ikey;
    if (arena == NULL || !KILN_VAL_IS_STRING(key)) return 0;
    ikey = kiln_c99_interned_key_in((kiln_gc_heap *)arena->heap,
                                    (const kiln_string *)KILN_VAL_PTR(key));
    if (ikey == NULL) return 0;
    return kiln_c99_par_define(arena, NULL, target, ikey, value);
}

/* `{ ...source }` into `target`, a literal this worker built that has no member yet. */
static int kiln_c99_par_spread(kiln_c99_par_arena *arena, kiln_val target, kiln_val source) {
    kiln_object *object;
    kiln_object *dest;
    kiln_shape *sshape;
    size_t scount;
    (void)arena;
    if (source == KILN_VAL_UNDEFINED || source == KILN_VAL_NULL) return KILN_VAL_IS_OBJECT(target);
    if (!KILN_VAL_IS_OBJECT(source) || !KILN_VAL_IS_OBJECT(target)) return 0;
    object = (kiln_object *)KILN_VAL_PTR(source);
    dest = (kiln_object *)KILN_VAL_PTR(target);
    if (object == NULL || dest == NULL || kiln_c99_gc_payload_kind(object) != KILN_GC_OBJECT ||
        kiln_c99_gc_payload_kind(dest) != KILN_GC_OBJECT) {
        return 0;
    }
    if (object->properties != NULL || object->shape == NULL || object->elements != NULL ||
        object->accessor_count != 0 || object->arguments_brand || object->error_brand ||
        object->module_namespace) {
        return 0;
    }
    if (dest->properties != NULL || dest->shape == NULL || dest->shape->property_count != 0 ||
        dest->elements != NULL || dest->frozen || dest->not_extensible || dest->accessor_count != 0) {
        return 0;
    }
    sshape = object->shape;
    scount = sshape->property_count;
    if (sshape->special_mask != 0 || sshape->has_symbol_key || scount > dest->slot_capacity ||
        scount > object->slot_capacity) {
        return 0;
    }
    if (scount > 0) memcpy(dest->slots, object->slots, scount * sizeof(kiln_val));
    dest->shape = sshape;
    return 1;
}

static int kiln_c99_par_array(kiln_c99_par_arena *arena, const kiln_val *elements, size_t count,
                              kiln_val *out) {
    size_t inline_capacity = sizeof(((kiln_array *)0)->inline_elements) / sizeof(kiln_val);
    size_t in_block = count > inline_capacity ? count : 0;
    kiln_array *array;
    size_t index;
    int numeric = 1;
    if (count > KILN_C99_ARRAY_INBLOCK_MAX) return 0;
    array = (kiln_array *)kiln_c99_par_alloc(arena, KILN_GC_ARRAY,
                                             sizeof *array + in_block * sizeof(kiln_val));
    if (array == NULL) return 0;
    /* No `elements` (tier 4's `map`/`filter`/`split`): holes, which the caller overwrites before
     * the array is seen, and `kiln_c99_par_array_seal` sets `is_numeric` from what it wrote. */
    for (index = 0; elements != NULL && index < count; ++index) {
        numeric &= KILN_VAL_IS_NUMBER(elements[index]) ? 1 : 0;
    }
    if (elements == NULL) numeric = 0;
    array->elements = NULL;
    array->count = 0;
    array->capacity = 0;
    array->properties = kiln_c99_undefined();
    array->frozen = 0;
    array->not_extensible = 0;
    array->match_index = -1;
    array->match_input = kiln_c99_undefined();
    array->match_groups = kiln_c99_undefined();
    array->is_numeric = numeric;
    array->candidate_elements = 0;
    if (in_block > 0) {
        array->elements = (kiln_val *)(array + 1);
        array->capacity = in_block;
    } else if (count > 0) {
        array->elements = array->inline_elements;
        array->capacity = inline_capacity;
    }
    if (count > 0 && elements != NULL) memcpy(array->elements, elements, count * sizeof(kiln_val));
    for (index = 0; elements == NULL && index < count; ++index) array->elements[index] = KILN_VAL_HOLE;
    array->count = count;
    *out = KILN_VAL_TAG_PTR(KILN_TAG_BUFFER_PREFIX, array);
    return 1;
}

/* The units of `value` as `ToString` gives them, for a primitive whose conversion runs no code:
 * a string is itself; a number, a boolean, undefined and null are spelled into `buffer`. 0 for
 * anything else (an object's `toString`, a symbol, a BigInt). */
static int kiln_c99_par_text(kiln_val value, char buffer[32], kiln_string *view,
                             const kiln_string **out) {
    const char *spelled;
    size_t length;
    if (KILN_VAL_IS_STRING(value)) {
        *out = (const kiln_string *)KILN_VAL_PTR(value);
        return *out != NULL;
    }
    if (KILN_VAL_IS_NUMBER(value)) {
        double d;
        memcpy(&d, &value, sizeof d);
        length = kiln_c99_double_to_js(d, buffer);
        spelled = buffer;
    } else if (value == KILN_VAL_TRUE) {
        spelled = "true";
        length = 4;
    } else if (value == KILN_VAL_FALSE) {
        spelled = "false";
        length = 5;
    } else if (value == KILN_VAL_UNDEFINED) {
        spelled = "undefined";
        length = 9;
    } else if (value == KILN_VAL_NULL) {
        spelled = "null";
        length = 4;
    } else {
        return 0;
    }
    view->unit_len = (uint32_t)length;
    view->flags = KILN_STRING_LATIN1;
    view->data = spelled;
    *out = view;
    return 1;
}

/* A flat string of `left` then `right`, in the arena. */
static int kiln_c99_par_join(kiln_c99_par_arena *arena, const kiln_string *left,
                             const kiln_string *right, kiln_val *out) {
    size_t total = (size_t)left->unit_len + (size_t)right->unit_len;
    uint32_t flags = kiln_string_is_utf16(left) || kiln_string_is_utf16(right) ? KILN_STRING_UTF16
                                                                              : KILN_STRING_LATIN1;
    size_t storage;
    kiln_string *joined;
    if (total > UINT32_MAX || !kiln_string_storage_size((uint32_t)total, flags, &storage)) return 0;
    joined = (kiln_string *)kiln_c99_par_alloc(arena, KILN_GC_STRING, storage + 1);
    if (joined == NULL) return 0;
    joined->unit_len = (uint32_t)total;
    joined->flags = flags;
    joined->data = (const void *)(joined + 1);
    if (flags == KILN_STRING_UTF16) {
        uint16_t *units = (uint16_t *)(joined + 1);
        size_t index;
        for (index = 0; index < left->unit_len; ++index) units[index] = kiln_string_unit_at(left, index);
        for (index = 0; index < right->unit_len; ++index) {
            units[left->unit_len + index] = kiln_string_unit_at(right, index);
        }
        ((char *)(joined + 1))[storage - sizeof(kiln_string)] = '\0';
    } else {
        char *bytes = (char *)(joined + 1);
        if (left->unit_len != 0) memcpy(bytes, left->data, left->unit_len);
        if (right->unit_len != 0) memcpy(bytes + left->unit_len, right->data, right->unit_len);
        bytes[total] = '\0';
    }
    *out = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, joined);
    return 1;
}

/* `String(value)`, and a template literal's substitution. */
static int kiln_c99_par_to_string(kiln_c99_par_arena *arena, kiln_val value, kiln_val *out) {
    char buffer[32];
    kiln_string view;
    kiln_string empty;
    const kiln_string *text;
    if (KILN_VAL_IS_STRING(value)) {
        *out = value;
        return 1;
    }
    if (arena == NULL || !kiln_c99_par_text(value, buffer, &view, &text)) return 0;
    empty.unit_len = 0;
    empty.flags = KILN_STRING_LATIN1;
    empty.data = "";
    return kiln_c99_par_join(arena, text, &empty, out);
}

/* `a + b` where one side is a string (`kiln_c99_par_add`). */
static int kiln_c99_par_concat(kiln_c99_par_arena *arena, kiln_val a, kiln_val b, kiln_val *out) {
    char left_buffer[32];
    char right_buffer[32];
    kiln_string left_view;
    kiln_string right_view;
    const kiln_string *left;
    const kiln_string *right;
    if (arena == NULL || !kiln_c99_par_text(a, left_buffer, &left_view, &left) ||
        !kiln_c99_par_text(b, right_buffer, &right_view, &right)) {
        return 0;
    }
    return kiln_c99_par_join(arena, left, right, out);
}

/* `parallelFlatMap`'s answer from the callback's answers (`count` of them in `mapped`, a hole
 * where `flatMap` skips an element), on the realm thread once the arenas have joined: each answer
 * that is an array contributes its elements -- holes skipped, as `FlattenIntoArray` skips them --
 * and every other answer itself. An answer `IsArray` would look through (a proxy) declines, and
 * the call runs `flatMap`. `mapped` stays live across the allocation below, so the collector the
 * allocation may run marks every answer through it. */
static int kiln_c99_par_flatten(kiln_val mapped, size_t count, kiln_val *out) {
    const kiln_array *answers;
    kiln_value built;
    kiln_array *flat;
    size_t total = 0;
    size_t index;
    size_t at = 0;
    if ((mapped & KILN_TAG_MASK) != KILN_TAG_BUFFER_PREFIX) return 0;
    answers = (const kiln_array *)KILN_VAL_PTR(mapped);
    if (count > answers->count) return 0;
    for (index = 0; index < count; ++index) {
        kiln_val answer = answers->elements[index];
        if (answer == KILN_VAL_HOLE) continue;
        if ((answer & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX &&
            kiln_c99_gc_payload_kind(KILN_VAL_PTR(answer)) == KILN_GC_ARRAY) {
            const kiln_array *inner = (const kiln_array *)KILN_VAL_PTR(answer);
            size_t k;
            for (k = 0; k < inner->count; ++k) total += inner->elements[k] != KILN_VAL_HOLE;
        } else if (KILN_VAL_IS_OBJECT(answer) &&
                   kiln_c99_gc_payload_kind(KILN_VAL_PTR(answer)) == KILN_GC_PROXY) {
            return 0;
        } else {
            total += 1;
        }
    }
    if (kiln_c99_new_array_with_capacity(&built, total, 0) != KILN_C99_OK) return 0;
    flat = kiln_c99_array_of(built);
    for (index = 0; index < count; ++index) {
        kiln_val answer = answers->elements[index];
        if (answer == KILN_VAL_HOLE) continue;
        if ((answer & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX &&
            kiln_c99_gc_payload_kind(KILN_VAL_PTR(answer)) == KILN_GC_ARRAY) {
            const kiln_array *inner = (const kiln_array *)KILN_VAL_PTR(answer);
            size_t k;
            for (k = 0; k < inner->count; ++k) {
                if (inner->elements[k] != KILN_VAL_HOLE) flat->elements[at++] = inner->elements[k];
            }
        } else {
            flat->elements[at++] = answer;
        }
    }
    flat->count = at;
    flat->is_numeric = 1;
    for (index = 0; index < at && flat->is_numeric; ++index) {
        flat->is_numeric = KILN_VAL_IS_NUMBER(flat->elements[index]);
    }
    *out = kiln_val_from_boundary(built);
    return 1;
}
