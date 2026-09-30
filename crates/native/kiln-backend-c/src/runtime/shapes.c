
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
    kiln_c99_heap_store_val(array, &array->elements[0], kiln_val_from_boundary(getter));
    kiln_c99_heap_store_val(array, &array->elements[1], kiln_val_from_boundary(setter));
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

/* Whether the property in `slot` is enumerable as the walks over own *string* keys see it --
 * the stored bit of the list-mode record `kiln_c99_ensure_dictionary` would build for it, which
 * is 0 for every reserved (`@@`) key. A symbol key's program-visible answer is `ENUM_FLIP`'s
 * absence (`KILN_C99_ATTR_ENUM_FLIP`). */
static inline int kiln_c99_shape_slot_enumerable(const kiln_shape *shape, uint32_t slot) {
    return !kiln_c99_key_is_reserved(shape->keys[slot]) &&
           (shape->attrs[slot] & KILN_C99_ATTR_ENUM_FLIP) == 0;
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
                                         uint32_t expected_bit, uint32_t attrs,
                                         kiln_body_fn code) {
    /* A slot's special bit is set exactly when its attributes are not zero, which answers the
     * common case -- an assignment's edge -- from the word beside the double mask. Only an
     * attribute-bearing slot carries a code record (step 3c), and the child's `code` is the
     * record of the slot it adds. */
    return (child->double_slots_mask & bit) == expected_bit &&
           (attrs == 0 ? (child->special_mask & bit) == 0
                       : child->attrs[count] == attrs && child->code == code);
}

static kiln_shape *kiln_c99_shape_inline_edge(const kiln_shape *current, uint32_t count,
                                              const kiln_string *ikey, uint32_t bit,
                                              uint32_t expected_bit, uint32_t attrs,
                                              kiln_body_fn code) {
    uint32_t i;
    for (i = 0; i < 2 && i < count; ++i) {
        if (current->inline_trans_ikey[i] == ikey) {
            kiln_shape *cand = current->inline_trans_next[i];
            if (kiln_c99_shape_edge_is(cand, current->property_count, bit, expected_bit, attrs,
                                       code))
                return cand;
        }
    }
    return NULL;
}

static kiln_shape *kiln_c99_shape_list_edge(const kiln_shape *current, const kiln_string *ikey,
                                            uint32_t bit, uint32_t expected_bit, uint32_t attrs,
                                            kiln_body_fn code) {
    const kiln_shape_transition *tr;
    for (tr = __atomic_load_n(&current->transitions, __ATOMIC_ACQUIRE); tr != NULL;
         tr = tr->next_transition) {
        if (tr->ikey == ikey) {
            kiln_shape *cand = tr->next;
            if (kiln_c99_shape_edge_is(cand, current->property_count, bit, expected_bit, attrs,
                                       code))
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
                                                    uint32_t expected_bit, uint32_t attrs,
                                                    kiln_body_fn code) {
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
                                                   expected_bit, attrs, code)))
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
    const kiln_c99_local_edge *row = kiln_c99_local_edge_row(heap, parent, NULL, 0, 0, 0, NULL);
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
    count_row = kiln_c99_local_edge_row(heap, parent, NULL, 0, 0, 0, NULL);
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
 * object that would take a new one goes to dictionary mode instead, and how many shapes may be
 * minted in all: shared ones by the process, and each realm's own by that realm. With the edge index below, a branch costs the same to find among a
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

static int kiln_c99_shape_edge_room(uint32_t edges, uint32_t total, int unbounded) {
    if (unbounded) return 1;
    return edges < KILN_SHAPE_MAX_BRANCHES && total < KILN_SHAPE_MAX_TOTAL;
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
                                             uint32_t expected_bit, uint32_t attrs,
                                             kiln_body_fn code) {
    uint32_t at = kiln_c99_shape_edge_hash(ikey) & table->mask;
    kiln_shape *seen;
    while ((seen = __atomic_load_n(&table->edges[at], __ATOMIC_ACQUIRE)) != NULL) {
        /* The slot a child adds is its parent's count, which is `bit`'s index. */
        if (seen->ikey == ikey && kiln_c99_shape_edge_is(seen, seen->slot_index, bit, expected_bit, attrs,
                                                               code))
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
 * ADR 0071's publishing rule: a grown table is built completely and published with one store; a
 * slot of a live table is claimed with a compare-and-swap. A shared shape's replaced table is
 * retired (`kiln_realm_retire`): a reader on another thread may still be probing it, and it is
 * freed once every such thread has passed a turn boundary. A realm-local shape's is freed at once:
 * only its realm's thread reads it. */
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
        /* Sequentially consistent, as every unpublish is (see `kiln_c99_slot_claim`). */
        __atomic_store_n(&shape->edge_table, grown, __ATOMIC_SEQ_CST);
        if (!shape->shared) free(table);
        else if (table != NULL) (void)kiln_realm_retire(table);
        return 1;
    }
    kiln_c99_shape_edge_claim(table, next);
    return 1;
}

