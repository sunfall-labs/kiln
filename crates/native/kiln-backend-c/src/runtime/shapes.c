
/* An accessor property of a shaped object keeps its two halves in its one slot, as a pair: a
 * two-element array, getter then setter, that no program ever sees. Every path that reads a slot
 * it found by key checks the shape's `accessor_mask` first, and an inline cache only learns a
 * slot whose attributes say it holds a value, so the pair is only ever read through here. It is
 * an ordinary collected block, reached from the object's slot like any other value. */
static int kiln_c99_accessor_pair_new(kiln_value getter, kiln_value setter, kiln_val *out) {
    kiln_value pair;
    kiln_array *array;
    int status = kiln_c99_new_array(&pair);
    if (status != KILN_C99_OK) return status;
    array = kiln_c99_array_of(pair);
    array->is_numeric = 0;
    status = kiln_c99_array_reserve(array, 2);
    if (status != KILN_C99_OK) return status;
    array->elements[0] = kiln_val_from_boundary(getter);
    array->elements[1] = kiln_val_from_boundary(setter);
    array->count = 2;
    *out = kiln_val_from_boundary(pair);
    return KILN_C99_OK;
}

/* One half of a pair (0 the getter, 1 the setter); undefined for a half never defined. */
static kiln_value kiln_c99_accessor_pair_half(kiln_val pair, int which) {
    kiln_value boxed = kiln_val_to_boundary(pair);
    const kiln_array *array;
    if (boxed.tag != KILN_VALUE_ARRAY) return kiln_c99_undefined();
    array = kiln_c99_array_of(boxed);
    if (array->count != 2) return kiln_c99_undefined();
    return kiln_val_to_boundary(array->elements[which]);
}

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

/* Whether the property in `slot` is enumerable, exactly as the list-mode record
 * `kiln_c99_ensure_dictionary` would build for it says. */
