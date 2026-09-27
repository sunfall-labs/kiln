
/* The one process-wide root (ADR 0071), in the shared block every unit reaches. */
static inline struct kiln_shape *kiln_c99_shape_root(void) {
    return &kiln_c99_process()->root;
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

/* `@@private:<scope>:%brand`, the key a class constructor stores first on every instance (the
 * spelling is `kiln_ir`'s `private_key(scope, "%brand")`). */
static int kiln_c99_is_class_brand_key(const kiln_string *key) {
    static const char prefix[] = "@@private:";
    static const char suffix[] = ":%brand";
    size_t prefix_len = sizeof prefix - 1;
    size_t suffix_len = sizeof suffix - 1;
    size_t len;
    size_t index;
    if (key == NULL || kiln_string_is_utf16(key)) return 0;
    len = key->unit_len;
    if (len < prefix_len + suffix_len) return 0;
    for (index = 0; index < prefix_len; ++index) {
        if (kiln_string_latin1(key)[index] != prefix[index]) return 0;
    }
    for (index = 0; index < suffix_len; ++index) {
        if (kiln_string_latin1(key)[len - suffix_len + index] != suffix[index]) return 0;
    }
    return 1;
}

/* ---- Transitions (ADR 0050, shared per ADR 0071) --------------------------------------------
 *
 * A shape's own edges (two inline, the rest a list) name children of the same kind as the edge
 * warrants: a shared shape's own edges are by shared keys to shared children only, published with
 * release stores under the process lock because any realm's thread may add one while another
 * reads; a realm-local shape's own edges lead to realm-local children. A realm-local edge out of
 * a shared shape -- a key of the realm's own, or a child the realm owns -- lives in the realm
 * heap's side table instead, so no realm ever follows an edge into another realm's memory. */
static kiln_shape *kiln_c99_shape_inline_edge(const kiln_shape *current, uint32_t count,
                                              const kiln_string *ikey, uint32_t bit,
                                              uint32_t expected_bit) {
    uint32_t i;
    for (i = 0; i < 2 && i < count; ++i) {
        if (current->inline_trans_ikey[i] == ikey) {
            kiln_shape *cand = current->inline_trans_next[i];
            if ((cand->double_slots_mask & bit) == expected_bit) return cand;
        }
    }
    return NULL;
}

static kiln_shape *kiln_c99_shape_list_edge(const kiln_shape *current, const kiln_string *ikey,
                                            uint32_t bit, uint32_t expected_bit) {
    const kiln_shape_transition *tr;
    for (tr = __atomic_load_n(&current->transitions, __ATOMIC_ACQUIRE); tr != NULL;
         tr = tr->next_transition) {
        if (tr->ikey == ikey) {
            kiln_shape *cand = tr->next;
            if ((cand->double_slots_mask & bit) == expected_bit) return cand;
        }
    }
    return NULL;
}

static size_t kiln_c99_local_edge_home(const kiln_shape *parent, const kiln_string *key,
                                       size_t mask) {
    uint64_t hash = ((uint64_t)(uintptr_t)parent * UINT64_C(0x9e3779b97f4a7c15)) ^
                    ((uint64_t)(uintptr_t)key * UINT64_C(0xff51afd7ed558ccd));
    hash ^= hash >> 29;
    return (size_t)hash & mask;
}

/* The side-table row for `parent` and `key` whose child agrees on the double bit, or with a NULL
 * `key` the parent's count row. */
static kiln_c99_local_edge *kiln_c99_local_edge_row(const kiln_gc_heap *heap,
                                                    const kiln_shape *parent,
                                                    const kiln_string *key, uint32_t bit,
                                                    uint32_t expected_bit) {
    size_t mask;
    size_t at;
    if (heap->local_edge_count == 0) return NULL;
    mask = heap->local_edge_capacity - 1;
    at = kiln_c99_local_edge_home(parent, key, mask);
    for (;;) {
        kiln_c99_local_edge *row = &heap->local_edges[at];
        if (row->parent == NULL) return NULL;
        if (row->parent == parent && row->key == key &&
            (key == NULL || (row->child->double_slots_mask & bit) == expected_bit))
            return row;
        at = (at + 1) & mask;
    }
}

static void kiln_c99_local_edge_place(kiln_c99_local_edge *rows, size_t capacity,
                                      kiln_c99_local_edge row) {
    size_t mask = capacity - 1;
    size_t at = kiln_c99_local_edge_home(row.parent, row.key, mask);
    while (rows[at].parent != NULL) at = (at + 1) & mask;
    rows[at] = row;
}

static int kiln_c99_local_edge_reserve(kiln_gc_heap *heap) {
    kiln_c99_local_edge *grown;
    size_t capacity;
    size_t index;
    if ((heap->local_edge_count + 1) * 2 <= heap->local_edge_capacity) return 1;
    capacity = heap->local_edge_capacity == 0 ? 64 : heap->local_edge_capacity * 2;
    grown = (kiln_c99_local_edge *)calloc(capacity, sizeof *grown);
    if (grown == NULL) return 0;
    for (index = 0; index < heap->local_edge_capacity; ++index)
        if (heap->local_edges[index].parent != NULL)
            kiln_c99_local_edge_place(grown, capacity, heap->local_edges[index]);
    free(heap->local_edges);
    heap->local_edges = grown;
    heap->local_edge_capacity = capacity;
    return 1;
}

/* How many realm-local edges `parent` has in this heap. */
static uint32_t kiln_c99_local_edge_count(const kiln_gc_heap *heap, const kiln_shape *parent) {
    const kiln_c99_local_edge *row = kiln_c99_local_edge_row(heap, parent, NULL, 0, 0);
    return row == NULL ? 0 : (uint32_t)(uintptr_t)row->child;
}

static int kiln_c99_local_edge_insert(kiln_gc_heap *heap, const kiln_shape *parent,
                                      const kiln_string *key, kiln_shape *child) {
    kiln_c99_local_edge *count_row;
    kiln_c99_local_edge row;
    /* Room for the edge and, at most, a new count row. */
    if (!kiln_c99_local_edge_reserve(heap)) return 0;
    heap->local_edge_count += 1;
    if (!kiln_c99_local_edge_reserve(heap)) {
        heap->local_edge_count -= 1;
        return 0;
    }
    row.parent = parent;
    row.key = key;
    row.child = child;
    kiln_c99_local_edge_place(heap->local_edges, heap->local_edge_capacity, row);
    count_row = kiln_c99_local_edge_row(heap, parent, NULL, 0, 0);
    if (count_row != NULL) {
        count_row->child = (kiln_shape *)(uintptr_t)((uintptr_t)count_row->child + 1);
    } else {
        row.key = NULL;
        row.child = (kiln_shape *)(uintptr_t)1;
        kiln_c99_local_edge_place(heap->local_edges, heap->local_edge_capacity, row);
        heap->local_edge_count += 1;
    }
    return 1;
}

/* A shape stops growing branches at 32: past that its objects go to dictionary mode rather than
 * make every miss walk a long list. A class brand (`@@private:<scope>:%brand`) is the exception,
 * because it is the first property of every instance of every class, so every class hangs its
 * branch off the root shape -- and a realm whose bootstrap already defines a few dozen classes
 * had filled the root before the program's first `new`, which left every instance of every later
 * class a dictionary from its first field on. A program has as many brands as it has classes,
 * which bounds the list; the ceiling below only keeps it finite. `unbounded` is the runtime's own
 * fixed layouts, which must not depend on how many branches a program grew first. */
static int kiln_c99_shape_edge_room(uint32_t edges, const kiln_string *ikey, int unbounded) {
    if (unbounded) return 1;
    return !(edges >= 32 && (edges >= 1024 || !kiln_c99_is_class_brand_key(ikey)));
}

static kiln_shape *kiln_c99_shape_child(kiln_shape *current, const kiln_string *ikey,
                                        uint32_t expected_bit) {
    kiln_shape *next_shape = (kiln_shape *)calloc(1, sizeof(kiln_shape));
    if (next_shape == NULL) return NULL;
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
    return next_shape;
}

/* Add `next` to `current`'s own edges. The child is complete before either store that makes it
 * reachable, and the count is stored last, so a reader that loads the count with acquire sees
 * every edge it counts. A shared parent is only ever linked under the process lock. */
static int kiln_c99_shape_link(kiln_shape *current, const kiln_string *ikey, kiln_shape *next) {
    uint32_t count = current->num_transitions;
    if (count < 2) {
        current->inline_trans_next[count] = next;
        current->inline_trans_ikey[count] = ikey;
    } else {
        kiln_shape_transition *new_tr =
            (kiln_shape_transition *)calloc(1, sizeof(kiln_shape_transition));
        if (new_tr == NULL) return 0;
        new_tr->ikey = ikey;
        new_tr->next = next;
        new_tr->next_transition = current->transitions;
        __atomic_store_n(&current->transitions, new_tr, __ATOMIC_RELEASE);
    }
    __atomic_store_n(&current->num_transitions, count + 1, __ATOMIC_RELEASE);
    return 1;
}

static int kiln_c99_shape_add(kiln_shape *current, const kiln_string *ikey, int is_double,
                              int unbounded, kiln_shape **out_shape) {
    uint32_t bit = 1u << current->property_count;
    uint32_t expected_bit = is_double ? bit : 0;
    int key_shared = kiln_c99_key_is_shared(ikey);
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    kiln_shape *next_shape;
    uint32_t edges;
    if (current->shared && key_shared) {
        kiln_shared_state *process = kiln_c99_process();
        kiln_c99_process_lock(process);
        /* Another thread may have added it since the unlocked look. */
        next_shape = kiln_c99_shape_inline_edge(current, current->num_transitions, ikey, bit,
                                                expected_bit);
        if (next_shape == NULL)
            next_shape = kiln_c99_shape_list_edge(current, ikey, bit, expected_bit);
        if (next_shape == NULL) {
            edges = current->num_transitions +
                    (heap != NULL ? kiln_c99_local_edge_count(heap, current) : 0);
            if (kiln_c99_shape_edge_room(edges, ikey, unbounded) &&
                (next_shape = kiln_c99_shape_child(current, ikey, expected_bit)) != NULL) {
                next_shape->shared = 1;
                if (!kiln_c99_shape_link(current, ikey, next_shape)) {
                    free(next_shape);
                    next_shape = NULL;
                }
            }
        }
        kiln_c99_process_unlock(process);
        if (next_shape == NULL) return 0;
        *out_shape = next_shape;
        return 1;
    }
    if (heap == NULL) return 0;
    edges = __atomic_load_n(&current->num_transitions, __ATOMIC_ACQUIRE);
    if (current->shared) edges += kiln_c99_local_edge_count(heap, current);
    if (!kiln_c99_shape_edge_room(edges, ikey, unbounded)) return 0;
    next_shape = kiln_c99_shape_child(current, ikey, expected_bit);
    if (next_shape == NULL) return 0;
    if (current->shared ? !kiln_c99_local_edge_insert(heap, current, ikey, next_shape)
                        : !kiln_c99_shape_link(current, ikey, next_shape)) {
        free(next_shape);
        return 0;
    }
    /* A realm-local key is pinned: shapes are memory the collector never traces, and this one
     * names the key for as long as the heap lives. */
    if (!key_shared) kiln_c99_pin_interned_key(ikey);
    next_shape->owned_next = heap->local_shapes;
    heap->local_shapes = next_shape;
    *out_shape = next_shape;
    return 1;
}

static int kiln_c99_shape_transition_ex(kiln_shape *current, const kiln_string *ikey,
                                        int is_double, int unbounded, kiln_shape **out_shape) {
    uint32_t bit;
    uint32_t expected_bit;
    kiln_shape *next_shape;
    if (current == NULL || ikey == NULL || current->property_count >= KILN_SHAPE_MAX_FAST_PROPERTIES) {
        return 0;
    }
    bit = 1u << current->property_count;
    expected_bit = is_double ? bit : 0;
    next_shape = kiln_c99_shape_inline_edge(
        current, __atomic_load_n(&current->num_transitions, __ATOMIC_ACQUIRE), ikey, bit,
        expected_bit);
    if (next_shape == NULL && current->shared) {
        const kiln_gc_heap *heap = kiln_c99_gc_heap();
        if (heap != NULL && heap->local_edge_count != 0) {
            const kiln_c99_local_edge *row =
                kiln_c99_local_edge_row(heap, current, ikey, bit, expected_bit);
            if (row != NULL) next_shape = row->child;
        }
    }
    if (next_shape == NULL) next_shape = kiln_c99_shape_list_edge(current, ikey, bit, expected_bit);
    if (next_shape != NULL) {
        *out_shape = next_shape;
        return 1;
    }
    return kiln_c99_shape_add(current, ikey, is_double, unbounded, out_shape);
}

static int kiln_c99_shape_transition(kiln_shape *current, const kiln_string *ikey, int is_double,
                                     kiln_shape **out_shape) {
    return kiln_c99_shape_transition_ex(current, ikey, is_double, 0, out_shape);
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
    object->shape = kiln_c99_shape_root();
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
    object->shape = kiln_c99_shape_root();
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