static kiln_shape *kiln_c99_shape_child(kiln_shape *current, const kiln_string *ikey,
                                        uint32_t expected_bit, uint32_t attrs, kiln_body_fn code) {
    uint32_t bit = 1u << current->property_count;
    /* Inductive native-metadata authority: parents contain only canonical keys; every new
     * edge comes from the same key table, whose allocations are outside candidate ownership. */
    if (ikey == NULL || kiln_c99_interned_key_in(kiln_c99_gc_heap(), ikey) != ikey) return NULL;
    kiln_shape *next_shape = (kiln_shape *)calloc(1, sizeof(kiln_shape));
    if (next_shape == NULL) return NULL;
    next_shape->parent = current;
    next_shape->ikey = ikey;
    next_shape->has_symbol_key = current->has_symbol_key || kiln_c99_key_is_reserved(ikey);
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
        ((attrs & (KILN_C99_ATTR_READONLY | KILN_C99_ATTR_ACCESSOR)) != 0 || code != NULL ? bit : 0);
    next_shape->code = code;
    next_shape->code_mask = current->code_mask | (code != NULL ? bit : 0);
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
                              uint32_t attrs, kiln_body_fn code, int unbounded,
                              kiln_shape **out_shape) {
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
                                                expected_bit, attrs, code);
        if (next_shape == NULL)
            next_shape = kiln_c99_shape_list_edge(current, ikey, bit, expected_bit, attrs, code);
        if (next_shape == NULL) {
            edges = current->num_transitions +
                    (heap != NULL ? kiln_c99_local_edge_count(heap, current) : 0);
            if (kiln_c99_shape_edge_room(edges, process->shared_shapes, unbounded) &&
                (next_shape = kiln_c99_shape_child(current, ikey, expected_bit, attrs, code)) != NULL) {
                next_shape->shared = 1;
                if (!kiln_c99_shape_link(current, ikey, next_shape)) {
                    free(next_shape);
                    next_shape = NULL;
                } else {
                    process->shared_shapes += 1;
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
    if (!kiln_c99_shape_edge_room(edges, heap->local_shape_count, unbounded)) return 0;
    next_shape = kiln_c99_shape_child(current, ikey, expected_bit, attrs, code);
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
    heap->local_shape_count += 1;
    *out_shape = next_shape;
    return 1;
}

/* The edge out of `current` a definition of `ikey` takes -- its own edges, then this realm's
 * side table -- or NULL when there is none yet. Only reads: every edge it can find was published
 * complete (see `kiln_c99_shape_link`), so a pool thread may ask too (`kiln_c99_par_shape_step`). */
static kiln_shape *kiln_c99_shape_edge_find(kiln_shape *current, const kiln_string *ikey,
                                            uint32_t bit, uint32_t expected_bit, uint32_t attrs,
                                            kiln_body_fn code) {
    kiln_shape *next_shape;
    {
        const kiln_shape_edge_table *table = __atomic_load_n(&current->edge_table, __ATOMIC_ACQUIRE);
        if (table != NULL) {
            /* The index covers every own edge it was published with; one it has not seen yet is
             * found again by `kiln_c99_shape_add`'s rescan under the lock. */
            next_shape = kiln_c99_shape_table_edge(table, ikey, bit, expected_bit, attrs, code);
            if (next_shape != NULL) return next_shape;
        } else {
            next_shape = kiln_c99_shape_inline_edge(
                current, __atomic_load_n(&current->num_transitions, __ATOMIC_ACQUIRE), ikey, bit,
                expected_bit, attrs, code);
        }
    }
    if (next_shape == NULL && current->shared) {
        const kiln_gc_heap *heap = kiln_c99_gc_heap();
        if (heap != NULL && heap->local_edge_count != 0) {
            const kiln_c99_local_edge *row =
                kiln_c99_local_edge_row(heap, current, ikey, bit, expected_bit, attrs, code);
            if (row != NULL) next_shape = row->child;
        }
    }
    if (next_shape == NULL && current->edge_table == NULL)
        next_shape = kiln_c99_shape_list_edge(current, ikey, bit, expected_bit, attrs, code);
    return next_shape;
}

static int kiln_c99_shape_transition_code(kiln_shape *current, const kiln_string *ikey,
                                          int is_double, uint32_t attrs, kiln_body_fn code,
                                          int unbounded, kiln_shape **out_shape) {
    uint32_t bit;
    uint32_t expected_bit;
    kiln_shape *next_shape;
    if (current == NULL || ikey == NULL || (current->shared & KILN_C99_SHAPE_FINAL) != 0 ||
        current->property_count >= KILN_SHAPE_MAX_FAST_PROPERTIES) {
        return 0;
    }
    bit = 1u << current->property_count;
    expected_bit = is_double ? bit : 0;
    next_shape = kiln_c99_shape_edge_find(current, ikey, bit, expected_bit, attrs, code);
    if (next_shape != NULL) {
        *out_shape = next_shape;
        return 1;
    }
    return kiln_c99_shape_add(current, ikey, is_double, attrs, code, unbounded, out_shape);
}

/* Tier 3 of the parallel built-ins (parallel issue 08): the transition a definition takes on a
 * pool thread, while the realm thread is blocked in the call. The edge is found as the realm
 * thread finds it. A missing edge between shared shapes by a shared key is made the way any
 * realm's thread makes one -- under the process lock, published complete (ADR 0071) -- because
 * that is exactly the concurrency the shared tree was built for. A missing realm-local edge (a key
 * of the realm's own, or a parent the realm owns) is not: the side table and the realm's shape
 * list are the realm thread's alone, so the worker declines, the call runs sequentially and makes
 * the edge, and every later call finds it here. Either way the object ends with the shape the
 * sequential definition gives it. */
static int kiln_c99_par_shape_step(kiln_shape *current, const kiln_string *ikey, int is_double,
                                   kiln_shape **out_shape) {
    uint32_t bit;
    uint32_t expected_bit;
    kiln_shape *next_shape;
    if (current == NULL || ikey == NULL || (current->shared & KILN_C99_SHAPE_FINAL) != 0 ||
        current->property_count >= KILN_SHAPE_MAX_FAST_PROPERTIES) {
        return 0;
    }
    bit = 1u << current->property_count;
    expected_bit = is_double ? bit : 0;
    next_shape = kiln_c99_shape_edge_find(current, ikey, bit, expected_bit, 0, NULL);
    if (next_shape != NULL) {
        *out_shape = next_shape;
        return 1;
    }
    if (!current->shared || !kiln_c99_key_is_shared(ikey)) return 0;
    return kiln_c99_shape_add(current, ikey, is_double, 0, NULL, 0, out_shape);
}

static int kiln_c99_shape_transition_ex(kiln_shape *current, const kiln_string *ikey,
                                        int is_double, uint32_t attrs, int unbounded,
                                        kiln_shape **out_shape) {
    return kiln_c99_shape_transition_code(current, ikey, is_double, attrs, NULL, unbounded,
                                          out_shape);
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
                                     uint32_t attrs, kiln_body_fn code, int *status) {
    kiln_shape *next;
    uint32_t slot;
    if (object->shape == NULL || object->properties != NULL || ikey == NULL ||
        kiln_c99_key_index_key(ikey) >= 0.0) {
        return 0;
    }
    /* A record rides only on an attribute-bearing slot (see `kiln_c99_shape_edge_is`). A branch
     * that has no room for the recorded edge may still have the unrecorded one: the record is an
     * optimisation, the shape is not. */
    if (attrs == 0) code = NULL;
    if (!kiln_c99_shape_transition_code(object->shape, ikey,
                                        attrs == 0 && KILN_VAL_IS_NUMBER(held), attrs, code, 0,
                                        &next) &&
        (code == NULL ||
         !kiln_c99_shape_transition_code(object->shape, ikey, 0, attrs, NULL, 0, &next))) {
        return 0;
    }
    slot = next->slot_index;
    if (slot >= object->slot_capacity) {
        size_t new_cap =
            object->slot_capacity == 0 ? KILN_C99_INLINE_SLOTS : object->slot_capacity * 2;
        kiln_val *new_slots;
        size_t fill;
        while (slot >= new_cap) new_cap *= 2;
        new_slots = kiln_c99_object_slots_resize(object, new_cap);
        if (new_slots == NULL) {
            *status = KILN_C99_OUT_OF_MEMORY;
            return 1;
        }
        for (fill = object->slot_capacity; fill < new_cap; ++fill) new_slots[fill] = KILN_VAL_UNDEFINED;
        kiln_c99_heap_store_pointer(object, &object->slots, new_slots);
        object->slot_capacity = new_cap;
    }
    kiln_c99_prototype_touched(object);
    kiln_c99_heap_store_val(object, &object->slots[slot], held);
    object->shape = next;
    if (attrs & KILN_C99_ATTR_ACCESSOR) {
        object->accessor_count += 1;
    } else if (attrs & KILN_C99_ATTR_READONLY) {
        object->read_only_seen = 1;
    }
    *status = KILN_C99_OK;
    return 1;
}

/* The code of `value` when it is a closure, else NULL: what a record compares against. */
static inline kiln_body_fn kiln_c99_val_code(kiln_val value) {
    return KILN_VAL_IS_CLOSURE(value) ? ((const kiln_closure *)KILN_VAL_PTR(value))->body : NULL;
}

static int kiln_c99_ensure_dictionary(kiln_object *object);
static uint32_t *kiln_c99_sealed_orders(const kiln_object *object);
static int kiln_c99_sealed_property_after(const kiln_object *object, uint32_t after,
                                          uint32_t *next, kiln_property *out);
static int kiln_c99_sealed_property_nth(const kiln_object *object, size_t at,
                                         kiln_property *out);

/* Store `value` into `object`'s slot `slot`, which its shape records code for (`code_mask`), as
 * an assignment would: in place when `value` is a closure with exactly that code, so the record
 * stays true; otherwise the object first moves to the shape that is its own with this slot's
 * record dropped -- the same keys, attributes, double bits and every other record, rebuilt from
 * the slot's parent by ordinary transitions -- so no object of a recording shape ever holds
 * anything else there. When that shape cannot be had (a full branch) the object leaves shape
 * mode instead and 0 is returned: the caller redoes the store on the list. 1 when stored. */
static int kiln_c99_shape_store_recorded(kiln_object *object, uint32_t slot, kiln_val value) {
    kiln_shape *shape = object->shape;
    const kiln_shape *chain[KILN_SHAPE_MAX_FAST_PROPERTIES];
    uint32_t depth = 0;
    const kiln_shape *walk;
    kiln_shape *rebuilt;
    if (slot >= object->slot_capacity) {
        kiln_c99_ensure_dictionary(object);
        return 0;
    }
    if (kiln_c99_val_code(value) == kiln_c99_shape_slot_code(shape, slot)) {
        kiln_c99_prototype_touched(object);
        kiln_c99_heap_store_val(object, &object->slots[slot], value);
        return 1;
    }
    /* The shapes from the one that added `slot` down to `shape`, deepest last. */
    for (walk = shape; walk != NULL && walk->property_count > slot; walk = walk->parent)
        chain[depth++] = walk;
    rebuilt = walk != NULL ? (kiln_shape *)walk : NULL;
    while (rebuilt != NULL && depth > 0) {
        const kiln_shape *step = chain[--depth];
        uint32_t at = step->slot_index;
        kiln_shape *next;
        if (!kiln_c99_shape_transition_code(rebuilt, step->ikey,
                                            (step->double_slots_mask >> at) & 1u, step->attrs[at],
                                            at == slot ? NULL : step->code, 0, &next)) {
            rebuilt = NULL;
            break;
        }
        rebuilt = next;
    }
    if (rebuilt == NULL) {
        kiln_c99_ensure_dictionary(object);
        return 0;
    }
    kiln_c99_prototype_touched(object);
    object->shape = rebuilt;
    kiln_c99_heap_store_val(object, &object->slots[slot], value);
    return 1;
}

/* Project one shaped slot into the ordinary property record. This is a borrowed read view:
 * values still belong to the realm, while keys and attributes come from the shared shape. */
static void kiln_c99_shape_property(const kiln_object *object, uint32_t slot,
                                    kiln_property *property) {
    const kiln_shape *shape = object->shape;
    const kiln_string *key = shape->keys[slot];
    uint32_t attrs = shape->attrs[slot];
    kiln_val held = object->slots != NULL && slot < object->slot_capacity
                        ? object->slots[slot] : KILN_VAL_UNDEFINED;
    int flip = (attrs & KILN_C99_ATTR_ENUM_FLIP) != 0;
    property->key = key;
    property->enumerable = !kiln_c99_key_is_reserved(key) && !flip;
    property->symbol_hidden = kiln_c99_key_is_symbol(key) && flip;
    property->writable = (attrs & KILN_C99_ATTR_READONLY) == 0;
    property->configurable = (attrs & KILN_C99_ATTR_FIXED) == 0;
    property->accessor = (attrs & KILN_C99_ATTR_ACCESSOR) != 0;
    property->refused = 0;
    if (property->accessor) {
        kiln_c99_heap_store_value(object, &property->getter, kiln_c99_accessor_pair_half(held, 0));
        kiln_c99_heap_store_value(object, &property->setter, kiln_c99_accessor_pair_half(held, 1));
    } else {
        kiln_c99_heap_store_value(object, &property->value, kiln_val_to_boundary(held));
    }
}

/* Candidate provenance is attached only when the object and its side buffer were both made
 * inside the active factory. A pre-existing dictionary keeps its ordinary malloc layout. */
static uint64_t kiln_c99_object_candidate_id(const kiln_object *object) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    kiln_gc_block *block;
    kiln_nursery_chunk *chunk;
    if (heap == NULL || heap->actor_candidate_id == 0) return 0;
    block = kiln_c99_gc_lookup(heap, object);
    if (block == NULL || (block->kind & KILN_GC_FLAG_NURSERY) == 0) return 0;
    chunk = kiln_c99_nursery_chunk_of(heap, block);
    return chunk != NULL && chunk->candidate_id == heap->actor_candidate_id
               ? heap->actor_candidate_id : 0;
}

/* Numeric keys use a separate dense vector. Candidate-created objects give that vector the
 * same owner/generation certificate as shaped slots and dictionary records, so the graph
 * mover can account for its malloc allocation before handing it to another heap. */
static kiln_val *kiln_c99_object_elements_resize(kiln_object *object, size_t capacity) {
    uint64_t candidate_id;
    kiln_val *elements;
    if (capacity < object->element_capacity || capacity > SIZE_MAX / sizeof(kiln_val))
        return NULL;
    candidate_id = object->candidate_elements ? 0 : kiln_c99_object_candidate_id(object);
    if (object->candidate_elements || candidate_id != 0) {
        kiln_c99_candidate_object_elements *header;
        if (capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_val)) return NULL;
        if (object->candidate_elements) {
            header = kiln_c99_object_candidate_elements_header(object);
            if (object->candidate_elements != 1 || header->owner != object ||
                header->capacity != object->element_capacity ||
                header->generation == UINT64_MAX) return NULL;
            header = (kiln_c99_candidate_object_elements *)realloc(
                header, sizeof *header + capacity * sizeof(kiln_val));
            if (header == NULL) return NULL;
            header->generation += 1;
        } else {
            header = (kiln_c99_candidate_object_elements *)malloc(
                sizeof *header + capacity * sizeof(kiln_val));
            if (header == NULL) return NULL;
            header->candidate_id = candidate_id;
            header->owner = object;
            header->generation = 1;
            if (object->elements != NULL) {
                kiln_c99_heap_relocate_vals(object, (kiln_val *)(header + 1), object->elements,
                                        object->element_capacity);
                free(object->elements);
            }
            object->candidate_elements = 1;
        }
        header->capacity = capacity;
        return (kiln_val *)(header + 1);
    }
    elements = (kiln_val *)realloc(object->elements, capacity * sizeof(kiln_val));
    return elements;
}

