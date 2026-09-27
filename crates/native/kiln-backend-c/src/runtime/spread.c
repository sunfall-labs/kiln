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
    if (dest->slots == dest->inline_slots) {
        new_slots = (kiln_val *)malloc(new_cap * sizeof(kiln_val));
        if (new_slots == NULL) return KILN_C99_OUT_OF_MEMORY;
        memcpy(new_slots, dest->inline_slots, KILN_C99_INLINE_SLOTS * sizeof(kiln_val));
    } else if (dest->slots == NULL) {
        new_slots = (kiln_val *)malloc(new_cap * sizeof(kiln_val));
        if (new_slots == NULL) return KILN_C99_OUT_OF_MEMORY;
    } else {
        new_slots = (kiln_val *)realloc(dest->slots, new_cap * sizeof(kiln_val));
        if (new_slots == NULL) return KILN_C99_OUT_OF_MEMORY;
    }
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
    if (site == NULL || site->pic_count == 0) return 0;
    entry = &site->entries[0];
    if (object->shape != entry->shape) return 0;
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
    kiln_c99_pic_entry *entry;
    if (site == NULL) return;
    entry = &site->entries[0];
    memset(entry, 0, sizeof *entry);
    entry->shape = from;
    entry->transition_to = to;
    entry->slot = slot;
    site->pic_count = 1;
    kiln_c99_site_learned(site);
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

typedef struct kiln_c99_clone_table {
    kiln_val *keys;
    kiln_val *values;
    size_t capacity;
    size_t count;
    kiln_val inline_keys[KILN_C99_CLONE_INLINE];
    kiln_val inline_values[KILN_C99_CLONE_INLINE];
} kiln_c99_clone_table;

#define KILN_C99_CLONE_MAX_DEPTH 512
#define KILN_C99_CLONE_BAIL 1

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

