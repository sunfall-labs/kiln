
static struct kiln_shape *kiln_c99_shape_root(void) {
    return &kiln_c99_root_shape;
}

static int kiln_c99_shape_find_slot(const kiln_shape *shape, const kiln_string *ikey, uint32_t *out_slot) {
    uint32_t i;
    if (shape == NULL || ikey == NULL) {
        return 0;
    }
    if (shape->ikey == ikey) {
        if (out_slot) {
            *out_slot = shape->slot_index;
        }
        return 1;
    }
    for (i = 0; i < shape->property_count; ++i) {
        if (shape->keys[i] == ikey) {
            if (out_slot) {
                *out_slot = i;
            }
            return 1;
        }
    }
    return 0;
}

/* The slot of the key spelt `bytes` in `shape`, or UINT32_MAX when the shape has no such key. A
 * spelling the key table has never seen is a key no shape holds, so the lookup never interns. A
 * sunk spread (see `emit_sunk_bridge` in the emitter) learns its source's slots through this,
 * once per shape it meets. */
static uint32_t kiln_c99_shape_slot_named(const kiln_shape *shape, const char *bytes, size_t len) {
    const kiln_string *ikey = kiln_c99_interned(bytes, len);
    uint32_t slot;
    if (ikey == NULL || !kiln_c99_shape_find_slot(shape, ikey, &slot)) {
        return UINT32_MAX;
    }
    return slot;
}

static void kiln_c99_pin_interned_key(const kiln_string *ikey) {
    kiln_gc_heap *heap;
    kiln_key_entry *entry;
    size_t i;
    if (ikey == NULL || ikey == kiln_c99_key_empty) return;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return;
    entry = kiln_c99_key_probe(&heap->keys, ikey, kiln_c99_key_hash(ikey));
    if (entry != NULL) {
        entry->pinned = 1;
        return;
    }
    for (i = 0; i < heap->keys.capacity; ++i) {
        if (heap->keys.entries[i].string == ikey) {
            heap->keys.entries[i].pinned = 1;
            return;
        }
    }
}

/* Past this many branches a shape indexes them by key pointer (`edge_table`). */
#define KILN_SHAPE_TABLE_FROM 8u
/* How many branches one shape may grow before an object that would take a new one goes to
 * dictionary mode instead. With the index a branch costs the same to find among a thousand as
 * among eight, so this bounds memory rather than lookup time: a program that builds objects keyed
 * by unbounded data (`{[id]: x}` per request) stops minting shapes here. */
#define KILN_SHAPE_MAX_BRANCHES 1024u
/* And how many shapes the whole process may mint: the same bound for a program whose unbounded
 * keys arrive one level down, where no single shape reaches the per-shape limit. Shapes are never
 * freed and pin their keys, so this is the ceiling on what the tree can hold. */
#define KILN_SHAPE_MAX_TOTAL 32768u

static uint32_t kiln_c99_shape_total;

static inline uint32_t kiln_c99_shape_edge_hash(const kiln_string *ikey) {
    uint64_t bits = (uint64_t)(uintptr_t)ikey;
    bits ^= bits >> 17;
    bits *= UINT64_C(0x9E3779B97F4A7C15);
    return (uint32_t)(bits >> 32);
}

/* Claim a free slot of `table` for `next` (keyed by `next->ikey`) with one compare-and-swap. */
static void kiln_c99_shape_edge_claim(kiln_shape_edge_table *table, kiln_shape *next) {
    uint32_t at = kiln_c99_shape_edge_hash(next->ikey) & table->mask;
    for (;;) {
        kiln_shape *expected = NULL;
        if (__atomic_compare_exchange_n(&table->edges[at], &expected, next, 0, __ATOMIC_RELEASE,
                                        __ATOMIC_RELAXED)) {
            return;
        }
        at = (at + 1) & table->mask;
    }
}

/* Record `next` in `shape`'s index, growing it first when it is past half full. Returns 0 only
 * when a table the shape needs could not be allocated; the caller then refuses the transition, so
 * the list and the index never disagree.
 *
 * ADR 0071's publishing rule: a grown table is built completely, then published with one release
 * store of the shape's pointer; a slot of a live table is claimed with a compare-and-swap. A
 * replaced table is never freed -- shapes are never freed either, and a reader on another thread
 * may still be probing it; each table is at most half the next, so what is kept is bounded by the
 * live one. (Retiring it at a turn boundary, per the ADR, is the refinement once the tree is
 * written by several threads.) */