static kiln_val *kiln_c99_object_slots_resize(kiln_object *object, size_t capacity) {
    kiln_val *slots;
    uint64_t candidate_id;
    if (object->sealed != NULL || capacity < object->slot_capacity ||
        capacity > SIZE_MAX / sizeof(kiln_val)) return NULL;
    candidate_id = object->candidate_slots ? 0 : kiln_c99_object_candidate_id(object);
    if (object->candidate_slots ||
        (candidate_id != 0 && !kiln_c99_object_slots_owned(object))) {
        kiln_c99_candidate_object_slots *header;
        if (capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_val)) return NULL;
        if (object->candidate_slots) {
            header = kiln_c99_object_candidate_slots_header(object);
            if (object->candidate_slots != 1 || header->owner != object ||
                header->capacity != object->slot_capacity ||
                header->generation == UINT64_MAX) return NULL;
            header = (kiln_c99_candidate_object_slots *)realloc(
                header, sizeof *header + capacity * sizeof(kiln_val));
            if (header == NULL) return NULL;
            header->generation += 1;
        } else {
            header = (kiln_c99_candidate_object_slots *)malloc(
                sizeof *header + capacity * sizeof(kiln_val));
            if (header == NULL) return NULL;
            header->candidate_id = candidate_id;
            header->owner = object;
            header->generation = 1;
            if (object->slots != NULL)
                kiln_c99_heap_relocate_vals(object, (kiln_val *)(header + 1), object->slots,
                                        object->slot_capacity);
            object->candidate_slots = 1;
        }
        header->capacity = capacity;
        return (kiln_val *)(header + 1);
    }
    if (kiln_c99_object_slots_owned(object))
        return (kiln_val *)realloc(object->slots, capacity * sizeof(kiln_val));
    slots = (kiln_val *)malloc(capacity * sizeof(kiln_val));
    if (slots != NULL && object->slots != NULL)
        kiln_c99_heap_relocate_vals(object, slots, object->slots, object->slot_capacity);
    return slots;
}

