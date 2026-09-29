
static kiln_collection *kiln_c99_collection_of(kiln_value value) {
    return (kiln_collection *)(uintptr_t)value.bits;
}

static void kiln_c99_collection_register_weak(kiln_collection *collection) {
    kiln_gc_heap *heap;
    if (!collection->is_weak) return;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return;
    collection->weak_owner = heap;
    collection->weak_next = heap->weak_collections;
    if (collection->weak_next != NULL) collection->weak_next->weak_previous = collection;
    heap->weak_collections = collection;
}

static kiln_proxy *kiln_c99_proxy_of(kiln_value value) {
    return (kiln_proxy *)(uintptr_t)value.bits;
}

/* The thing a proxy ultimately stands in front of, following a chain of them.
 *
 * Only for the questions that are about the *kind* of value rather than about its properties --
 * `Array.isArray`, `typeof`, whether a call is possible. A property read must never take this
 * shortcut: skipping the handler is exactly the bug a proxy exists to make impossible. */
static kiln_value kiln_c99_proxy_inner(kiln_value value) {
    size_t steps;
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        if (value.tag != KILN_VALUE_PROXY) {
            return value;
        }
        value = kiln_c99_proxy_of(value)->target;
    }
    return value;
}

static int kiln_c99_new_proxy(kiln_value *out, kiln_value target, kiln_value handler) {
    kiln_proxy *proxy = (kiln_proxy *)kiln_c99_gc_alloc(KILN_GC_PROXY, sizeof *proxy);
    if (proxy == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    kiln_c99_heap_store_value(proxy, &proxy->target, target);
    kiln_c99_heap_store_value(proxy, &proxy->handler, handler);
    proxy->revoked = 0;
    *out = kiln_c99_value(KILN_VALUE_PROXY, (uint64_t)(uintptr_t)proxy, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_new_collection(kiln_value *out, int is_map, int is_weak) {
    kiln_collection *collection =
        (kiln_collection *)kiln_c99_gc_alloc_hot(KILN_GC_COLLECTION, sizeof *collection, 0);
    if (collection == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    collection->entries = NULL;
    collection->count = 0;
    collection->live = 0;
    collection->capacity = 0;
    collection->hash_table = NULL;
    collection->hash_capacity = 0;
    collection->is_map = is_map;
    collection->is_weak = is_weak;
    collection->weak_previous = NULL;
    collection->weak_next = NULL;
    collection->weak_owner = NULL;
    kiln_c99_heap_store_value(collection, &collection->properties, kiln_c99_undefined());
    kiln_c99_collection_register_weak(collection);
    *out = kiln_c99_value(KILN_VALUE_COLLECTION, (uint64_t)(uintptr_t)collection, NULL, 0);
    return KILN_C99_OK;
}

/* Map/Set hashes follow SameValueZero's string units. The boundary pointer names a canonical
 * header; hashing it as if it were a byte span folds the header's first bytes into the key and
 * makes an equal Latin1/UTF16 pair land in different buckets. */
static uint64_t kiln_c99_collection_string_hash(const kiln_string *string) {
    if (string == NULL || string->unit_len == 0 || string->data == NULL) return 0;
    const uint64_t mix = UINT64_C(0xff51afd7ed558ccd);
    size_t length = string->unit_len;
    uint64_t hash = UINT64_C(0x9e3779b97f4a7c15) ^ (uint64_t)length;
    size_t index;
    if (KILN_LIKELY(!kiln_string_is_utf16(string))) {
        const uint8_t *bytes = (const uint8_t *)string->data;
        for (index = 0; index < length; ++index) {
            hash = (hash ^ (uint64_t)bytes[index]) * mix;
            hash ^= hash >> 29;
        }
    } else {
        const uint16_t *units = (const uint16_t *)string->data;
        for (index = 0; index < length; ++index) {
            hash = (hash ^ (uint64_t)units[index]) * mix;
            hash ^= hash >> 29;
        }
    }
    return hash ^ (hash >> 32);
}

static uint64_t kiln_c99_value_hash(kiln_value v) {
    if (v.tag == KILN_VALUE_NUMBER) {
        uint64_t bits = (uint64_t)v.bits;
        double d;
        memcpy(&d, &bits, sizeof bits);
        if (isnan(d)) {
            return UINT64_C(0x7ff8000000000000);
        }
        if (d == 0.0) {
            bits = 0;
        }
        bits ^= bits >> 32;
        bits *= UINT64_C(0xff51afd7ed558ccd);
        return bits ^ (bits >> 29);
    }
    if (v.tag == KILN_VALUE_STRING) {
        return kiln_c99_collection_string_hash((const kiln_string *)v.ptr);
    }
    if (v.tag == KILN_VALUE_SYMBOL) {
        const kiln_symbol *sym = kiln_c99_symbol_of(v);
        if (sym != NULL && sym->registry_id != 0) {
            return (uint64_t)sym->registry_id * UINT64_C(0x9e3779b97f4a7c15);
        }
        return (uint64_t)(uintptr_t)v.bits * UINT64_C(0x9e3779b97f4a7c15);
    }
    if (v.tag == KILN_VALUE_BIGINT) {
        int64_t high = 0;
        uint64_t low = 0;
        if (kiln_bigint_to_i128((const void *)(uintptr_t)v.bits, &high, &low) == 0) {
            uint64_t h = (uint64_t)high ^ low;
            h *= UINT64_C(0xff51afd7ed558ccd);
            return h ^ (h >> 29);
        }
    }
    {
        uint64_t h = ((uint64_t)v.tag << 32) ^ (uint64_t)v.bits;
        h ^= h >> 29;
        h *= UINT64_C(0xff51afd7ed558ccd);
        return h ^ (h >> 32);
    }
}

static int kiln_c99_collection_rehash(kiln_collection *collection, size_t new_cap) {
    uint32_t *new_table = (uint32_t *)calloc(new_cap, sizeof(uint32_t));
    size_t mask;
    size_t i;
    if (new_table == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    mask = new_cap - 1;
    for (i = 0; i < collection->count; ++i) {
        if (!collection->entries[i].dead) {
            kiln_value k = collection->entries[i].key;
            uint64_t hash;
            if (KILN_LIKELY(k.tag == KILN_VALUE_NUMBER)) {
                uint64_t bits = (uint64_t)k.bits;
                double d;
                memcpy(&d, &bits, sizeof(d));
                if (KILN_UNLIKELY(isnan(d))) {
                    hash = UINT64_C(0x7ff8000000000000);
                } else {
                    if (d == 0.0) bits = 0;
                    bits ^= bits >> 32;
                    bits *= UINT64_C(0xff51afd7ed558ccd);
                    hash = bits ^ (bits >> 29);
                }
            } else {
                hash = kiln_c99_value_hash(k);
            }
            size_t at = (size_t)hash & mask;
            while (new_table[at] != 0) {
                at = (at + 1) & mask;
            }
            new_table[at] = (uint32_t)(i + 1);
        }
    }
    free(collection->hash_table);
    collection->hash_table = new_table;
    collection->hash_capacity = new_cap;
    return KILN_C99_OK;
}

static void kiln_c99_collection_hash_remove(kiln_collection *collection, size_t entry_idx, kiln_value key) {
    size_t mask;
    size_t at;
    size_t hole;
    if (collection->hash_table == NULL || collection->hash_capacity == 0) {
        return;
    }
    mask = collection->hash_capacity - 1;
    at = (size_t)kiln_c99_value_hash(key) & mask;
    while (collection->hash_table[at] != 0) {
        if (collection->hash_table[at] == (uint32_t)(entry_idx + 1)) {
            break;
        }
        at = (at + 1) & mask;
    }
    if (collection->hash_table[at] == 0) {
        return;
    }
    hole = at;
    collection->hash_table[hole] = 0;
    at = (hole + 1) & mask;
    while (collection->hash_table[at] != 0) {
        uint32_t cand_idx = collection->hash_table[at] - 1;
        size_t home = (size_t)kiln_c99_value_hash(collection->entries[cand_idx].key) & mask;
        if (((hole - home) & mask) < ((at - home) & mask)) {
            collection->hash_table[hole] = collection->hash_table[at];
            collection->hash_table[at] = 0;
            hole = at;
        }
        at = (at + 1) & mask;
    }
}

/* The index of a key, or -1. SameValueZero, which is what a collection compares with: NaN finds
 * itself, where `===` never would, and that is why `new Set([NaN, NaN]).size` is 1. */
static double kiln_c99_collection_find_unleased(kiln_collection *collection, kiln_value key) {
    size_t index;
    if (collection->count == 0) {
        return -1.0;
    }
    if (collection->hash_table != NULL && collection->hash_capacity > 0) {
        size_t mask = collection->hash_capacity - 1;
        size_t at = (size_t)kiln_c99_value_hash(key) & mask;
        while (collection->hash_table[at] != 0) {
            uint32_t idx = collection->hash_table[at] - 1;
            if (!collection->entries[idx].dead) {
                kiln_value k = collection->entries[idx].key;
                if (k.bits == key.bits && k.tag == key.tag) {
                    return (double)idx;
                }
                if (kiln_c99_same_value_zero(k, key)) {
                    return (double)idx;
                }
            }
            at = (at + 1) & mask;
        }
        return -1.0;
    }
    for (index = 0; index < collection->count; ++index) {
        if (collection->entries[index].dead) {
            continue;
        }
        kiln_value k = collection->entries[index].key;
        if (k.bits == key.bits && k.tag == key.tag) {
            return (double)index;
        }
        if (kiln_c99_same_value_zero(k, key)) {
            return (double)index;
        }
    }
    return -1.0;
}

static int kiln_c99_collection_put_unleased(kiln_value target, kiln_value key, kiln_value value) {
    kiln_collection *collection = kiln_c99_collection_of(target);
    int status;
    size_t new_idx;
    size_t empty_slot = 0;
    int has_empty_slot = 0;
    if (collection->count > 0 && collection->hash_table != NULL && collection->hash_capacity > 0) {
        size_t mask = collection->hash_capacity - 1;
        size_t at = (size_t)kiln_c99_value_hash(key) & mask;
        while (collection->hash_table[at] != 0) {
            uint32_t idx = collection->hash_table[at] - 1;
            if (!collection->entries[idx].dead) {
                kiln_value k = collection->entries[idx].key;
                if ((k.bits == key.bits && k.tag == key.tag) || kiln_c99_same_value_zero(k, key)) {
                    kiln_c99_heap_store_value(collection, &collection->entries[idx].value, value);
                    return KILN_C99_OK;
                }
            }
            at = (at + 1) & mask;
        }
        empty_slot = at;
        has_empty_slot = 1;
    } else if (collection->count > 0 && collection->hash_table == NULL) {
        for (size_t index = 0; index < collection->count; ++index) {
            if (collection->entries[index].dead) continue;
            kiln_value k = collection->entries[index].key;
            if ((k.bits == key.bits && k.tag == key.tag) || kiln_c99_same_value_zero(k, key)) {
                kiln_c99_heap_store_value(collection, &collection->entries[index].value, value);
                return KILN_C99_OK;
            }
        }
    }
    status = kiln_c99_reserve((void **)&collection->entries, &collection->capacity,
                              collection->count + 1, sizeof *collection->entries);
    if (status != KILN_C99_OK) {
        return status;
    }
    new_idx = collection->count;
    kiln_c99_heap_store_value(collection, &collection->entries[new_idx].key, key);
    kiln_c99_heap_store_value(collection, &collection->entries[new_idx].value, value);
    collection->entries[new_idx].dead = 0;
    collection->count += 1;
    collection->live += 1;

    if (collection->hash_table == NULL) {
        if (collection->count >= 8) {
            status = kiln_c99_collection_rehash(collection, 16);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    } else {
        if ((collection->count + 1) * 2 > collection->hash_capacity) {
            status = kiln_c99_collection_rehash(collection, collection->hash_capacity * 2);
            if (status != KILN_C99_OK) {
                return status;
            }
        } else if (has_empty_slot) {
            collection->hash_table[empty_slot] = (uint32_t)(new_idx + 1);
        } else {
            size_t mask = collection->hash_capacity - 1;
            size_t h_at = (size_t)kiln_c99_value_hash(key) & mask;
            while (collection->hash_table[h_at] != 0) {
                h_at = (h_at + 1) & mask;
            }
            collection->hash_table[h_at] = (uint32_t)(new_idx + 1);
        }
    }
    return KILN_C99_OK;
}

/* A shared key's facade is a realm-local weak identity. Pin its graph for the entire
 * lookup/update, including a hash-table allocation, so retirement cannot race this read. */
static uint64_t kiln_c99_collection_key_lease(kiln_value key) {
    kiln_shared_facade *facade;
    if (key.tag != KILN_VALUE_SHARED) return 0;
    facade = (kiln_shared_facade *)(uintptr_t)key.bits;
    return kiln_shared_graph_retain(facade->graph, facade->generation, facade->node);
}

static double kiln_c99_collection_find(kiln_collection *collection, kiln_value key) {
    uint64_t lease = kiln_c99_collection_key_lease(key);
    double at;
    if (key.tag == KILN_VALUE_SHARED && lease == 0) return -1.0;
    at = kiln_c99_collection_find_unleased(collection, key);
    if (lease != 0) (void)kiln_shared_graph_release(lease);
    return at;
}

static int kiln_c99_collection_put(kiln_value target, kiln_value key, kiln_value value) {
    uint64_t lease = kiln_c99_collection_key_lease(key);
    int status;
    if (key.tag == KILN_VALUE_SHARED && lease == 0) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_collection_put_unleased(target, key, value);
    if (lease != 0) (void)kiln_shared_graph_release(lease);
    return status;
}