static int kiln_c99_shape_index_edge(kiln_shape *shape, kiln_shape *next) {
    uint32_t need = shape->num_transitions + 1;
    kiln_shape_edge_table *table = __atomic_load_n(&shape->edge_table, __ATOMIC_ACQUIRE);
    if (table == NULL || need * 2 > table->mask + 1) {
        uint32_t cap = table == NULL ? 32u : (table->mask + 1) * 2;
        kiln_shape_edge_table *grown;
        kiln_shape_transition *tr;
        size_t i;
        while (need * 2 > cap) cap *= 2;
        grown = (kiln_shape_edge_table *)calloc(1, sizeof *grown + cap * sizeof(kiln_shape *));
        if (grown == NULL) return 0;
        grown->mask = cap - 1;
        /* Every existing branch -- the two inline ones and the list -- and the new one. */
        for (i = 0; i < 2 && i < shape->num_transitions; ++i) {
            kiln_c99_shape_edge_claim(grown, shape->inline_trans_next[i]);
        }
        for (tr = shape->transitions; tr != NULL; tr = tr->next_transition) {
            kiln_c99_shape_edge_claim(grown, tr->next);
        }
        kiln_c99_shape_edge_claim(grown, next);
        __atomic_store_n(&shape->edge_table, grown, __ATOMIC_RELEASE);
        return 1;
    }
    kiln_c99_shape_edge_claim(table, next);
    return 1;
}

static int kiln_c99_shape_transition(kiln_shape *current, const kiln_string *ikey, int is_double, kiln_shape **out_shape) {
    kiln_shape_transition *tr;
    kiln_shape *next_shape;
    size_t i;
    if (current == NULL || ikey == NULL || current->property_count >= KILN_SHAPE_MAX_FAST_PROPERTIES) {
        return 0;
    }
    uint32_t expected_bit = is_double ? (1u << current->property_count) : 0;
    const kiln_shape_edge_table *table = __atomic_load_n(&current->edge_table, __ATOMIC_ACQUIRE);
    if (table != NULL) {
        /* The index holds every branch (it is built from the list and kept in step with it), so
         * a probe that reaches an empty slot has seen every edge under this key. Two edges can
         * share a key -- one whose new slot holds a double and one whose does not -- which is why
         * the probe checks the mask and keeps going rather than stopping at the key. */
        uint32_t at = kiln_c99_shape_edge_hash(ikey) & table->mask;
        kiln_shape *seen;
        while ((seen = __atomic_load_n(&table->edges[at], __ATOMIC_ACQUIRE)) != NULL) {
            if (seen->ikey == ikey &&
                (seen->double_slots_mask & (1u << current->property_count)) == expected_bit) {
                *out_shape = seen;
                return 1;
            }
            at = (at + 1) & table->mask;
        }
    } else {
        for (i = 0; i < 2 && i < current->num_transitions; ++i) {
            if (current->inline_trans_ikey[i] == ikey) {
                kiln_shape *cand = current->inline_trans_next[i];
                if ((cand->double_slots_mask & (1u << current->property_count)) == expected_bit) {
                    *out_shape = cand;
                    return 1;
                }
            }
        }
        for (tr = current->transitions; tr != NULL; tr = tr->next_transition) {
            if (tr->ikey == ikey) {
                kiln_shape *cand = tr->next;
                if ((cand->double_slots_mask & (1u << current->property_count)) == expected_bit) {
                    *out_shape = cand;
                    return 1;
                }
            }
        }
    }
    /* A shape stops growing branches at `KILN_SHAPE_MAX_BRANCHES`, and the process at
     * `KILN_SHAPE_MAX_TOTAL` shapes: past either, the object goes to dictionary mode.
     *
     * This used to be 32 branches, with an exception for class brands, and the root shape was
     * full before the program ran: the bootstrap alone takes some thirty first keys (`constructor`,
     * `prototype`, `value`, `name`, `log`, ...), so every object literal whose first key was not
     * one of those -- `{ a: 1 }`, `{ headers }`, `{ status }` -- became a dictionary at its first
     * member. That made the literal site's shape cache (`ku_new_object_literal_fast`) and every
     * shape-guarded read cache useless for exactly the objects handler code makes most. */
    if (current->num_transitions >= KILN_SHAPE_MAX_BRANCHES ||
        kiln_c99_shape_total >= KILN_SHAPE_MAX_TOTAL) {
        return 0;
    }
    next_shape = (kiln_shape *)calloc(1, sizeof(kiln_shape));
    if (next_shape == NULL) {
        return 0;
    }
    kiln_c99_pin_interned_key(ikey);
    next_shape->parent = current;
    next_shape->ikey = ikey;
    next_shape->has_symbol_key = current->has_symbol_key ||
                                 (ikey->unit_len >= 2 && kiln_string_unit_at(ikey, 0) == '@' &&
                                  kiln_string_unit_at(ikey, 1) == '@');
    next_shape->slot_index = current->property_count;
    next_shape->property_count = current->property_count + 1;
    next_shape->num_transitions = 0;
    next_shape->double_slots_mask = current->double_slots_mask | expected_bit;
    memcpy((void *)next_shape->keys, (const void *)current->keys,
           current->property_count * sizeof(const kiln_string *));
    next_shape->keys[current->property_count] = ikey;

    {
        kiln_shape_transition *new_tr = NULL;
        if (current->num_transitions >= 2) {
            new_tr = (kiln_shape_transition *)calloc(1, sizeof(kiln_shape_transition));
            if (new_tr == NULL) {
                free(next_shape);
                return 0;
            }
        }
        if (current->num_transitions + 1 > KILN_SHAPE_TABLE_FROM &&
            !kiln_c99_shape_index_edge(current, next_shape)) {
            free(new_tr);
            free(next_shape);
            return 0;
        }
        if (new_tr == NULL) {
            current->inline_trans_ikey[current->num_transitions] = ikey;
            current->inline_trans_next[current->num_transitions] = next_shape;
        } else {
            new_tr->ikey = ikey;
            new_tr->next = next_shape;
            new_tr->next_transition = current->transitions;
            current->transitions = new_tr;
        }
    }
    current->num_transitions++;
    kiln_c99_shape_total++;

    *out_shape = next_shape;
    return 1;
}