static int kiln_c99_object_reserve_properties(kiln_object *object, size_t needed) {
    if (needed <= object->capacity) return KILN_C99_OK;
    if (object->candidate_properties || object->properties == NULL ||
        kiln_c99_object_properties_in_block(object)) {
        uint64_t candidate_id = object->candidate_properties ? 0
                                    : kiln_c99_object_candidate_id(object);
        if (object->candidate_properties || candidate_id != 0) {
            kiln_c99_candidate_object_properties *header;
            kiln_property *properties;
            size_t capacity = object->capacity > needed / 2 &&
                              object->capacity <= SIZE_MAX / 2
                                  ? object->capacity * 2 : needed;
            if (capacity < needed ||
                capacity > (SIZE_MAX - sizeof *header) / sizeof *properties)
                return KILN_C99_OUT_OF_MEMORY;
            if (object->candidate_properties) {
                header = kiln_c99_object_candidate_header(object);
                if (object->candidate_properties != 1 || header->owner != object ||
                    header->capacity != object->capacity ||
                    header->generation == UINT64_MAX) return KILN_C99_OUT_OF_MEMORY;
                header = (kiln_c99_candidate_object_properties *)realloc(
                    header, sizeof *header + capacity * sizeof *properties);
                if (header == NULL) return KILN_C99_OUT_OF_MEMORY;
                header->generation += 1;
            } else {
                header = (kiln_c99_candidate_object_properties *)malloc(
                    sizeof *header + capacity * sizeof *properties);
                if (header == NULL) return KILN_C99_OUT_OF_MEMORY;
                header->candidate_id = candidate_id;
                header->owner = object;
                header->generation = 1;
                if (object->properties != NULL)
                    kiln_c99_heap_copy_properties(object, (kiln_property *)(header + 1),
                                                 object->properties, object->count);
                object->candidate_properties = 1;
            }
            header->capacity = capacity;
            kiln_c99_heap_store_pointer(object, &object->properties, (kiln_property *)(header + 1));
            object->capacity = capacity;
            return KILN_C99_OK;
        }
    }
    if (kiln_c99_object_properties_in_block(object)) {
        kiln_property *properties;
        size_t capacity = object->capacity > needed / 2 && object->capacity <= SIZE_MAX / 2
                              ? object->capacity * 2 : needed;
        if (capacity < needed || capacity > SIZE_MAX / sizeof *properties)
            return KILN_C99_OUT_OF_MEMORY;
        properties = (kiln_property *)malloc(capacity * sizeof *properties);
        if (properties == NULL) return KILN_C99_OUT_OF_MEMORY;
        kiln_c99_heap_copy_properties(object, properties, object->properties, object->count);
        kiln_c99_heap_store_pointer(object, &object->properties, properties);
        object->capacity = capacity;
        return KILN_C99_OK;
    }
    return kiln_c99_reserve((void **)&object->properties, &object->capacity, needed,
                            sizeof *object->properties);
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
    if (object->sealed != NULL) {
        kiln_property *properties;
        kiln_sealed_values *values;
        size_t visible = 0;
        size_t at;
        size_t index;
        uint32_t after = 0;
        uint32_t *orders = kiln_c99_sealed_orders(object);
        for (index = 0; index < object->sealed->count; ++index)
            visible += orders[index] != 0;
        visible += object->count;
        if (visible > SIZE_MAX / sizeof *properties) return KILN_C99_OUT_OF_MEMORY;
        properties = visible == 0 ? NULL : malloc(visible * sizeof *properties);
        if (visible != 0 && properties == NULL) return KILN_C99_OUT_OF_MEMORY;
        for (at = 0; at < visible; ++at) {
            kiln_property descriptor;
            if (!kiln_c99_sealed_property_after(object, after, &after, &descriptor)) {
                free(properties);
                return KILN_C99_INVALID_ARGUMENT;
            }
            kiln_c99_heap_store_property(object, &properties[at], descriptor);
        }
        values = kiln_c99_sealed_values_of(object);
        kiln_c99_object_properties_free(object);
        kiln_c99_heap_store_pointer(object, &object->properties, properties);
        object->count = visible;
        object->capacity = visible;
        object->candidate_properties = 0;
        kiln_c99_heap_store_pointer(object, &object->slots, NULL);
        object->slot_capacity = 0;
        object->sealed = NULL;
        free(values);
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
        status = kiln_c99_object_reserve_properties(object, total_properties);
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
            kiln_c99_heap_store_pointer(object, &object->properties[pos].key, ikey);
            object->properties[pos].enumerable = 1;
            object->properties[pos].writable = 1;
            object->properties[pos].configurable = 1;
            object->properties[pos].symbol_hidden = 0;
            object->properties[pos].accessor = 0;
            object->properties[pos].refused = 0;
            kiln_c99_heap_store_value(object, &object->properties[pos].value, kiln_val_to_boundary(object->elements[i]));
            pos++;
        }
        kiln_c99_object_elements_free(object);
        kiln_c99_heap_store_pointer(object, &object->elements, NULL);
        object->element_count = 0;
        object->element_capacity = 0;
    }
    for (i = 0; i < shape->property_count; ++i) {
        kiln_property descriptor;
        kiln_c99_shape_property(object, (uint32_t)i, &descriptor);
        kiln_c99_heap_store_property(object, &object->properties[pos], descriptor);
        pos++;
    }
    object->count = pos;
    kiln_c99_object_slots_free(object);
    kiln_c99_heap_store_pointer(object, &object->slots, NULL);
    object->slot_capacity = 0;
    object->shape = NULL;
    return KILN_C99_OK;
}

static uint32_t *kiln_c99_sealed_orders(const kiln_object *object) {
    return (uint32_t *)(object->slots + object->sealed->count);
}

static uint32_t *kiln_c99_sealed_extra_orders(const kiln_object *object) {
    return kiln_c99_sealed_orders(object) + object->sealed->count;
}

static void kiln_c99_sealed_property(const kiln_object *object, size_t slot,
                                      kiln_property *out) {
    const kiln_sealed_property *entry = &object->sealed->entries[slot];
    memset(out, 0, sizeof *out);
    out->key = entry->key;
    out->enumerable = entry->enumerable;
    out->writable = entry->writable;
    out->configurable = entry->configurable;
    out->symbol_hidden = entry->symbol_hidden;
    out->value = kiln_val_to_boundary(object->slots[slot]);
}

/* Select the first visible record after an insertion-order number. A dictionary conversion
 * walks every record, so carrying `next` across that walk avoids repeating each earlier scan. */
static int kiln_c99_sealed_property_after(const kiln_object *object, uint32_t after,
                                          uint32_t *next, kiln_property *out) {
    const kiln_sealed_values *values = kiln_c99_sealed_values_of(object);
    uint32_t *orders = kiln_c99_sealed_orders(object);
    uint32_t *extras = kiln_c99_sealed_extra_orders(object);
    uint32_t best = UINT32_MAX;
    size_t best_index = 0;
    int best_extra = 0;
    size_t index;
    if (values->dense_order && object->count == 0) {
        if (after >= object->sealed->count || orders[after] == 0) return 0;
        *next = after + 1;
        kiln_c99_sealed_property(object, after, out);
        return 1;
    }
    for (index = 0; index < object->sealed->count; ++index) {
        if (orders[index] > after && orders[index] < best) {
            best = orders[index];
            best_index = index;
            best_extra = 0;
        }
    }
    for (index = 0; index < object->count; ++index) {
        if (extras[index] > after && extras[index] < best) {
            best = extras[index];
            best_index = index;
            best_extra = 1;
        }
    }
    if (best == UINT32_MAX) return 0;
    *next = best;
    if (best_extra) *out = object->properties[best_index];
    else kiln_c99_sealed_property(object, best_index, out);
    return 1;
}

/* Return the nth visible record in insertion order, including realm-only keys added between
 * descriptor keys. Random indexed reads still start from the beginning; conversion uses the
 * cursor above and never restarts it for each record. */
static int kiln_c99_sealed_property_nth(const kiln_object *object, size_t at,
                                         kiln_property *out) {
    const kiln_sealed_values *values = kiln_c99_sealed_values_of(object);
    uint32_t after = 0;
    size_t rank;
    if (values->dense_order && object->count == 0) {
        if (at >= object->sealed->count || kiln_c99_sealed_orders(object)[at] == 0) return 0;
        kiln_c99_sealed_property(object, at, out);
        return 1;
    }
    for (rank = 0; rank <= at; ++rank) {
        if (!kiln_c99_sealed_property_after(object, after, &after, out)) return 0;
    }
    return 1;
}

/* An own-property walk sees a process-sealed descriptor with realm-owned values, a shared
 * shape, or a realm-owned dictionary. Mixed dense/shape storage still converts as before. */
static int kiln_c99_property_walk(kiln_object *object, size_t *count) {
    if (object->sealed != NULL) {
        size_t index;
        size_t visible = object->count;
        uint32_t *orders = kiln_c99_sealed_orders(object);
        for (index = 0; index < object->sealed->count; ++index)
            visible += orders[index] != 0;
        *count = visible;
        return KILN_C99_OK;
    }
    if (object->shape != NULL &&
        (object->elements != NULL || object->properties != NULL || object->count != 0)) {
        int status = kiln_c99_ensure_dictionary(object);
        if (status != KILN_C99_OK) return status;
    }
    *count = object->shape != NULL ? object->shape->property_count : object->count;
    return KILN_C99_OK;
}

static const kiln_property *kiln_c99_property_view(const kiln_object *object, size_t index,
                                                    kiln_property *scratch) {
    if (object->sealed != NULL)
        return kiln_c99_sealed_property_nth(object, index, scratch) ? scratch : NULL;
    if (object->shape == NULL) return &object->properties[index];
    kiln_c99_shape_property(object, (uint32_t)index, scratch);
    return scratch;
}

/* Value-based, copied projections are the read contract for a future compact bootstrap handle.
 * A read site must not need a mutable `kiln_object *` or retain a borrowed dictionary pointer:
 * the compact handle will carry its own identity while descriptors live in process storage and
 * values remain in this realm. Ordinary objects still use their existing storage here. */
static kiln_value kiln_c99_object_prototype_read(kiln_value owner) {
    if (owner.tag != KILN_VALUE_OBJECT) return kiln_c99_undefined();
    return kiln_c99_object_of(owner)->prototype;
}

