
static kiln_collection *kiln_c99_collection_of(kiln_value value) {
    return (kiln_collection *)(uintptr_t)value.bits;
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
    proxy->target = target;
    proxy->handler = handler;
    proxy->revoked = 0;
    *out = kiln_c99_value(KILN_VALUE_PROXY, (uint64_t)(uintptr_t)proxy, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_new_collection(kiln_value *out, int is_map, int is_weak) {
    kiln_collection *collection =
        (kiln_collection *)kiln_c99_gc_alloc(KILN_GC_COLLECTION, sizeof *collection);
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
    collection->properties = kiln_c99_undefined();
    *out = kiln_c99_value(KILN_VALUE_COLLECTION, (uint64_t)(uintptr_t)collection, NULL, 0);
    return KILN_C99_OK;
}

static uint64_t kiln_c99_value_hash(kiln_value v) {
    if (v.tag == KILN_VALUE_NUMBER) {
        double d = kiln_c99_as_number(v);
        uint64_t bits;
        if (isnan(d)) {
            return UINT64_C(0x7ff8000000000000);
        }
        if (d == 0.0) {
            d = 0.0;
        }
        memcpy(&bits, &d, sizeof bits);
        bits ^= bits >> 32;
        bits *= UINT64_C(0xff51afd7ed558ccd);
        return bits ^ (bits >> 29);
    }
    if (v.tag == KILN_VALUE_STRING) {
        return kiln_c99_key_hash(v.ptr, v.len);
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
            size_t at = (size_t)kiln_c99_value_hash(collection->entries[i].key) & mask;
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
static double kiln_c99_collection_find(kiln_collection *collection, kiln_value key) {
    size_t index;
    if (collection->count == 0) {
        return -1.0;
    }
    if (collection->hash_table != NULL && collection->hash_capacity > 0) {
        size_t mask = collection->hash_capacity - 1;
        size_t at = (size_t)kiln_c99_value_hash(key) & mask;
        while (collection->hash_table[at] != 0) {
            uint32_t idx = collection->hash_table[at] - 1;
            if (!collection->entries[idx].dead &&
                kiln_c99_same_value_zero(collection->entries[idx].key, key)) {
                return (double)idx;
            }
            at = (at + 1) & mask;
        }
        return -1.0;
    }
    for (index = 0; index < collection->count; ++index) {
        if (collection->entries[index].dead) {
            continue;
        }
        if (kiln_c99_same_value_zero(collection->entries[index].key, key)) {
            return (double)index;
        }
    }
    return -1.0;
}

static int kiln_c99_collection_put(kiln_value target, kiln_value key, kiln_value value) {
    kiln_collection *collection = kiln_c99_collection_of(target);
    double at = kiln_c99_collection_find(collection, key);
    int status;
    size_t new_idx;
    if (at >= 0.0) {
        /* Re-setting a key keeps its position: insertion order is the order of *first*
         * appearance, which is what iteration reports. */
        collection->entries[(size_t)at].value = value;
        return KILN_C99_OK;
    }
    status = kiln_c99_reserve((void **)&collection->entries, &collection->capacity,
                              collection->count + 1, sizeof *collection->entries);
    if (status != KILN_C99_OK) {
        return status;
    }
    new_idx = collection->count;
    collection->entries[new_idx].key = key;
    collection->entries[new_idx].value = value;
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