static int kiln_c99_ensure_dictionary(kiln_object *object) {
    kiln_c99_prototype_touched(object);
    kiln_shape *shape;
    size_t i;
    int status;
    size_t active_elements = 0;
    size_t total_properties;
    size_t pos = 0;
    if (object == NULL) {
        return KILN_C99_OK;
    }
    shape = object->shape;
    if (shape == NULL) {
        return KILN_C99_OK;
    }
    if (object->elements != NULL) {
        for (i = 0; i < object->element_count; ++i) {
            if (object->elements[i] != KILN_VAL_HOLE) {
                active_elements++;
            }
        }
    }
    total_properties = shape->property_count + active_elements;
    if (total_properties > object->capacity) {
        status = kiln_c99_reserve((void **)&object->properties, &object->capacity,
                                  total_properties, sizeof *object->properties);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    if (object->elements != NULL) {
        for (i = 0; i < object->element_count; ++i) {
            if (object->elements[i] == KILN_VAL_HOLE) continue;
            char num_buf[32];
            size_t nlen = kiln_c99_double_to_js((double)i, num_buf);
            const kiln_string *ikey = kiln_c99_intern(num_buf, nlen);
            object->properties[pos].key = ikey;
            object->properties[pos].key_len = nlen;
            object->properties[pos].enumerable = 1;
            object->properties[pos].writable = 1;
            object->properties[pos].configurable = 1;
            object->properties[pos].symbol_hidden = 0;
            object->properties[pos].accessor = 0;
            object->properties[pos].refused = 0;
            object->properties[pos].value = kiln_val_to_boundary(object->elements[i]);
            object->properties[pos].getter = kiln_c99_undefined();
            object->properties[pos].setter = kiln_c99_undefined();
            pos++;
        }
        free(object->elements);
        object->elements = NULL;
        object->element_count = 0;
        object->element_capacity = 0;
    }
    for (i = 0; i < shape->property_count; ++i) {
        const kiln_string *key = shape->keys[i];
        size_t key_len = kiln_c99_key_len(key);
        object->properties[pos].key = key;
        object->properties[pos].key_len = key_len;
        object->properties[pos].enumerable = !(key_len >= 2 && kiln_string_unit_at(key, 0) == '@' &&
                                              kiln_string_unit_at(key, 1) == '@');
        object->properties[pos].writable = 1;
        object->properties[pos].configurable = 1;
        object->properties[pos].symbol_hidden = 0;
        object->properties[pos].accessor = 0;
        object->properties[pos].refused = 0;
        object->properties[pos].value = (object->slots != NULL && i < object->slot_capacity)
                                          ? kiln_val_to_boundary(object->slots[i])
                                          : kiln_c99_undefined();
        object->properties[pos].getter = kiln_c99_undefined();
        object->properties[pos].setter = kiln_c99_undefined();
        pos++;
    }
    object->count = pos;
    if (object->slots != NULL && object->slots != object->inline_slots) {
        free(object->slots);
    }
    object->slots = NULL;
    object->slot_capacity = 0;
    object->shape = NULL;
    return KILN_C99_OK;
}

static int kiln_c99_new_object(kiln_value *out) {
    /* Every field is written below before anything can observe the object, so the block is
     * taken unzeroed: the zeroing was a second pass over two hundred bytes per object. */
    kiln_object *object = (kiln_object *)kiln_c99_gc_alloc_hot(KILN_GC_OBJECT, sizeof *object, 0);
    if (object == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    object->properties = NULL;
    object->count = 0;
    object->capacity = 0;
    object->prototype = kiln_c99_undefined();
    object->frozen = 0;
    object->not_extensible = 0;
    object->module_namespace = 0;
    object->arguments_brand = 0;
    object->error_brand = 0;
    object->cache_holder = 0;
    object->accessor_count = 0;
    object->read_only_seen = 0;
    object->shape = &kiln_c99_root_shape;
    object->slots = object->inline_slots;
    object->slot_capacity = KILN_C99_INLINE_SLOTS;
    for (size_t slot_i = 0; slot_i < KILN_C99_INLINE_SLOTS; ++slot_i) {
        object->inline_slots[slot_i] = KILN_VAL_UNDEFINED;
    }
    object->elements = NULL;
    object->element_count = 0;
    object->element_capacity = 0;
    *out = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)object, NULL, 0);
    return KILN_C99_OK;
}