static int kiln_c99_property_walk_value(kiln_value owner, size_t *count) {
    if (owner.tag != KILN_VALUE_OBJECT) return KILN_C99_INVALID_ARGUMENT;
    return kiln_c99_property_walk(kiln_c99_object_of(owner), count);
}

static int kiln_c99_property_view_value(kiln_value owner, size_t index,
                                        kiln_property *out) {
    kiln_object *object;
    size_t count;
    if (owner.tag != KILN_VALUE_OBJECT || out == NULL) return 0;
    object = kiln_c99_object_of(owner);
    if (object->sealed != NULL) {
        uint32_t *orders = kiln_c99_sealed_orders(object);
        size_t slot;
        count = object->count;
        for (slot = 0; slot < object->sealed->count; ++slot) count += orders[slot] != 0;
    } else {
        count = object->shape != NULL ? object->shape->property_count : object->count;
    }
    if (index >= count) return 0;
    *out = *kiln_c99_property_view(object, index, out);
    return 1;
}

/* The first complete, prefix-free built-in layout. Keep this activation deliberately narrow:
 * Object.prototype's bootstrap store has four ordinary data properties and a stable BUILTIN
 * code. Every reader already understands a shaped object, and every value stays in this realm's
 * inline slots. Wider stores need a representation whose masks exceed 32 bits. */
static int kiln_c99_final_builtin_attrs(const kiln_property *property, uint8_t *attrs,
                                         int *number) {
    const kiln_string *key = property->key;
    uint8_t bits = 0;
    if (key == NULL || !kiln_c99_key_is_shared(key) || property->refused || property->accessor)
        return 0;
    if (kiln_c99_key_is_reserved(key)) {
        if (property->enumerable) return 0;
        if (!kiln_c99_key_is_symbol(key) && property->symbol_hidden) return 0;
        if (kiln_c99_key_is_symbol(key) && property->symbol_hidden)
            bits |= KILN_C99_ATTR_ENUM_FLIP;
    } else {
        if (property->symbol_hidden) return 0;
        if (!property->enumerable) bits |= KILN_C99_ATTR_ENUM_FLIP;
    }
    if (!property->writable) bits |= KILN_C99_ATTR_READONLY;
    if (!property->configurable) bits |= KILN_C99_ATTR_FIXED;
    *attrs = bits;
    *number = bits == 0 && property->value.tag == KILN_VALUE_NUMBER;
    return 1;
}

static int kiln_c99_final_builtin_matches(const kiln_shape *shape, const kiln_object *object) {
    size_t i;
    if (shape->property_count != object->count) return 0;
    for (i = 0; i < object->count; ++i) {
        uint8_t attrs;
        int number;
        if (!kiln_c99_final_builtin_attrs(&object->properties[i], &attrs, &number) ||
            shape->keys[i] != object->properties[i].key || shape->attrs[i] != attrs ||
            ((shape->double_slots_mask >> i) & 1u) != (uint32_t)number) return 0;
    }
    return 1;
}

static void kiln_c99_final_builtin_store(uint32_t tag, uint64_t code, kiln_object *object) {
    kiln_shared_state *process = kiln_c99_process();
    kiln_shape *shape;
    kiln_shape *candidate = NULL;
    size_t i;
    if (tag != KILN_VALUE_BUILTIN || code != KILN_C99_BUILTIN_OBJECT_PROTOTYPE ||
        object == NULL || object->shape != NULL || object->elements != NULL ||
        object->count == 0 || object->count > KILN_C99_INLINE_SLOTS ||
        object->slots != NULL) return;
    shape = __atomic_load_n(&process->final_builtin_layout.shape, __ATOMIC_ACQUIRE);
    if (shape == NULL) {
        candidate = (kiln_shape *)calloc(1, sizeof *candidate);
        if (candidate == NULL) return;
        candidate->property_count = (uint32_t)object->count;
        candidate->slot_index = (uint32_t)(object->count - 1);
        candidate->ikey = object->properties[object->count - 1].key;
        candidate->shared = 1 | KILN_C99_SHAPE_FINAL;
        for (i = 0; i < object->count; ++i) {
            uint8_t attrs;
            int number;
            const kiln_property *property = &object->properties[i];
            if (!kiln_c99_final_builtin_attrs(property, &attrs, &number)) {
                free(candidate);
                return;
            }
            uint32_t bit = 1u << i;
            candidate->keys[i] = property->key;
            candidate->attrs[i] = attrs;
            if (kiln_c99_key_is_reserved(property->key)) candidate->has_symbol_key = 1;
            if (number) candidate->double_slots_mask |= bit;
            if (attrs != 0) candidate->special_mask |= bit;
            if (attrs & KILN_C99_ATTR_READONLY) candidate->store_mask |= bit;
        }
        kiln_c99_process_lock(process);
        shape = process->final_builtin_layout.shape;
        if (shape == NULL) {
            process->final_builtin_layout.tag = tag;
            process->final_builtin_layout.code = code;
            __atomic_store_n(&process->final_builtin_layout.shape, candidate, __ATOMIC_RELEASE);
            shape = candidate;
            candidate = NULL;
        }
        kiln_c99_process_unlock(process);
        free(candidate);
    }
    if (process->final_builtin_layout.tag != tag ||
        process->final_builtin_layout.code != code ||
        !kiln_c99_final_builtin_matches(shape, object)) return;
    /* No frame may hold a pointer to these list records after bootstrap. A later mutation
     * converts this one object back to a realm-owned dictionary; the shared shape never grows
     * an edge. The collector already traces shaped values through the object's own slots. */
    kiln_c99_prototype_touched(object);
    kiln_c99_heap_store_interior_pointer(object, &object->slots, offsetof(kiln_object, inline_slots));
    object->slot_capacity = KILN_C99_INLINE_SLOTS;
    for (i = 0; i < KILN_C99_INLINE_SLOTS; ++i)
        kiln_c99_heap_store_val(object, &object->inline_slots[i], KILN_VAL_UNDEFINED);
    for (i = 0; i < object->count; ++i)
        kiln_c99_heap_store_val(object, &object->inline_slots[i], kiln_val_from_boundary(object->properties[i].value));
    kiln_c99_object_properties_free(object);
    kiln_c99_heap_store_pointer(object, &object->properties, NULL);
    object->capacity = 0;
    object->count = 0;
    object->shape = shape;
}

static const kiln_sealed_layout *kiln_c99_sealed_layout_find(uint32_t tag, uint64_t code) {
    kiln_shared_state *process = kiln_c99_process();
    kiln_sealed_layout *node = __atomic_load_n(&process->sealed_layouts, __ATOMIC_ACQUIRE);
    for (; node != NULL; node = node->next) {
        if (node->tag == tag && node->code == code) return node;
    }
    return NULL;
}

static uint32_t *kiln_c99_sealed_layout_buckets(const kiln_sealed_layout *layout) {
    return (uint32_t *)(layout->entries + layout->count);
}

/* A complete descriptor has immutable, canonical keys, so one process-owned index serves all
 * realms. The caller still checks its own visibility ordinals and reads its own value slots. */
static int kiln_c99_sealed_layout_index(const kiln_sealed_layout *layout,
                                         const kiln_string *key, const kiln_string *ikey,
                                         size_t *index) {
    const kiln_string *view = ikey != NULL ? ikey : key;
    const uint32_t *buckets;
    size_t slot, probes;
    if (view == NULL || layout->bucket_count == 0) return 0;
    buckets = kiln_c99_sealed_layout_buckets(layout);
    slot = (size_t)kiln_c99_key_hash(view) & (layout->bucket_count - 1);
    for (probes = 0; probes < layout->bucket_count; ++probes) {
        uint32_t entry = buckets[slot];
        kiln_property candidate;
        if (entry == 0) return 0;
        candidate = (kiln_property){.key = layout->entries[entry - 1].key};
        if ((ikey != NULL && candidate.key == ikey) ||
            kiln_c99_same_key_view(&candidate, view)) {
            *index = entry - 1;
            return 1;
        }
        slot = (slot + 1) & (layout->bucket_count - 1);
    }
    return 0;
}

/* Adopt a complete process descriptor only when this realm's bootstrap produced exactly the
 * same ordered keys and final attributes. The table never owns values: compact slots remain on
 * this realm's heap, and every later mutation can return to ordinary dictionary storage. */