static inline int kiln_c99_shape_slot_enumerable(const kiln_shape *shape, uint32_t slot) {
    return !kiln_c99_key_is_reserved(shape->keys[slot]) ^
           ((shape->attrs[slot] & KILN_C99_ATTR_ENUM_FLIP) != 0);
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

/* ---- Transitions (ADR 0050, shared per ADR 0071) --------------------------------------------
 *
 * A shape's own edges (two inline, the rest a list) name children of the same kind as the edge
 * warrants: a shared shape's own edges are by shared keys to shared children only, published with
 * release stores under the process lock because any realm's thread may add one while another
 * reads; a realm-local shape's own edges lead to realm-local children. A realm-local edge out of
 * a shared shape -- a key of the realm's own, or a child the realm owns -- lives in the realm
 * heap's side table instead, so no realm ever follows an edge into another realm's memory. */
/* Whether `child`, an edge of a shape with `count` properties, is the transition wanted: the
 * double bit and the attributes of the slot it adds are part of the edge's key, as the key is. */
static inline int kiln_c99_shape_edge_is(const kiln_shape *child, uint32_t count, uint32_t bit,
                                         uint32_t expected_bit, uint32_t attrs) {
    /* A slot's special bit is set exactly when its attributes are not zero, which answers the
     * common case -- an assignment's edge -- from the word beside the double mask. */
    return (child->double_slots_mask & bit) == expected_bit &&
           (attrs == 0 ? (child->special_mask & bit) == 0 : child->attrs[count] == attrs);
}

static kiln_shape *kiln_c99_shape_inline_edge(const kiln_shape *current, uint32_t count,
                                              const kiln_string *ikey, uint32_t bit,
                                              uint32_t expected_bit, uint32_t attrs) {
    uint32_t i;
    for (i = 0; i < 2 && i < count; ++i) {
        if (current->inline_trans_ikey[i] == ikey) {
            kiln_shape *cand = current->inline_trans_next[i];
            if (kiln_c99_shape_edge_is(cand, current->property_count, bit, expected_bit, attrs))
                return cand;
        }
    }
    return NULL;
}

static kiln_shape *kiln_c99_shape_list_edge(const kiln_shape *current, const kiln_string *ikey,
                                            uint32_t bit, uint32_t expected_bit, uint32_t attrs) {
    const kiln_shape_transition *tr;
    for (tr = __atomic_load_n(&current->transitions, __ATOMIC_ACQUIRE); tr != NULL;
         tr = tr->next_transition) {
        if (tr->ikey == ikey) {
            kiln_shape *cand = tr->next;
            if (kiln_c99_shape_edge_is(cand, current->property_count, bit, expected_bit, attrs))
                return cand;
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

/* The side-table row for `parent` and `key` whose child agrees on the double bit and the
 * attributes, or with a NULL `key` the parent's count row. */
static kiln_c99_local_edge *kiln_c99_local_edge_row(const kiln_gc_heap *heap,
                                                    const kiln_shape *parent,
                                                    const kiln_string *key, uint32_t bit,
                                                    uint32_t expected_bit, uint32_t attrs) {
    size_t mask;
    size_t at;
    if (heap->local_edge_count == 0) return NULL;
    mask = heap->local_edge_capacity - 1;
    at = kiln_c99_local_edge_home(parent, key, mask);
    for (;;) {
        kiln_c99_local_edge *row = &heap->local_edges[at];
        if (row->parent == NULL) return NULL;
        if (row->parent == parent && row->key == key &&
            (key == NULL || kiln_c99_shape_edge_is(row->child, parent->property_count, bit,
                                                   expected_bit, attrs)))
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
    const kiln_c99_local_edge *row = kiln_c99_local_edge_row(heap, parent, NULL, 0, 0, 0);
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
    count_row = kiln_c99_local_edge_row(heap, parent, NULL, 0, 0, 0);
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

/* How many branches one shape may grow -- its shared edges plus this realm's local ones -- before an
 * object that would take a new one goes to dictionary mode instead, and how many shapes this
 * prelude may mint in all. With the edge index below, a branch costs the same to find among a
 * thousand as among eight, so these bound memory rather than lookup time: a program that keys
 * objects by unbounded data (`{[id]: x}` per request) stops minting shapes here.
 *
 * This was 32 per shape (class brands excepted), and the root shape was full before a program ran:
 * the bootstrap takes about thirty first keys (`constructor`, `prototype`, `value`, `name`, `log`,
 * ...), so every object literal whose first key was not one of those -- `{ a: 1 }`, `{ headers }`,
 * `{ status }` -- became a dictionary at its first member, which disabled the literal site's shape
 * cache and every shape-guarded read cache for exactly the objects handler code builds most.
 * `unbounded` is the runtime's own fixed layouts, which must not depend on what a program grew. */
#define KILN_SHAPE_MAX_BRANCHES 1024u
#define KILN_SHAPE_MAX_TOTAL 65536u
/* Past this many own edges a shape indexes them by key pointer (`edge_table`). */
#define KILN_SHAPE_TABLE_FROM 8u

static int kiln_c99_shape_edge_room(uint32_t edges, const kiln_string *ikey, int unbounded) {
    (void)ikey;
    if (unbounded) return 1;
    return edges < KILN_SHAPE_MAX_BRANCHES &&
           __atomic_load_n(&kiln_c99_shape_total, __ATOMIC_RELAXED) < KILN_SHAPE_MAX_TOTAL;
}

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

/* An index over `current`'s own edges (the inline two and the list), kept complete by
 * `kiln_c99_shape_link`: a probe that reaches an empty slot has seen every own edge under the key.
 * Two edges can share a key -- one whose new slot holds a double and one whose does not -- which is
 * why the probe checks the mask and keeps going. The side table of a shared parent's realm-local
 * edges is separate and already hashed. */
static kiln_shape *kiln_c99_shape_table_edge(const kiln_shape_edge_table *table,
                                             const kiln_string *ikey, uint32_t bit,
                                             uint32_t expected_bit, uint32_t attrs) {
    uint32_t at = kiln_c99_shape_edge_hash(ikey) & table->mask;
    kiln_shape *seen;
    while ((seen = __atomic_load_n(&table->edges[at], __ATOMIC_ACQUIRE)) != NULL) {
        /* The slot a child adds is its parent's count, which is `bit`'s index. */
        if (seen->ikey == ikey && kiln_c99_shape_edge_is(seen, seen->slot_index, bit, expected_bit, attrs))
            return seen;
        at = (at + 1) & table->mask;
    }
    return NULL;
}

/* Record `next` in `shape`'s index once it has more than `KILN_SHAPE_TABLE_FROM` own edges. Called
 * by the one writer `kiln_c99_shape_link` allows (the process lock for a shared shape, the realm's
 * thread for a local one), before the count store that publishes the edge. Returns 0 only when a
 * table could not be allocated, and the caller then refuses the link, so index and list agree.
 *
 * ADR 0071's publishing rule: a grown table is built completely and published with one release
 * store; a slot of a live table is claimed with a compare-and-swap. A shared shape's replaced table
 * is never freed (a reader on another thread may still be probing it; each is at most half the
 * next, so what is kept is bounded by the live one) -- reclaiming it at a turn boundary is step 4's.
 * A realm-local shape's is freed at once: only its realm's thread reads it. */
static int kiln_c99_shape_index_edge(kiln_shape *shape, uint32_t count, kiln_shape *next) {
    uint32_t need = count + 1;
    kiln_shape_edge_table *table;
    if (need <= KILN_SHAPE_TABLE_FROM) return 1;
    table = __atomic_load_n(&shape->edge_table, __ATOMIC_ACQUIRE);
    if (table == NULL || need * 2 > table->mask + 1) {
        uint32_t cap = table == NULL ? 32u : (table->mask + 1) * 2;
        kiln_shape_edge_table *grown;
        kiln_shape_transition *tr;
        uint32_t i;
        while (need * 2 > cap) cap *= 2;
        grown = (kiln_shape_edge_table *)calloc(1, sizeof *grown + cap * sizeof(kiln_shape *));
        if (grown == NULL) return 0;
        grown->mask = cap - 1;
        for (i = 0; i < 2 && i < count; ++i) kiln_c99_shape_edge_claim(grown, shape->inline_trans_next[i]);
        for (tr = shape->transitions; tr != NULL; tr = tr->next_transition)
            kiln_c99_shape_edge_claim(grown, tr->next);
        kiln_c99_shape_edge_claim(grown, next);
        __atomic_store_n(&shape->edge_table, grown, __ATOMIC_RELEASE);
        if (!shape->shared) free(table);
        return 1;
    }
    kiln_c99_shape_edge_claim(table, next);
    return 1;
}

static kiln_shape *kiln_c99_shape_child(kiln_shape *current, const kiln_string *ikey,
                                        uint32_t expected_bit, uint32_t attrs) {
    uint32_t bit = 1u << current->property_count;
    kiln_shape *next_shape = (kiln_shape *)calloc(1, sizeof(kiln_shape));
    if (next_shape == NULL) return NULL;
    __atomic_fetch_add(&kiln_c99_shape_total, 1u, __ATOMIC_RELAXED);
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
    memcpy(next_shape->attrs, current->attrs, current->property_count);
    next_shape->attrs[current->property_count] = (uint8_t)attrs;
    next_shape->special_mask = current->special_mask | (attrs != 0 ? bit : 0);
    next_shape->accessor_mask =
        current->accessor_mask | ((attrs & KILN_C99_ATTR_ACCESSOR) != 0 ? bit : 0);
    next_shape->store_mask =
        current->store_mask |
        ((attrs & (KILN_C99_ATTR_READONLY | KILN_C99_ATTR_ACCESSOR)) != 0 ? bit : 0);
    return next_shape;
}

/* Add `next` to `current`'s own edges. The child is complete before either store that makes it
 * reachable, and the count is stored last, so a reader that loads the count with acquire sees
 * every edge it counts. A shared parent is only ever linked under the process lock. */
static int kiln_c99_shape_link(kiln_shape *current, const kiln_string *ikey, kiln_shape *next) {
    uint32_t count = current->num_transitions;
    if (!kiln_c99_shape_index_edge(current, count, next)) return 0;
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
                              uint32_t attrs, int unbounded, kiln_shape **out_shape) {
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
                                                expected_bit, attrs);
        if (next_shape == NULL)
            next_shape = kiln_c99_shape_list_edge(current, ikey, bit, expected_bit, attrs);
        if (next_shape == NULL) {
            edges = current->num_transitions +
                    (heap != NULL ? kiln_c99_local_edge_count(heap, current) : 0);
            if (kiln_c99_shape_edge_room(edges, ikey, unbounded) &&
                (next_shape = kiln_c99_shape_child(current, ikey, expected_bit, attrs)) != NULL) {
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
    next_shape = kiln_c99_shape_child(current, ikey, expected_bit, attrs);
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
                                        int is_double, uint32_t attrs, int unbounded,
                                        kiln_shape **out_shape) {
    uint32_t bit;
    uint32_t expected_bit;
    kiln_shape *next_shape;
    if (current == NULL || ikey == NULL || current->property_count >= KILN_SHAPE_MAX_FAST_PROPERTIES) {
        return 0;
    }
    bit = 1u << current->property_count;
    expected_bit = is_double ? bit : 0;
    {
        const kiln_shape_edge_table *table = __atomic_load_n(&current->edge_table, __ATOMIC_ACQUIRE);
        if (table != NULL) {
            /* The index covers every own edge it was published with; one it has not seen yet is
             * found again by `kiln_c99_shape_add`'s rescan under the lock. */
            next_shape = kiln_c99_shape_table_edge(table, ikey, bit, expected_bit, attrs);
            if (next_shape != NULL) {
                *out_shape = next_shape;
                return 1;
            }
        } else {
            next_shape = kiln_c99_shape_inline_edge(
                current, __atomic_load_n(&current->num_transitions, __ATOMIC_ACQUIRE), ikey, bit,
                expected_bit, attrs);
        }
    }
    if (next_shape == NULL && current->shared) {
        const kiln_gc_heap *heap = kiln_c99_gc_heap();
        if (heap != NULL && heap->local_edge_count != 0) {
            const kiln_c99_local_edge *row =
                kiln_c99_local_edge_row(heap, current, ikey, bit, expected_bit, attrs);
            if (row != NULL) next_shape = row->child;
        }
    }
    if (next_shape == NULL && current->edge_table == NULL)
        next_shape = kiln_c99_shape_list_edge(current, ikey, bit, expected_bit, attrs);
    if (next_shape != NULL) {
        *out_shape = next_shape;
        return 1;
    }
    return kiln_c99_shape_add(current, ikey, is_double, attrs, unbounded, out_shape);
}

static int kiln_c99_shape_transition(kiln_shape *current, const kiln_string *ikey, int is_double,
                                     kiln_shape **out_shape) {
    return kiln_c99_shape_transition_ex(current, ikey, is_double, 0, 0, out_shape);
}

/* Issue 04 step 3b: a definition that keeps its object shaped.
 *
 * Adds `ikey`, which the object does not have, as a new own property whose attributes are
 * `attrs` (`KILN_C99_ATTR_*`) and whose slot holds `held` -- a value, or for an accessor its pair
 * -- by the transition keyed by both. This is how a class prototype's `constructor`, methods and
 * accessors, a private method, and a runtime object's non-enumerable members are defined without
 * leaving shape mode. Returns 1 when done (`*status` says whether it succeeded), 0 when the object
 * cannot stay shaped this way -- an index-like key, the fast-property limit, a branch that is
 * full -- and the caller's list-mode path, unchanged, must do the definition. The counters
 * `kiln_c99_put_mode_key`'s chain walk skips by are kept as the list would keep them. */
static int kiln_c99_shape_define_new(kiln_object *object, const kiln_string *ikey, kiln_val held,
                                     uint32_t attrs, int *status) {
    kiln_shape *next;
    uint32_t slot;
    if (object->shape == NULL || object->properties != NULL || ikey == NULL ||
        kiln_c99_key_index_key(ikey) >= 0.0) {
        return 0;
    }
    if (!kiln_c99_shape_transition_ex(object->shape, ikey,
                                      attrs == 0 && KILN_VAL_IS_NUMBER(held), attrs, 0, &next)) {
        return 0;
    }
    slot = next->slot_index;
    if (slot >= object->slot_capacity) {
        size_t new_cap =
            object->slot_capacity == 0 ? KILN_C99_INLINE_SLOTS : object->slot_capacity * 2;
        kiln_val *new_slots;
        size_t fill;
        while (slot >= new_cap) new_cap *= 2;
        if (object->slots == object->inline_slots) {
            new_slots = (kiln_val *)malloc(new_cap * sizeof(kiln_val));
            if (new_slots != NULL)
                memcpy(new_slots, object->inline_slots, KILN_C99_INLINE_SLOTS * sizeof(kiln_val));
        } else if (object->slots == NULL) {
            new_slots = (kiln_val *)malloc(new_cap * sizeof(kiln_val));
        } else {
            new_slots = (kiln_val *)realloc(object->slots, new_cap * sizeof(kiln_val));
        }
        if (new_slots == NULL) {
            *status = KILN_C99_OUT_OF_MEMORY;
            return 1;
        }
        for (fill = object->slot_capacity; fill < new_cap; ++fill) new_slots[fill] = KILN_VAL_UNDEFINED;
        object->slots = new_slots;
        object->slot_capacity = new_cap;
    }
    kiln_c99_prototype_touched(object);
    object->slots[slot] = held;
    object->shape = next;
    if (attrs & KILN_C99_ATTR_ACCESSOR) {
        object->accessor_count += 1;
    } else if (attrs & KILN_C99_ATTR_READONLY) {
        object->read_only_seen = 1;
    }
    *status = KILN_C99_OK;
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
        uint32_t attrs = shape->attrs[i];
        kiln_val held = (object->slots != NULL && i < object->slot_capacity)
                            ? object->slots[i]
                            : KILN_VAL_UNDEFINED;
        /* The record a list-mode object would carry for the same history: see
         * `KILN_C99_ATTR_*`, which are its differences from an assignment's record. */
        object->properties[pos].key = key;
        object->properties[pos].key_len = key_len;
        object->properties[pos].enumerable =
            !(key_len >= 2 && kiln_string_unit_at(key, 0) == '@' &&
              kiln_string_unit_at(key, 1) == '@') ^
            ((attrs & KILN_C99_ATTR_ENUM_FLIP) != 0);
        object->properties[pos].writable = (attrs & KILN_C99_ATTR_READONLY) == 0;
        object->properties[pos].configurable = (attrs & KILN_C99_ATTR_FIXED) == 0;
        object->properties[pos].symbol_hidden = 0;
        object->properties[pos].accessor = (attrs & KILN_C99_ATTR_ACCESSOR) != 0;
        object->properties[pos].refused = 0;
        if (attrs & KILN_C99_ATTR_ACCESSOR) {
            object->properties[pos].value = kiln_c99_undefined();
            object->properties[pos].getter = kiln_c99_accessor_pair_half(held, 0);
            object->properties[pos].setter = kiln_c99_accessor_pair_half(held, 1);
        } else {
            object->properties[pos].value = kiln_val_to_boundary(held);
            object->properties[pos].getter = kiln_c99_undefined();
            object->properties[pos].setter = kiln_c99_undefined();
        }
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
    object->layout_token = 0;
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
    object->layout_token = 0;
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