/* An object whose property array is sized once, for a caller that already knows how many members
 * it is about to add.
 *
 * `kiln_c99_reserve` doubles from four, which is the right policy for a program building an
 * object a member at a time and the wrong one for the runtime filling a fixed template: a
 * `node:http` request with twelve own properties walked 4 -> 8 -> 16, paying a `realloc` and a
 * copy at each step to arrive at the capacity it could have asked for. The capacity is a hint,
 * not a limit -- passing a number too small only restores the doubling. */
static int kiln_c99_new_object_with_capacity(kiln_value *out, size_t capacity) {
    kiln_object *object;
    int status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) {
        return status;
    }
    object = kiln_c99_object_of(*out);
    if (capacity > KILN_C99_INLINE_SLOTS) {
        kiln_val *new_slots = (kiln_val *)malloc(capacity * sizeof(kiln_val));
        size_t i;
        if (new_slots == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (i = 0; i < capacity; ++i) {
            new_slots[i] = KILN_VAL_UNDEFINED;
        }
        object->slots = new_slots;
        object->slot_capacity = capacity;
    }
    return KILN_C99_OK;
}

static int kiln_c99_new_object_tenured(kiln_value *out) {
    kiln_object *object = (kiln_object *)kiln_c99_gc_alloc_tenured(KILN_GC_OBJECT, sizeof *object);
    if (object == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    object->properties = NULL;
    object->count = 0;
    object->capacity = 0;
    object->prototype = kiln_c99_undefined();
    object->frozen = 0;
    object->not_extensible = 0;
    object->module_namespace = 0;
    object->arguments_brand = 0;
    object->error_brand = 0;
    object->cache_holder = 0;
    object->accessor_count = 0;
    object->read_only_seen = 0;
    object->shape = &kiln_c99_root_shape;
    object->slots = object->inline_slots;
    object->slot_capacity = KILN_C99_INLINE_SLOTS;
    for (size_t slot_i = 0; slot_i < KILN_C99_INLINE_SLOTS; ++slot_i) {
        object->inline_slots[slot_i] = KILN_VAL_UNDEFINED;
    }
    object->elements = NULL;
    object->element_count = 0;
    object->element_capacity = 0;
    *out = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)object, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_new_object_with_capacity_tenured(kiln_value *out, size_t capacity) {
    kiln_object *object;
    int status = kiln_c99_new_object_tenured(out);
    if (status != KILN_C99_OK) {
        return status;
    }
    object = kiln_c99_object_of(*out);
    if (capacity > KILN_C99_INLINE_SLOTS) {
        kiln_val *new_slots = (kiln_val *)malloc(capacity * sizeof(kiln_val));
        size_t i;
        if (new_slots == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (i = 0; i < capacity; ++i) {
            new_slots[i] = KILN_VAL_UNDEFINED;
        }
        object->slots = new_slots;
        object->slot_capacity = capacity;
    }
    return KILN_C99_OK;
}