static int kiln_c99_sealed_layout_attach(uint32_t tag, uint64_t code,
                                          kiln_object *object) {
    const kiln_sealed_layout *layout = kiln_c99_sealed_layout_find(tag, code);
    kiln_sealed_values *values;
    uint32_t *orders;
    size_t count;
    size_t bytes;
    size_t index;
    /* A same-binary diagnostic baseline for the per-realm RSS slope. The default is the sealed
     * representation; disabling it only retains the established dictionary representation. */
    if (getenv("KILN_DISABLE_SEALED_LAYOUTS") != NULL) return 0;
    if (layout == NULL || object == NULL || object->sealed != NULL ||
        object->shape != NULL || object->elements != NULL || object->slots != NULL ||
        object->count != layout->count || object->properties == NULL ||
        layout->count >= UINT32_MAX) return 0;
    count = layout->count;
    if (count > (SIZE_MAX - sizeof *values) / (sizeof(kiln_val) + sizeof(uint32_t)))
        return 0;
    for (index = 0; index < count; ++index) {
        const kiln_property *property = &object->properties[index];
        const kiln_sealed_property *entry = &layout->entries[index];
        if (property->key != entry->key || property->enumerable != entry->enumerable ||
            property->writable != entry->writable ||
            property->configurable != entry->configurable ||
            property->symbol_hidden != entry->symbol_hidden || property->accessor ||
            property->refused) return 0;
    }
    bytes = sizeof *values + count * (sizeof(kiln_val) + sizeof(uint32_t));
    values = malloc(bytes);
    if (values == NULL) return 0;
    values->next_order = (uint32_t)count + 1;
    values->extra_capacity = 0;
    values->dense_order = 1;
    orders = (uint32_t *)(values->values + count);
    for (index = 0; index < count; ++index) {
        kiln_c99_heap_store_val(object, &values->values[index],
                                  kiln_val_from_boundary(object->properties[index].value));
        orders[index] = (uint32_t)index + 1;
    }
    kiln_c99_object_properties_free(object);
    kiln_c99_heap_store_pointer(object, &object->properties, NULL);
    object->count = 0;
    object->capacity = 0;
    kiln_c99_heap_store_pointer(object, &object->slots, values->values);
    object->slot_capacity = count;
    object->sealed = layout;
    return 1;
}

/* Later realms construct a known callable store straight into their own compact values. A
 * missing layout or an unexpected bootstrap write declines to ordinary dictionary storage;
 * no partial descriptor is ever published to the process. */
static int kiln_c99_sealed_layout_attach_empty(uint32_t tag, uint64_t code,
                                                kiln_object *object) {
    const kiln_sealed_layout *layout = kiln_c99_sealed_layout_find(tag, code);
    kiln_sealed_values *values;
    uint32_t *orders;
    size_t count;
    size_t bytes;
    size_t index;
    if (getenv("KILN_DISABLE_SEALED_LAYOUTS") != NULL ||
        getenv("KILN_DISABLE_SEALED_DIRECT") != NULL || layout == NULL || object == NULL ||
        object->sealed != NULL || object->shape != kiln_c99_shape_root() ||
        object->properties != NULL || object->count != 0 || object->elements != NULL ||
        object->slots != object->inline_slots || layout->count >= UINT32_MAX) return 0;
    count = layout->count;
    if (count > (SIZE_MAX - sizeof *values) / (sizeof(kiln_val) + sizeof(uint32_t)))
        return 0;
    bytes = sizeof *values + count * (sizeof(kiln_val) + sizeof(uint32_t));
    values = malloc(bytes);
    if (values == NULL) return 0;
    values->next_order = 1;
    values->extra_capacity = 0;
    values->dense_order = 1;
    orders = (uint32_t *)(values->values + count);
    for (index = 0; index < count; ++index) {
        values->values[index] = KILN_VAL_UNDEFINED;
        orders[index] = 0;
    }
    object->shape = NULL;
    kiln_c99_heap_store_pointer(object, &object->slots, values->values);
    object->slot_capacity = count;
    object->sealed = layout;
    return 1;
}

/* Record a complete wide store after bootstrap. This table is process-owned metadata only:
 * every key is already canonical and shared, and neither a property value nor a realm object
 * pointer crosses the publication boundary. The candidate is built privately; the process
 * lock publishes a whole immutable node or discards it if another realm won the race. */
static void kiln_c99_sealed_layout_publish(uint32_t tag, uint64_t code,
                                            const kiln_object *object) {
    kiln_shared_state *process;
    kiln_sealed_layout *candidate;
    kiln_sealed_layout *known;
    size_t index;
    size_t bytes;
    size_t bucket_count = 1;
    if (kiln_c99_sealed_layout_find(tag, code) != NULL || object == NULL ||
        object->shape != NULL || object->properties == NULL ||
        object->count == 0 || object->count >= UINT32_MAX ||
        object->count > SIZE_MAX / 2 ||
        object->count > (SIZE_MAX - sizeof *candidate) /
                        sizeof(kiln_sealed_property)) return;
    while (bucket_count < object->count * 2) {
        if (bucket_count > SIZE_MAX / 2) return;
        bucket_count *= 2;
    }
    bytes = sizeof *candidate + object->count * sizeof(kiln_sealed_property);
    if (bucket_count > (SIZE_MAX - bytes) / sizeof(uint32_t)) return;
    bytes += bucket_count * sizeof(uint32_t);
    candidate = (kiln_sealed_layout *)malloc(bytes);
    if (candidate == NULL) return;
    candidate->next = NULL;
    candidate->tag = tag;
    candidate->code = code;
    candidate->count = object->count;
    candidate->bucket_count = bucket_count;
    memset(kiln_c99_sealed_layout_buckets(candidate), 0,
           bucket_count * sizeof(uint32_t));
    for (index = 0; index < object->count; ++index) {
        const kiln_property *property = &object->properties[index];
        kiln_sealed_property *entry = &candidate->entries[index];
        if (property->key == NULL || !kiln_c99_key_is_shared(property->key) ||
            property->refused || property->accessor) {
            free(candidate);
            return;
        }
        entry->key = property->key;
        entry->enumerable = (uint8_t)property->enumerable;
        entry->writable = (uint8_t)property->writable;
        entry->configurable = (uint8_t)property->configurable;
        entry->symbol_hidden = (uint8_t)property->symbol_hidden;
        {
            uint32_t *buckets = kiln_c99_sealed_layout_buckets(candidate);
            size_t slot = (size_t)kiln_c99_key_hash(entry->key) & (bucket_count - 1);
            while (buckets[slot] != 0) {
                if (candidate->entries[buckets[slot] - 1].key == entry->key) {
                    free(candidate);
                    return;
                }
                slot = (slot + 1) & (bucket_count - 1);
            }
            buckets[slot] = (uint32_t)index + 1;
        }
    }
    process = kiln_c99_process();
    kiln_c99_process_lock(process);
    for (known = process->sealed_layouts; known != NULL; known = known->next) {
        if (known->tag == tag && known->code == code) break;
    }
    if (known == NULL) {
        candidate->next = process->sealed_layouts;
        __atomic_store_n(&process->sealed_layouts, candidate, __ATOMIC_RELEASE);
        candidate = NULL;
    }
    kiln_c99_process_unlock(process);
    free(candidate);
}

/* Both the realm allocator and a private graph arena use this exact object layout. */
static void kiln_c99_object_init(kiln_object *object, size_t tail_slots) {
    kiln_c99_heap_store_pointer(object, &object->properties, NULL);
    object->count = 0;
    object->capacity = 0;
    kiln_c99_heap_store_value(object, &object->prototype, kiln_c99_undefined());
    object->frozen = 0;
    object->not_extensible = 0;
    object->module_namespace = 0;
    object->arguments_brand = 0;
    object->error_brand = 0;
    object->layout_token = 0;
    object->sealed = NULL;
    object->candidate_properties = 0;
    object->candidate_slots = 0;
    object->candidate_elements = 0;
    object->accessor_count = 0;
    object->read_only_seen = 0;
    object->shape = kiln_c99_shape_root();
    kiln_c99_heap_store_interior_pointer(object, &object->slots,
        tail_slots == 0 ? offsetof(kiln_object, inline_slots) : sizeof(kiln_object));
    object->slot_capacity = tail_slots == 0 ? KILN_C99_INLINE_SLOTS : tail_slots;
    for (size_t slot_i = 0; slot_i < KILN_C99_INLINE_SLOTS; ++slot_i) {
        kiln_c99_heap_store_val(object, &object->inline_slots[slot_i], KILN_VAL_UNDEFINED);
    }
    for (size_t slot_i = 0; slot_i < tail_slots; ++slot_i)
        kiln_c99_heap_store_val(object, &object->slots[slot_i], KILN_VAL_UNDEFINED);
    kiln_c99_heap_store_pointer(object, &object->elements, NULL);
    object->element_count = 0;
    object->element_capacity = 0;
}