/* One value: `*out` is its copy, or the status is KILN_C99_CLONE_BAIL. */
static int kiln_c99_clone_data_val(kiln_c99_clone_table *table, kiln_val value, int depth,
                                   kiln_val *out) {
    void *ptr;
    uint32_t kind;
    int status;
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
        kiln_value made;
        kiln_object *copy;
        kiln_val copy_val;
        size_t count;
        size_t i;
        if (source->elements != NULL || source->accessor_count != 0 ||
            source->prototype.tag != KILN_VALUE_UNDEFINED || source->arguments_brand ||
            source->error_brand || source->module_namespace ||
            (shape != NULL && (source->properties != NULL || shape->has_symbol_key ||
                               shape->special_mask != 0))) {
            return KILN_C99_CLONE_BAIL;
        }
        if (shape == NULL) {
            /* List mode: every property must be an enumerable data property with a string key
             * (a symbol key is stored non-enumerable, so this refuses those too). */
            for (i = 0; i < source->count; ++i) {
                const kiln_property *property = &source->properties[i];
                if (!property->enumerable || property->accessor || property->refused) {
                    return KILN_C99_CLONE_BAIL;
                }
            }
        }
        status = kiln_c99_new_object(&made);
        if (status != KILN_C99_OK) return status;
        copy_val = kiln_val_from_boundary(made);
        copy = (kiln_object *)KILN_VAL_PTR(copy_val);
        status = kiln_c99_clone_table_put(table, value, copy_val);
        if (status != KILN_C99_OK) return status;
        if (shape == NULL) shape = kiln_c99_clone_list_shape(source, copy->shape);
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
                if (KILN_VAL_IS_NUMBER(item) || KILN_VAL_IS_STRING(item) || item == KILN_VAL_UNDEFINED ||
                    item == KILN_VAL_NULL || KILN_VAL_IS_BOOLEAN(item)) {
                    continue;
                }
                status = kiln_c99_clone_data_val(table, item, depth + 1, &cloned);
                if (status != KILN_C99_OK) return status;
                copy->slots[i] = cloned;
            }
        } else {
            count = source->count;
            if (count > 0) {
                kiln_property *properties = (kiln_property *)malloc(count * sizeof *properties);
                if (properties == NULL) return KILN_C99_OUT_OF_MEMORY;
                memcpy(properties, source->properties, count * sizeof *properties);
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
                kiln_value item = copy->properties[i].value;
                kiln_val cloned;
                if (item.tag != KILN_VALUE_OBJECT && item.tag != KILN_VALUE_ARRAY) {
                    if (item.tag == KILN_VALUE_NUMBER || item.tag == KILN_VALUE_STRING ||
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
                copy->properties[i].value = kiln_val_to_boundary(cloned);
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
        if (source->properties.tag != KILN_VALUE_UNDEFINED || source->match_index != -1) {
            return KILN_C99_CLONE_BAIL;
        }
        status = kiln_c99_new_array_with_capacity(&made, count, source->is_numeric);
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
                if (KILN_VAL_IS_NUMBER(item) || KILN_VAL_IS_STRING(item) || item == KILN_VAL_HOLE ||
                    item == KILN_VAL_UNDEFINED || item == KILN_VAL_NULL || KILN_VAL_IS_BOOLEAN(item)) {
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

static int kiln_c99_clone_data_raw(kiln_value *out, const kiln_value *args, size_t argc) {
    kiln_c99_clone_table table;
    kiln_val root = KILN_VAL_UNDEFINED;
    int status;
    *out = kiln_c99_undefined();
    if (argc == 0) return KILN_C99_OK;
    memset(table.inline_keys, 0, sizeof table.inline_keys);
    table.keys = table.inline_keys;
    table.values = table.inline_values;
    table.capacity = KILN_C99_CLONE_INLINE;
    table.count = 0;
    status = kiln_c99_clone_data_val(&table, kiln_val_from_boundary(args[0]), 0, &root);
    if (table.keys != table.inline_keys) {
        free(table.keys);
        free(table.values);
    }
    if (status == KILN_C99_CLONE_BAIL) return KILN_C99_OK;
    if (status != KILN_C99_OK) return status;
    *out = kiln_val_to_boundary(root);
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
    if (object->shape != NULL) {
        kiln_c99_ensure_dictionary(object);
    }
    /* The count is read each time round because a getter can add a property to the source; the
     * bound is the source's *original* length, so a property the getter appends is not copied,
     * which is what stops an appending getter from looping. */
    {
        size_t length = object->count;
        for (index = 0; index < length && index < object->count; ++index) {
            kiln_value copied;
            kiln_value symbol;
            int status;
            if (!object->properties[index].enumerable) {
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
                if (!kiln_c99_symbol_of_key_view(object->properties[index].key, &symbol) ||
                    object->properties[index].symbol_hidden) {
                    continue;
                }
            }
            if (object->properties[index].accessor) {
                if (object->properties[index].getter.tag == KILN_VALUE_UNDEFINED) {
                    /* Setter-only: reading it answers `undefined`, and `undefined` is what a
                     * spread copies. Skipping the key instead left the property *absent* --
                     * `"gone" in { ...source }` said false where node says true, because
                     * `CopyDataProperties` creates a property for every enumerable own key it
                     * was given, whatever the read produced. */
                    status = kiln_c99_put_mode_key(host, target, object->properties[index].key,
                                                   object->properties[index].key,
                                                   kiln_c99_undefined(), 0, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                    continue;
                }
                /* The receiver is the *source*, not the store it was found in: a getter a
                 * function's statics declare has to see the function. */
                status = kiln_c99_call(host, &copied, object->properties[index].getter, source,
                                       NULL, 0, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            } else {
                copied = object->properties[index].value;
            }
            status = kiln_c99_put_mode_key(host, target, object->properties[index].key,
                                           object->properties[index].key, copied, 0, depth,
                                           thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    return KILN_C99_OK;
}