static int kiln_c99_new_object_sized(kiln_value *out, size_t tail_slots) {
    /* Every field is written by the initializer before anything can observe the object, so
     * the block is taken unzeroed. */
    kiln_object *object;
    if (tail_slots > (SIZE_MAX - sizeof(kiln_object)) / sizeof(kiln_val))
        return KILN_C99_OUT_OF_MEMORY;
    object = (kiln_object *)kiln_c99_gc_alloc_hot(
        KILN_GC_OBJECT, sizeof *object + tail_slots * sizeof(kiln_val), 0);
    if (object == NULL) return KILN_C99_OUT_OF_MEMORY;
    kiln_c99_object_init(object, tail_slots);
    *out = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)object, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_new_object(kiln_value *out) {
    return kiln_c99_new_object_sized(out, 0);
}

/* A property/prototype store can be a slot in an older owner. Publishing a newly built
 * candidate object through that slot is an incoming edge, independently of its own fields. */
static int kiln_c99_new_object_for_owner(kiln_value *out, const void *owner_payload) {
    kiln_value built;
    int status = kiln_c99_new_object(&built);
    if (status == KILN_C99_OK) kiln_c99_heap_store_value(owner_payload, out, built);
    return status;
}

/* Allocate or recover this realm's identity for one graph node. The weak chain is not a GC root;
 * its entries leave on finalization. A second read of the same node while it is live therefore
 * returns exactly the same JS object, including after a message is echoed back to this realm. */
static int kiln_c99_new_shared_facade(uint32_t graph, uint32_t generation, uint32_t node,
                                      kiln_value *out) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    kiln_shared_facade *cached;
    uint64_t lease;
    kiln_shared_arena_view arena;
    kiln_shared_facade *facade;
    cached = kiln_c99_shared_facade_find(heap, graph, generation, node);
    if (cached != NULL) {
        *out = kiln_c99_value(KILN_VALUE_SHARED, (uint64_t)(uintptr_t)cached, NULL, 0);
        return KILN_C99_OK;
    }
    if (kiln_c99_shared_facade_reserve(heap) != KILN_C99_OK) return KILN_C99_OUT_OF_MEMORY;
    lease = kiln_shared_graph_retain(graph, generation, node);
    if (lease == 0) return KILN_C99_INVALID_ARGUMENT;
    if (kiln_shared_graph_view(lease, &arena) != 0 || arena.base == NULL ||
        arena.layout_version != KILN_SHARED_ARENA_LAYOUT_VERSION || node >= arena.node_count) {
        (void)kiln_shared_graph_release(lease);
        return KILN_C99_INVALID_ARGUMENT;
    }
    facade = (kiln_shared_facade *)kiln_c99_gc_alloc_tenured(KILN_GC_SHARED_FACADE, sizeof *facade);
    if (facade == NULL) {
        (void)kiln_shared_graph_release(lease);
        return KILN_C99_OUT_OF_MEMORY;
    }
    facade->lease = lease;
    facade->arena = arena;
    facade->graph = graph;
    facade->generation = generation;
    facade->node = node;
    kiln_c99_heap_store_pointer(facade, &facade->owner, heap);
    kiln_c99_heap_store_pointer(facade, &facade->previous, NULL);
    kiln_c99_heap_store_pointer(facade, &facade->next, heap->shared_facades);
    if (facade->next != NULL) kiln_c99_heap_store_pointer(facade->next, &facade->next->previous, facade);
    heap->shared_facades = facade;
    kiln_c99_shared_facade_insert(heap, facade);
    *out = kiln_c99_value(KILN_VALUE_SHARED, (uint64_t)(uintptr_t)facade, NULL, 0);
    return KILN_C99_OK;
}

/* Copy one graph key into this realm. Provider field reads never return an arena pointer; the
 * facade lease protects the graph while both copies are made. */
static int kiln_c99_shared_field_key(uint64_t lease, uint64_t index, kiln_value *out,
                                     int *enumerable) {
    kiln_shared_field_info field;
    uint16_t small[64];
    uint16_t *units = small;
    int status;
    if (kiln_shared_graph_field(lease, index, &field, NULL, 0, NULL, 0) != 0 ||
        field.key_length > SIZE_MAX / sizeof(uint16_t)) return KILN_C99_INVALID_ARGUMENT;
    if (field.key_length > sizeof small / sizeof small[0]) {
        units = (uint16_t *)malloc((size_t)field.key_length * sizeof(uint16_t));
        if (units == NULL) return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_shared_graph_field(lease, index, &field, units, field.key_length, NULL, 0);
    if (status == 0) {
        *out = kiln_c99_from_units(units, (size_t)field.key_length);
        if (out->tag != KILN_VALUE_STRING) status = -1;
        if (enumerable != NULL) *enumerable = field.enumerable != 0;
    }
    if (units != small) free(units);
    return status == 0 ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
}

static int kiln_c99_shared_is_array(kiln_value value) {
    kiln_shared_facade *facade = (kiln_shared_facade *)(uintptr_t)value.bits;
    kiln_shared_info info;
    return facade != NULL && facade->lease != 0 &&
           kiln_shared_graph_info(facade->lease, &info) == 0 && info.array != 0;
}

static int kiln_c99_shared_prototype(kiln_value value, kiln_value *out) {
    kiln_shared_facade *facade = (kiln_shared_facade *)(uintptr_t)value.bits;
    kiln_shared_info info;
    if (facade == NULL || facade->lease == 0 ||
        kiln_shared_graph_info(facade->lease, &info) != 0) return KILN_C99_INVALID_ARGUMENT;
    *out = info.array ? kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_PROTOTYPE)
          : info.null_prototype ? kiln_c99_null()
          : kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
    return KILN_C99_OK;
}

/* The source IDs passed to the Rust builder identify a live C heap object during this one
 * synchronous call. The low bits distinguish arrays from records without giving Rust a C
 * pointer layout. No callback allocates in the C heap or calls JavaScript. */
#define KILN_C99_SHARED_SOURCE_RECORD UINT64_C(1)
#define KILN_C99_SHARED_SOURCE_ARRAY UINT64_C(2)

static uint64_t kiln_c99_shared_source_id(kiln_value value) {
    uint64_t kind = value.tag == KILN_VALUE_ARRAY ? KILN_C99_SHARED_SOURCE_ARRAY
                                                 : KILN_C99_SHARED_SOURCE_RECORD;
    return value.bits | kind;
}

static kiln_object *kiln_c99_shared_source_store(uint64_t id, kiln_array **array_out) {
    uintptr_t pointer = (uintptr_t)(id & ~UINT64_C(7));
    *array_out = NULL;
    if ((id & 7) == KILN_C99_SHARED_SOURCE_RECORD) return (kiln_object *)pointer;
    if ((id & 7) == KILN_C99_SHARED_SOURCE_ARRAY) {
        kiln_array *array = (kiln_array *)pointer;
        *array_out = array;
        return array->properties.tag == KILN_VALUE_OBJECT
                   ? (kiln_object *)(uintptr_t)array->properties.bits : NULL;
    }
    return NULL;
}

static int kiln_c99_shared_source_value(kiln_value value, kiln_shared_source_field *field) {
    field->value_bits = value.bits;
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED: field->value_kind = 0; break;
    case KILN_VALUE_NULL: field->value_kind = 1; break;
    case KILN_VALUE_BOOLEAN: field->value_kind = 2; break;
    case KILN_VALUE_NUMBER: field->value_kind = 3; break;
    case KILN_VALUE_BIGINT: field->value_kind = 4; break;
    case KILN_VALUE_STRING: {
        const kiln_string *text = (const kiln_string *)value.ptr;
        if (text == NULL) return -1;
        field->value_kind = 5;
        field->value_bits = (uint64_t)(uintptr_t)text;
        field->value_length = text->unit_len;
        break;
    }
    case KILN_VALUE_OBJECT:
    case KILN_VALUE_ARRAY:
        field->value_kind = 7;
        field->value_bits = kiln_c99_shared_source_id(value);
        break;
    case KILN_VALUE_SHARED: {
        kiln_shared_facade *facade = (kiln_shared_facade *)(uintptr_t)value.bits;
        if (facade == NULL || facade->lease == 0) return -1;
        field->value_kind = 8;
        field->value_bits = facade->graph;
        field->value_generation = facade->generation;
        field->value_node = facade->node;
        break;
    }
    default: field->value_kind = 9; break;
    }
    return 0;
}

static int kiln_c99_shared_source_inspect(void *context, uint64_t id, uint64_t index,
                                           kiln_shared_source_node *node,
                                           kiln_shared_source_field *field) {
    kiln_array *array;
    kiln_object *store = kiln_c99_shared_source_store(id, &array);
    size_t elements = array != NULL ? array->count : store != NULL ? store->element_count : 0;
    size_t named = store == NULL ? 0 : store->shape != NULL
                                              ? store->shape->property_count : store->count;
    (void)context;
    if (array == NULL && store == NULL) return -1;
    if (index == UINT64_MAX) {
        kiln_value prototype = store == NULL ? kiln_c99_undefined() : store->prototype;
        if (node == NULL || (array != NULL && array->match_index >= 0) ||
            (store != NULL && (store->error_brand || store->arguments_brand ||
                               store->module_namespace))) return -1;
        if (array == NULL) {
            if (prototype.tag != KILN_VALUE_UNDEFINED && prototype.tag != KILN_VALUE_NULL &&
                !(prototype.tag == KILN_VALUE_BUILTIN &&
                  prototype.bits == KILN_C99_BUILTIN_OBJECT_PROTOTYPE)) return -1;
        } else if (prototype.tag != KILN_VALUE_UNDEFINED &&
                   !(prototype.tag == KILN_VALUE_BUILTIN &&
                     prototype.bits == KILN_C99_BUILTIN_ARRAY_PROTOTYPE)) return -1;
        if (array != NULL && array->count > UINT32_MAX) return -1;
        if (store != NULL) {
            for (size_t at = 0; at < named; ++at) {
                const kiln_string *key = store->shape != NULL
                                             ? store->shape->keys[at]
                                             : store->properties[at].key;
                if (key != NULL && (key->flags & KILN_STRING_PRIVATE_KEY) != 0) return -1;
            }
        }
        node->kind = array == NULL ? 1 : 2;
        node->null_prototype = prototype.tag == KILN_VALUE_NULL;
        node->length = array == NULL ? 0 : (uint32_t)array->count;
        node->field_count = (uint64_t)elements + (uint64_t)named;
        return 0;
    }
    if (field == NULL || index >= (uint64_t)elements + (uint64_t)named) return -1;
    memset(field, 0, sizeof *field);
    if (index < elements) {
        kiln_val value = array != NULL ? array->elements[index] : store->elements[index];
        if (value == KILN_VAL_HOLE) { field->key_kind = 3; return 0; }
        field->key_kind = 2;
        field->key_id = index;
        field->enumerable = 1;
        return kiln_c99_shared_source_value(kiln_val_to_boundary(value), field);
    }
    index -= elements;
    if (store->shape != NULL) {
        const kiln_string *key = store->shape->keys[index];
        uint32_t attrs = store->shape->attrs[index];
        if (key == NULL || index >= store->slot_capacity) return -1;
        field->key_kind = kiln_c99_key_is_symbol(key) ? 1 : 0;
        field->key_id = (uint64_t)(uintptr_t)key;
        field->key_length = key->unit_len;
        field->enumerable = kiln_c99_shape_slot_enumerable(store->shape, (uint32_t)index);
        field->accessor = (attrs & KILN_C99_ATTR_ACCESSOR) != 0;
        if (field->accessor) return 0;
        return kiln_c99_shared_source_value(kiln_val_to_boundary(store->slots[index]), field);
    }
    {
        const kiln_property *slot = &store->properties[index];
        if (slot->key == NULL || slot->refused) return -1;
        field->key_kind = kiln_c99_key_is_symbol(slot->key) ? 1 : 0;
        field->key_id = (uint64_t)(uintptr_t)slot->key;
        field->key_length = slot->key->unit_len;
        field->enumerable = slot->enumerable;
        field->accessor = slot->accessor;
        if (slot->accessor) return 0;
        return kiln_c99_shared_source_value(slot->value, field);
    }
}

static int kiln_c99_shared_source_unit(void *context, uint64_t string, uint64_t index,
                                        uint16_t *out) {
    const kiln_string *text = (const kiln_string *)(uintptr_t)string;
    (void)context;
    if (text == NULL || out == NULL || index >= text->unit_len) return -1;
    *out = kiln_string_unit_at(text, (size_t)index);
    return 0;
}

/* Internal until the full facade/reflection/message contract has both-engine parity. */
static int kiln_c99_share_value(kiln_value input, kiln_value *out) {
    kiln_shared_build_result result;
    int status;
    if (out == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (input.tag == KILN_VALUE_SHARED || input.tag == KILN_VALUE_UNDEFINED ||
        input.tag == KILN_VALUE_NULL || input.tag == KILN_VALUE_BOOLEAN ||
        input.tag == KILN_VALUE_NUMBER || input.tag == KILN_VALUE_BIGINT ||
        input.tag == KILN_VALUE_STRING) {
        *out = input;
        return KILN_C99_OK;
    }
    if (input.tag != KILN_VALUE_OBJECT && input.tag != KILN_VALUE_ARRAY)
        return KILN_C99_INVALID_ARGUMENT;
    status = kiln_shared_graph_build(kiln_c99_shared_source_id(input),
                                     kiln_c99_shared_source_inspect, kiln_c99_shared_source_unit,
                                     NULL, &result);
    if (status != 0) return status == -3 ? KILN_C99_OUT_OF_MEMORY : KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_new_shared_facade(result.graph, result.generation, result.node, out);
    (void)kiln_shared_graph_release(result.lease);
    return status;
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
    if (capacity > KILN_C99_INLINE_SLOTS && capacity <= KILN_SHAPE_MAX_FAST_PROPERTIES)
        return kiln_c99_new_object_sized(out, capacity);
    int status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) {
        return status;
    }
    object = kiln_c99_object_of(*out);
    if (capacity > KILN_C99_INLINE_SLOTS) {
        kiln_val *new_slots = kiln_c99_object_slots_resize(object, capacity);
        size_t i;
        if (new_slots == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (i = KILN_C99_INLINE_SLOTS; i < capacity; ++i) {
            new_slots[i] = KILN_VAL_UNDEFINED;
        }
        kiln_c99_heap_store_pointer(object, &object->slots, new_slots);
        object->slot_capacity = capacity;
    }
    return KILN_C99_OK;
}

static int kiln_c99_new_object_tenured(kiln_value *out) {
    kiln_object *object = (kiln_object *)kiln_c99_gc_alloc_tenured(KILN_GC_OBJECT, sizeof *object);
    if (object == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    kiln_c99_heap_store_pointer(object, &object->properties, NULL);
    object->count = 0;
    object->capacity = 0;
    kiln_c99_heap_store_value(object, &object->prototype, kiln_c99_undefined());
    object->frozen = 0;
    object->not_extensible = 0;
    object->module_namespace = 0;
    object->arguments_brand = 0;
    object->error_brand = 0;
    object->layout_token = 0;
    object->sealed = NULL;
    object->candidate_properties = 0;
    object->candidate_slots = 0;
    object->candidate_elements = 0;
    object->accessor_count = 0;
    object->read_only_seen = 0;
    object->shape = kiln_c99_shape_root();
    kiln_c99_heap_store_interior_pointer(object, &object->slots, offsetof(kiln_object, inline_slots));
    object->slot_capacity = KILN_C99_INLINE_SLOTS;
    for (size_t slot_i = 0; slot_i < KILN_C99_INLINE_SLOTS; ++slot_i) {
        kiln_c99_heap_store_val(object, &object->inline_slots[slot_i], KILN_VAL_UNDEFINED);
    }
    kiln_c99_heap_store_pointer(object, &object->elements, NULL);
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
        kiln_c99_heap_store_pointer(object, &object->slots, new_slots);
        object->slot_capacity = capacity;
    }
    return KILN_C99_OK;
}
