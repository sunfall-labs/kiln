
/* =========================================================================
 * Outlined instruction operation helpers (compact scalar emission)
 * ========================================================================= */

static int kiln_c99_val_new_array_literal(kiln_val *out, const kiln_val *elements,
                                           size_t count, int is_numeric) {
    kiln_value built;
    int status = kiln_c99_new_array_with_capacity(&built, count, is_numeric);
    if (status != KILN_C99_OK) return status;
    if (count != 0) {
        kiln_array *array = kiln_c99_array_of(built);
        memcpy(array->elements, elements, count * sizeof *elements);
        array->count = count;
    }
    *out = kiln_val_from_boundary(built);
    return KILN_C99_OK;
}

static int kiln_c99_val_new_object_literal(kiln_val *out, const kiln_val *pairs,
                                            size_t count) {
    kiln_value built;
    size_t pair;
    int status = count == 0
        ? kiln_c99_new_object(&built)
        : kiln_c99_new_object_with_capacity(&built, count);
    if (status != KILN_C99_OK) return status;
    for (pair = 0; pair < count; ++pair) {
        status = kiln_c99_define_computed(built, kiln_val_to_boundary(pairs[pair * 2]),
                                          kiln_val_to_boundary(pairs[pair * 2 + 1]));
        if (status != KILN_C99_OK) return status;
    }
    *out = kiln_val_from_boundary(built);
    return KILN_C99_OK;
}

static kiln_val kiln_c99_string_constant(void **cached, const char *bytes, size_t len) {
    if (KILN_UNLIKELY(*cached == NULL)) {
        kiln_value val = kiln_c99_copy_text(bytes, len);
        *cached = (void *)val.ptr;
    }
    return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, *cached);
}

/* What a method-call or property-read site remembers about a builtin receiver, beside the method:
 * see `kiln_c99_cached_method_matches`. */
static inline void kiln_c99_site_stamp_builtin(kiln_c99_site *site, uint32_t builtin) {
    site->cached_builtin = builtin;
    site->cached_epoch = kiln_c99_builtin_store_epoch;
    site->cached_generation = kiln_c99_realm_generation();
}

static kiln_value kiln_val_to_boundary(kiln_val v) {
    if (KILN_VAL_IS_NUMBER(v) || v == UINT64_C(0xFFF8000000000000)) {
        kiln_value out;
        out.tag = KILN_VALUE_NUMBER;
        out.flags = 0;
        out.len = 0;
        out.bits = (v == UINT64_C(0xFFF8000000000000)) ? KILN_QNAN_BASE : (uint64_t)v;
        return out;
    }
    if (KILN_VAL_IS_BUILTIN(v)) {
        return kiln_c99_builtin(KILN_VAL_BUILTIN_ID(v));
    }
    if (KILN_VAL_IS_METHOD(v)) {
        return kiln_c99_method(KILN_VAL_METHOD_ID(v));
    }
    switch (v) {
        case KILN_VAL_UNDEFINED:     return kiln_c99_undefined();
        case KILN_VAL_NULL:          return kiln_c99_null();
        case KILN_VAL_FALSE:         return kiln_c99_boolean(0);
        case KILN_VAL_TRUE:          return kiln_c99_boolean(1);
        case KILN_VAL_HOLE:          return kiln_c99_hole();
        case KILN_VAL_UNINITIALIZED: return kiln_c99_uninitialized();
        case KILN_VAL_RETURN_SIGNAL: return kiln_c99_return_signal();
        default: {
            uint64_t tag = v & KILN_TAG_MASK;
            void *ptr = KILN_VAL_PTR(v);
            if (tag == KILN_TAG_STRING_PREFIX) {
                const kiln_string *s = (const kiln_string *)ptr;
                if (s == NULL) {
                    return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)&kiln_c99_empty_string, 0);
                }
                kiln_value result = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)s, s->unit_len);
                if (!kiln_string_is_sliced(s) && s->data == (const char *)(s + 1)) result.flags = KILN_VALUE_FLAG_OWNED_STRING;
                return result;
            }
            if (tag == KILN_TAG_CLOSURE_PREFIX) {
                return kiln_c99_value(KILN_VALUE_FUNCTION, (uint64_t)(uintptr_t)ptr, NULL, 0);
            }
            if (tag == KILN_TAG_SYMBOL_PREFIX) {
                return kiln_c99_value(KILN_VALUE_SYMBOL, (uint64_t)(uintptr_t)ptr, NULL, 0);
            }
            if (tag == KILN_TAG_CELL_PREFIX) {
                return kiln_c99_value(KILN_VALUE_CELL, (uint64_t)(uintptr_t)ptr, NULL, 0);
            }
            if ((v & UINT64_C(0xFFFF800000000000)) == KILN_TAG_TEMPORAL_PREFIX) {
                return kiln_c99_value(KILN_VALUE_TEMPORAL, (uint64_t)(uintptr_t)ptr, NULL, 0);
            }
            if ((v & UINT64_C(0xFFFF800000000000)) == KILN_TAG_BIGINT_PREFIX) {
                return kiln_c99_value(KILN_VALUE_BIGINT, (uint64_t)(uintptr_t)ptr, NULL, 0);
            }
            uint32_t val_tag = (tag == KILN_TAG_BUFFER_PREFIX) ? KILN_VALUE_ARRAY : KILN_VALUE_OBJECT;
            uint32_t gc_kind = kiln_c99_gc_kind_of_ptr(ptr);
            switch (gc_kind) {
                case KILN_GC_OBJECT:            val_tag = KILN_VALUE_OBJECT; break;
                case KILN_GC_ARRAY:             val_tag = KILN_VALUE_ARRAY; break;
                case KILN_GC_TYPED_ARRAY:       val_tag = KILN_VALUE_TYPED_ARRAY; break;
                case KILN_GC_TYPED_BUFFER:      val_tag = KILN_VALUE_ARRAY_BUFFER; break;
                case KILN_GC_DATA_VIEW:         val_tag = KILN_VALUE_DATA_VIEW; break;
                case KILN_GC_GENERATOR:         val_tag = KILN_VALUE_GENERATOR; break;
                case KILN_GC_PROMISE:           val_tag = KILN_VALUE_PROMISE; break;
                case KILN_GC_SETTLER:           val_tag = KILN_VALUE_SETTLER; break;
                case KILN_GC_BOUND:             val_tag = KILN_VALUE_BOUND; break;
                case KILN_GC_COLLECTION:        val_tag = KILN_VALUE_COLLECTION; break;
                case KILN_GC_PROXY:             val_tag = KILN_VALUE_PROXY; break;
                case KILN_GC_DATE:              val_tag = KILN_VALUE_DATE; break;
                case KILN_GC_REGEXP:            val_tag = KILN_VALUE_REGEXP; break;
                case KILN_GC_SYMBOL:            val_tag = KILN_VALUE_SYMBOL; break;
                case KILN_GC_COMMONJS_REQUIRE:  val_tag = KILN_VALUE_COMMONJS_REQUIRE; break;
                case KILN_GC_CLOSURE:           val_tag = KILN_VALUE_FUNCTION; break;
                case KILN_GC_CELL:              val_tag = KILN_VALUE_CELL; break;
                default: break;
            }
            return kiln_c99_value(val_tag, (uint64_t)(uintptr_t)ptr, NULL, 0);
        }
    }
}

/* An ESM export publishes a binding's value, not its internal cell. Keep this distinct from the
 * general boundary conversion: runtime seams intentionally use KILN_VALUE_CELL to preserve a
 * shared mutable binding across generated translation units. */
static kiln_value kiln_val_binding_to_boundary(kiln_val v) {
    if ((v & KILN_TAG_MASK) == KILN_TAG_CELL_PREFIX) {
        v = *(const kiln_val *)KILN_VAL_PTR(v);
    }
    return kiln_val_to_boundary(v);
}

/* `Promise.resolve(v)` for the builtin constructor: a promise is answered as itself, anything
 * else gets a promise already fulfilled with it. The dispatch case does the same through two
 * boundary conversions and the method switch; the emitted C calls this behind the builtin
 * store epoch, so a program that reassigns `Promise.resolve` takes the general path. */
static int kiln_c99_val_promise_resolve(kiln_val value, kiln_val *out) {
    kiln_value promise;
    int status;
    if ((KILN_VAL_IS_OBJECT(value) || (value & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) &&
        kiln_c99_gc_kind_of_ptr(KILN_VAL_PTR(value)) == KILN_GC_PROMISE) {
        *out = value;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_promise(&promise);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_settle(promise, kiln_val_to_boundary(value), 0);
    if (status != KILN_C99_OK) return status;
    *out = kiln_val_from_boundary(promise);
    return KILN_C99_OK;
}

static size_t kiln_c99_val_string_length(kiln_val v) {
    if (KILN_LIKELY(KILN_VAL_IS_STRING(v))) {
        const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(v);
        if (s != NULL) return s->unit_len;
    }
    return kiln_c99_string_length(kiln_val_to_boundary(v));
}

static int kiln_c99_val_array_push_fast(kiln_val receiver, kiln_val arg, kiln_val *out) {
    if (KILN_LIKELY((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {
        void *ptr = KILN_VAL_PTR(receiver);
        if (KILN_LIKELY(kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY)) {
            kiln_array *arr = (kiln_array *)ptr;
            if (KILN_LIKELY(!arr->frozen && !arr->not_extensible)) {
                if (arr->is_numeric && !KILN_VAL_IS_NUMBER(arg)) {
                    arr->is_numeric = 0;
                }
                if (KILN_LIKELY(arr->count < arr->capacity)) {
                    arr->elements[arr->count++] = arg;
                    *out = kiln_c99_val_from_double((double)arr->count);
                    return KILN_C99_OK;
                }
                int status = kiln_c99_array_reserve(arr, arr->count + 1);
                if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
                arr->elements[arr->count++] = arg;
                *out = kiln_c99_val_from_double((double)arr->count);
                return KILN_C99_OK;
            }
        }
    }
    return -1;
}

static int kiln_c99_val_regexp_test_fast(kiln_val receiver, kiln_val arg, kiln_val *out) {
    if (KILN_LIKELY(KILN_VAL_IS_OBJECT(receiver) || ((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
        void *ptr = KILN_VAL_PTR(receiver);
        if (KILN_LIKELY(kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_REGEXP)) {
            kiln_regexp *regexp = (kiln_regexp *)ptr;
            if (KILN_LIKELY(regexp->matcher_ascii != NULL && KILN_VAL_IS_STRING(arg))) {
                const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(arg);
                if (str != NULL && !kiln_string_is_utf16(str)) {
                    kiln_value sval = kiln_val_to_boundary(arg);
                    int is_ascii;
                    if (KILN_LIKELY(regexp->cached_subject_ptr == sval.ptr && regexp->cached_subject_len == sval.len)) {
                        is_ascii = regexp->cached_is_ascii;
                    } else {
                        is_ascii = kiln_c99_string_is_ascii(sval);
                        regexp->cached_subject_ptr = sval.ptr;
                        regexp->cached_subject_len = sval.len;
                        regexp->cached_is_ascii = is_ascii;
                        regexp->cached_group_count = 0;
                    }
                    if (KILN_LIKELY(is_ascii)) {
                        size_t start = regexp->global || regexp->sticky
                            ? (regexp->last_index < 0.0 ? 0 : (size_t)regexp->last_index)
                            : 0;
                        if ((regexp->global || regexp->sticky) && start > sval.len) {
                            regexp->last_index = 0.0;
                            *out = KILN_VAL_FALSE;
                            return KILN_C99_OK;
                        }
                        int64_t ranges[64];
                        size_t groups = 0;
                        int matched = 0;
                        int status = regexp->matcher_ascii(kiln_string_latin1(str), sval.len, start,
                                                           ranges, sizeof ranges / sizeof *ranges, &groups, &matched);
                        if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
                        if (regexp->global || regexp->sticky) {
                            regexp->last_index = matched ? (double)ranges[1] : 0.0;
                        }
                        *out = matched ? KILN_VAL_TRUE : KILN_VAL_FALSE;
                        return KILN_C99_OK;
                    }
                }
            }
        }
    }
    return -1;
}

static int kiln_c99_val_regexp_exec_fast(kiln_val receiver, kiln_val arg, kiln_val *out) {
    if (KILN_LIKELY(KILN_VAL_IS_OBJECT(receiver) || ((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
        void *ptr = KILN_VAL_PTR(receiver);
        if (KILN_LIKELY(kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_REGEXP)) {
            kiln_regexp *regexp = (kiln_regexp *)ptr;
            if (KILN_LIKELY(regexp->matcher_ascii != NULL && KILN_VAL_IS_STRING(arg))) {
                const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(arg);
                if (str != NULL && !kiln_string_is_utf16(str)) {
                    kiln_value sval = kiln_val_to_boundary(arg);
                    int is_ascii;
                    if (KILN_LIKELY(regexp->cached_subject_ptr == sval.ptr && regexp->cached_subject_len == sval.len)) {
                        is_ascii = regexp->cached_is_ascii;
                    } else {
                        is_ascii = kiln_c99_string_is_ascii(sval);
                        regexp->cached_subject_ptr = sval.ptr;
                        regexp->cached_subject_len = sval.len;
                        regexp->cached_is_ascii = is_ascii;
                        regexp->cached_group_count = 0;
                    }
                    if (KILN_LIKELY(is_ascii)) {
                        size_t start = regexp->global || regexp->sticky
                            ? (regexp->last_index < 0.0 ? 0 : (size_t)regexp->last_index)
                            : 0;
                        if ((regexp->global || regexp->sticky) && start > sval.len) {
                            regexp->last_index = 0.0;
                            *out = KILN_VAL_NULL;
                            return KILN_C99_OK;
                        }
                        int64_t stack_ranges[64];
                        int64_t *ranges = stack_ranges;
                        size_t ranges_capacity = sizeof(stack_ranges) / sizeof(stack_ranges[0]);
                        size_t groups = 0;
                        int matched = 0;
                        int status = regexp->matcher_ascii(kiln_string_latin1(str), sval.len, start,
                                                           ranges, ranges_capacity, &groups, &matched);
                        if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
                        if (matched && groups > ranges_capacity / 2) {
                            ranges = (int64_t *)malloc(groups * 2 * sizeof(int64_t));
                            if (ranges == NULL) return KILN_C99_OUT_OF_MEMORY;
                            ranges_capacity = groups * 2;
                            status = regexp->matcher_ascii(kiln_string_latin1(str), sval.len, start,
                                                           ranges, ranges_capacity, &groups, &matched);
                            if (KILN_UNLIKELY(status != KILN_C99_OK)) {
                                free(ranges);
                                return status;
                            }
                        }
                        if (regexp->global || regexp->sticky) {
                            regexp->last_index = matched ? (double)ranges[1] : 0.0;
                        }
                        if (!matched) {
                            if (ranges != stack_ranges) free(ranges);
                            *out = KILN_VAL_NULL;
                            return KILN_C99_OK;
                        }
                        kiln_value built;
                        status = kiln_c99_new_array(&built);
                        if (KILN_UNLIKELY(status != KILN_C99_OK)) {
                            if (ranges != stack_ranges) free(ranges);
                            return status;
                        }
                        kiln_array *arr = kiln_c99_array_of(built);
                        if (groups <= sizeof(arr->inline_elements) / sizeof(arr->inline_elements[0])) {
                            arr->elements = arr->inline_elements;
                            arr->capacity = sizeof(arr->inline_elements) / sizeof(arr->inline_elements[0]);
                        } else {
                            status = kiln_c99_array_reserve(arr, groups);
                            if (KILN_UNLIKELY(status != KILN_C99_OK)) {
                                if (ranges != stack_ranges) free(ranges);
                                return status;
                            }
                        }
                        int can_reuse = (regexp->cached_group_count == groups && groups <= 32);
                        if (can_reuse) {
                            for (size_t g = 0; g < groups; ++g) {
                                if (regexp->cached_ranges[g * 2] != ranges[g * 2] ||
                                    regexp->cached_ranges[g * 2 + 1] != ranges[g * 2 + 1]) {
                                    can_reuse = 0;
                                    break;
                                }
                            }
                        }
                        if (can_reuse) {
                            memcpy(arr->elements, regexp->cached_elements, groups * sizeof(kiln_val));
                            arr->count = groups;
                        } else {
                            for (size_t g = 0; g < groups; ++g) {
                                int64_t from = ranges[g * 2];
                                int64_t to = ranges[g * 2 + 1];
                                kiln_value val;
                                if (from < 0) {
                                    val = kiln_c99_undefined();
                                } else {
                                    val = (to >= from && (size_t)to <= str->unit_len)
                                              ? kiln_c99_copy_ascii(
                                                    (const char *)kiln_string_latin1(str) + from,
                                                    (size_t)(to - from))
                                              : kiln_c99_undefined();
                                }
                                kiln_val bval = kiln_val_from_boundary(val);
                                arr->elements[g] = bval;
                                arr->count = g + 1;
                                if (g < 32) {
                                    regexp->cached_elements[g] = bval;
                                    regexp->cached_ranges[g * 2] = from;
                                    regexp->cached_ranges[g * 2 + 1] = to;
                                }
                            }
                            arr->count = groups;
                            if (groups <= 32) {
                                regexp->cached_group_count = groups;
                            } else {
                                regexp->cached_group_count = 0;
                            }
                        }
                        arr->match_index = ranges[0];
                        arr->match_input = sval;
                        {
                            int gstatus = kiln_c99_match_groups(regexp, arr, groups);
                            if (gstatus != KILN_C99_OK) {
                                if (ranges != stack_ranges) free(ranges);
                                return gstatus;
                            }
                        }
                        if (ranges != stack_ranges) free(ranges);
                        *out = kiln_val_from_boundary(built);
                        return KILN_C99_OK;
                    }
                }
            }
        }
    }
    return -1;
}

static kiln_collection *kiln_c99_collection_alloc_fast(int is_map, int is_weak) {
    kiln_collection *collection =
        (kiln_collection *)kiln_c99_gc_alloc_hot(KILN_GC_COLLECTION, sizeof *collection, 0);
    if (KILN_UNLIKELY(collection == NULL)) return NULL;
    collection->entries = NULL;
    collection->count = 0;
    collection->live = 0;
    collection->capacity = 0;
    collection->hash_table = NULL;
    collection->hash_capacity = 0;
    collection->is_map = is_map;
    collection->is_weak = is_weak;
    collection->properties = kiln_c99_undefined();
    return collection;
}

static inline uint64_t kiln_c99_val_collection_hash(kiln_val key) {
    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(key))) {
        uint64_t bits = (uint64_t)key;
        double d;
        memcpy(&d, &bits, sizeof(d));
        if (KILN_UNLIKELY(isnan(d))) return UINT64_C(0x7ff8000000000000);
        if (d == 0.0) bits = 0;
        bits ^= bits >> 32;
        bits *= UINT64_C(0xff51afd7ed558ccd);
        return bits ^ (bits >> 29);
    }
    if (KILN_VAL_IS_OBJECT(key)) {
        uint64_t h = ((uint64_t)KILN_VALUE_OBJECT << 32) ^ (uint64_t)(uintptr_t)KILN_VAL_PTR(key);
        h ^= h >> 29;
        h *= UINT64_C(0xff51afd7ed558ccd);
        return h ^ (h >> 32);
    }
    if (KILN_VAL_IS_STRING(key)) {
        return kiln_c99_collection_string_hash((const kiln_string *)KILN_VAL_PTR(key));
    }
    return kiln_c99_value_hash(kiln_val_to_boundary(key));
}

static inline int kiln_c99_val_collection_eq(kiln_value a, kiln_val b) {
    if (KILN_LIKELY(a.tag == KILN_VALUE_NUMBER && KILN_VAL_IS_NUMBER(b))) {
        if (a.bits == (uint64_t)b) return 1;
        double da, db;
        memcpy(&da, &a.bits, sizeof(da));
        memcpy(&db, &b, sizeof(db));
        if (KILN_UNLIKELY(isnan(da) && isnan(db))) return 1;
        return da == db;
    }
    if (a.tag == KILN_VALUE_OBJECT && KILN_VAL_IS_OBJECT(b)) {
        return a.bits == (uint64_t)(uintptr_t)KILN_VAL_PTR(b);
    }
    if (a.tag == KILN_VALUE_STRING && KILN_VAL_IS_STRING(b)) {
        const kiln_string *sa = (const kiln_string *)a.ptr;
        const kiln_string *sb = (const kiln_string *)KILN_VAL_PTR(b);
        if (sa == sb) return 1;
        if (sa == NULL || sb == NULL || sa->unit_len != sb->unit_len) return 0;
        return kiln_string_equal(sa, sb);
    }
    if (a.tag == KILN_VALUE_BOOLEAN && KILN_VAL_IS_BOOLEAN(b)) {
        return (a.bits != 0) == (b == KILN_VAL_TRUE);
    }
    if (a.tag == KILN_VALUE_UNDEFINED && b == KILN_VAL_UNDEFINED) return 1;
    if (a.tag == KILN_VALUE_NULL && b == KILN_VAL_NULL) return 1;
    return kiln_c99_same_value_zero(a, kiln_val_to_boundary(b));
}

static inline double kiln_c99_val_collection_find_fast(kiln_collection *collection, kiln_val key, uint64_t hash) {
    if (collection->count == 0) return -1.0;
    if (collection->hash_table != NULL && collection->hash_capacity > 0) {
        size_t mask = collection->hash_capacity - 1;
        size_t at = (size_t)hash & mask;
        while (collection->hash_table[at] != 0) {
            uint32_t idx = collection->hash_table[at] - 1;
            if (!collection->entries[idx].dead &&
                kiln_c99_val_collection_eq(collection->entries[idx].key, key)) {
                return (double)idx;
            }
            at = (at + 1) & mask;
        }
        return -1.0;
    }
    for (size_t index = 0; index < collection->count; ++index) {
        if (collection->entries[index].dead) continue;
        if (kiln_c99_val_collection_eq(collection->entries[index].key, key)) {
            return (double)index;
        }
    }
    return -1.0;
}

static int kiln_c99_collection_put_val_fast(kiln_collection *collection, kiln_val key, kiln_val value) {
    uint64_t hash = kiln_c99_val_collection_hash(key);
    size_t empty_slot = (size_t)-1;
    if (collection->count > 0 && collection->hash_table != NULL && collection->hash_capacity > 0) {
        size_t mask = collection->hash_capacity - 1;
        size_t at = (size_t)hash & mask;
        while (collection->hash_table[at] != 0) {
            uint32_t idx = collection->hash_table[at] - 1;
            if (!collection->entries[idx].dead &&
                kiln_c99_val_collection_eq(collection->entries[idx].key, key)) {
                collection->entries[idx].value = kiln_val_to_boundary(value);
                return KILN_C99_OK;
            }
            at = (at + 1) & mask;
        }
        empty_slot = at;
    } else if (collection->count > 0 && collection->hash_table == NULL) {
        for (size_t index = 0; index < collection->count; ++index) {
            if (collection->entries[index].dead) continue;
            if (kiln_c99_val_collection_eq(collection->entries[index].key, key)) {
                collection->entries[index].value = kiln_val_to_boundary(value);
                return KILN_C99_OK;
            }
        }
    }
    int status = kiln_c99_reserve((void **)&collection->entries, &collection->capacity,
                                  collection->count + 1, sizeof *collection->entries);
    if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
    size_t new_idx = collection->count;
    collection->entries[new_idx].key = kiln_val_to_boundary(key);
    collection->entries[new_idx].value = kiln_val_to_boundary(value);
    collection->entries[new_idx].dead = 0;
    collection->count += 1;
    collection->live += 1;

    if (collection->hash_table == NULL) {
        if (collection->count >= 8) {
            status = kiln_c99_collection_rehash(collection, 16);
            if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
        }
    } else {
        if ((collection->count + 1) * 2 > collection->hash_capacity) {
            status = kiln_c99_collection_rehash(collection, collection->hash_capacity * 2);
            if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
        } else if (empty_slot != (size_t)-1) {
            collection->hash_table[empty_slot] = (uint32_t)(new_idx + 1);
        } else {
            size_t mask = collection->hash_capacity - 1;
            size_t h_at = (size_t)hash & mask;
            while (collection->hash_table[h_at] != 0) {
                h_at = (h_at + 1) & mask;
            }
            collection->hash_table[h_at] = (uint32_t)(new_idx + 1);
        }
    }
    return KILN_C99_OK;
}

static kiln_val kiln_c99_collection_get_val_fast(kiln_collection *collection, kiln_val key) {
    uint64_t hash = kiln_c99_val_collection_hash(key);
    double at = kiln_c99_val_collection_find_fast(collection, key, hash);
    if (at < 0.0) return KILN_VAL_UNDEFINED;
    kiln_value v = collection->entries[(size_t)at].value;
    if (KILN_LIKELY(v.tag == KILN_VALUE_NUMBER)) {
        return (kiln_val)v.bits;
    }
    return kiln_val_from_boundary(v);
}

static kiln_val kiln_c99_collection_has_val_fast(kiln_collection *collection, kiln_val key) {
    uint64_t hash = kiln_c99_val_collection_hash(key);
    double at = kiln_c99_val_collection_find_fast(collection, key, hash);
    return at < 0.0 ? KILN_VAL_FALSE : KILN_VAL_TRUE;
}

static kiln_val kiln_c99_collection_delete_val_fast(kiln_collection *collection, kiln_val key) {
    uint64_t hash = kiln_c99_val_collection_hash(key);
    double at = kiln_c99_val_collection_find_fast(collection, key, hash);
    if (at < 0.0) return KILN_VAL_FALSE;
    collection->entries[(size_t)at].dead = 1;
    collection->live -= 1;
    kiln_c99_collection_hash_remove(collection, (size_t)at, kiln_val_to_boundary(key));
    return KILN_VAL_TRUE;
}

static int kiln_c99_val_json_stringify_fast(kiln_host *host, kiln_val val, kiln_val *out, int depth, kiln_value *thrown) {
    /* Most documents a program stringifies in a loop are a few hundred bytes: built on the
     * stack, they cost the one copy into their string and no allocator call at all. A larger
     * one moves into memory of the builder's own at the first append past the array. */
    char stack_bytes[1024];
    kiln_text text = {stack_bytes, 0, sizeof stack_bytes, 1};
    int wrote = 0;
    int status;
    status = kiln_c99_json_value(host, &text, kiln_val_to_boundary(val), "", 0, "", 0, &wrote, depth, NULL, thrown);
    if (KILN_UNLIKELY(status != KILN_C99_OK)) {
        if (!text.borrowed) free(text.bytes);
        return status;
    }
    if (wrote) {
        *out = kiln_c99_text_val(&text);
    } else {
        if (!text.borrowed) free(text.bytes);
        *out = KILN_VAL_UNDEFINED;
    }
    return KILN_C99_OK;
}

static int kiln_c99_val_json_parse_fast(kiln_host *host, kiln_val val, kiln_val *out, int depth, kiln_value *thrown) {
    (void)host;
    (void)depth;
    if (!KILN_VAL_IS_STRING(val)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Unexpected token in JSON");
    }
    const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(val);
    kiln_val parsed;
    int status = kiln_c99_json_document(s, &parsed);
    if (status == KILN_C99_CONTROL_ERROR) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Unexpected token in JSON");
    }
    if (status != KILN_C99_OK) return status;
    *out = parsed;
    return KILN_C99_OK;
}

typedef struct {
    kiln_host *host;
    kiln_val comparator;
    int depth;
    kiln_value *thrown;
} kiln_c99_val_sort_context;

static int kiln_c99_val_sort_compare(kiln_c99_val_sort_context *ctx, kiln_val a, kiln_val b, double *out_order) {
    if (KILN_VAL_IS_CLOSURE(ctx->comparator)) {
        kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(ctx->comparator);
        kiln_val args[2] = { a, b };
        kiln_val prod = KILN_VAL_UNDEFINED;
        int status;
        if (!plain->publishes_frame) {
            kiln_call_context ctx_call = { ctx->host, ctx->thrown, NULL, ctx->depth + 1 };
            kiln_val recv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED;
            status = plain->body(&ctx_call, &prod, plain->upvalues, &recv, args, (size_t)2);
        } else {
            status = kiln_c99_invoke_closure(ctx->host, &prod, plain, KILN_VAL_UNDEFINED, args, (size_t)2, ctx->depth, ctx->thrown, NULL, 0);
        }
        if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
        if (KILN_LIKELY(KILN_VAL_IS_NUMBER(prod))) {
            double d; memcpy(&d, &prod, sizeof(d));
            *out_order = isnan(d) ? 0.0 : d;
        } else {
            *out_order = 0.0;
        }
        return KILN_C99_OK;
    }
    kiln_c99_sort_context legacy_ctx = { ctx->host, kiln_val_to_boundary(ctx->comparator), ctx->depth, ctx->thrown };
    return kiln_c99_sort_compare_val(&legacy_ctx, a, b, out_order);
}

KILN_DEF_TIMSORT(val_fast, kiln_val, kiln_c99_val_sort_context, kiln_c99_val_sort_compare)

static int kiln_c99_val_array_sort_fast(kiln_host *host, kiln_array *array, kiln_val comparator, int depth, kiln_value *thrown) {
    if (comparator != KILN_VAL_UNDEFINED && !KILN_VAL_IS_CLOSURE(comparator)) {
        kiln_value bcomp = kiln_val_to_boundary(comparator);
        if (!kiln_c99_is_callable(bcomp)) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, "The comparison function must be either a function or undefined");
        }
    }
    size_t live = 0;
    for (size_t index = 0; index < array->count; ++index) {
        if (array->elements[index] != KILN_VAL_HOLE && array->elements[index] != KILN_VAL_UNDEFINED) {
            array->elements[live++] = array->elements[index];
        }
    }
    for (size_t index = live; index < array->count; ++index) {
        array->elements[index] = KILN_VAL_UNDEFINED;
    }
    if (live > 1) {
        kiln_c99_val_sort_context ctx;
        ctx.host = host;
        ctx.comparator = comparator;
        ctx.depth = depth;
        ctx.thrown = thrown;
        int status = kiln_c99_timsort_val_fast(&ctx, array->elements, live);
        if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
    }
    return KILN_C99_OK;
}

static int kiln_c99_val_iterator_next_fast(kiln_host *host, kiln_val receiver, kiln_val *out, int depth, kiln_value *thrown) {
    (void)host;
    (void)depth;
    (void)thrown;
    if (!KILN_VAL_IS_OBJECT(receiver)) return -1;
    kiln_value v_iter = kiln_val_to_boundary(receiver);
    kiln_property *p_iterated = kiln_c99_own_private(v_iter, "@@iterated", 10);
    kiln_property *p_index = kiln_c99_own_private(v_iter, "@@index", 7);
    if (p_iterated == NULL || p_index == NULL || p_index->value.tag != KILN_VALUE_NUMBER) return -1;
    double idx = kiln_c99_as_number(p_index->value);
    if (p_iterated->value.tag == KILN_VALUE_ARRAY) {
        kiln_array *arr = kiln_c99_array_of(p_iterated->value);
        size_t i = (size_t)idx;
        kiln_value item;
        int status = kiln_c99_new_object(&item);
        if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
        if (i < arr->count) {
            p_index->value = kiln_c99_number((double)(i + 1));
            kiln_c99_put(NULL, item, "value", 5, kiln_val_to_boundary(arr->elements[i]), 0, NULL);
            kiln_c99_put(NULL, item, "done", 4, kiln_c99_boolean(0), 0, NULL);
        } else {
            p_index->value = kiln_c99_number((double)arr->count);
            kiln_c99_put(NULL, item, "value", 5, kiln_c99_undefined(), 0, NULL);
            kiln_c99_put(NULL, item, "done", 4, kiln_c99_boolean(1), 0, NULL);
        }
        *out = kiln_val_from_boundary(item);
        return KILN_C99_OK;
    }
    if (p_iterated->value.tag == KILN_VALUE_COLLECTION) {
        kiln_collection *col = kiln_c99_collection_of(p_iterated->value);
        kiln_property *p_part = kiln_c99_own_private(v_iter, "@@part", 6);
        double which = p_part ? kiln_c99_as_number(p_part->value) : 0.0;
        size_t pos = (size_t)idx;
        while (pos < col->count && col->entries[pos].dead) {
            pos++;
        }
        kiln_value item;
        int status = kiln_c99_new_object(&item);
        if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
        if (pos < col->count) {
            p_index->value = kiln_c99_number((double)(pos + 1));
            kiln_value val;
            if (which == 2.0) {
                kiln_value pair;
                status = kiln_c99_new_array(&pair);
                if (status != KILN_C99_OK) return status;
                status = kiln_c99_array_push(pair, col->entries[pos].key);
                if (status != KILN_C99_OK) return status;
                status = kiln_c99_array_push(pair, col->entries[pos].value);
                if (status != KILN_C99_OK) return status;
                val = pair;
            } else if (which == 1.0) {
                val = col->entries[pos].key;
            } else {
                val = col->entries[pos].value;
            }
            kiln_c99_put(NULL, item, "value", 5, val, 0, NULL);
            kiln_c99_put(NULL, item, "done", 4, kiln_c99_boolean(0), 0, NULL);
        } else {
            p_index->value = kiln_c99_number((double)col->count);
            kiln_c99_put(NULL, item, "value", 5, kiln_c99_undefined(), 0, NULL);
            kiln_c99_put(NULL, item, "done", 4, kiln_c99_boolean(1), 0, NULL);
        }
        *out = kiln_val_from_boundary(item);
        return KILN_C99_OK;
    }
    return -1;
}

static int kiln_c99_val_same_value_zero(kiln_val a, kiln_val b) {
    if (a == b) {
        return 1;
    }
    if (KILN_VAL_IS_NUMBER(a) && KILN_VAL_IS_NUMBER(b)) {
        double da, db;
        memcpy(&da, &a, sizeof(da));
        memcpy(&db, &b, sizeof(db));
        if (isnan(da) && isnan(db)) return 1;
        return da == db;
    }
    return kiln_c99_same_value_zero(kiln_val_to_boundary(a), kiln_val_to_boundary(b));
}

static int kiln_c99_val_compare(uint16_t operation, kiln_val left, kiln_val right, int *result) {
    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(left) && KILN_VAL_IS_NUMBER(right))) {
        double a, b;
        memcpy(&a, &left, sizeof(a));
        memcpy(&b, &right, sizeof(b));
        switch (operation) {
            case 7:  *result = a == b; return 1;
            case 8:  *result = a != b; return 1;
            case 9:  *result = a < b;  return 1;
            case 10: *result = a <= b; return 1;
            case 11: *result = a > b;  return 1;
            case 12: *result = a >= b; return 1;
            case 15: *result = a == b; return 1;
            case 16: *result = a != b; return 1;
            default: return 0;
        }
    }
    if (operation == 7) { /* StrictEqual */
        if (left == right) { *result = 1; return 1; }
        if (KILN_VAL_IS_NUMBER(left) || KILN_VAL_IS_NUMBER(right)) { *result = 0; return 1; }
        if (KILN_VAL_IS_STRING(left) && KILN_VAL_IS_STRING(right)) {
            const kiln_string *s1 = (const kiln_string *)KILN_VAL_PTR(left);
            const kiln_string *s2 = (const kiln_string *)KILN_VAL_PTR(right);
            if (s1->unit_len != s2->unit_len) { *result = 0; return 1; }
            *result = kiln_string_equal(s1, s2);
            return 1;
        }
    }
    if (operation == 15) { /* Equal */
        if (left == right) { *result = 1; return 1; }
        if (KILN_VAL_IS_STRING(left) && KILN_VAL_IS_STRING(right)) {
            const kiln_string *s1 = (const kiln_string *)KILN_VAL_PTR(left);
            const kiln_string *s2 = (const kiln_string *)KILN_VAL_PTR(right);
            if (s1->unit_len != s2->unit_len) { *result = 0; return 1; }
            *result = kiln_string_equal(s1, s2);
            return 1;
        }
    }
    if (operation == 8) { /* StrictNotEqual */
        if (left == right) { *result = 0; return 1; }
        if (KILN_VAL_IS_NUMBER(left) || KILN_VAL_IS_NUMBER(right)) { *result = 1; return 1; }
        if (KILN_VAL_IS_STRING(left) && KILN_VAL_IS_STRING(right)) {
            const kiln_string *s1 = (const kiln_string *)KILN_VAL_PTR(left);
            const kiln_string *s2 = (const kiln_string *)KILN_VAL_PTR(right);
            if (s1->unit_len != s2->unit_len) { *result = 1; return 1; }
            *result = !kiln_string_equal(s1, s2);
            return 1;
        }
    }
    if (operation == 16) { /* NotEqual */
        if (left == right) { *result = 0; return 1; }
        if (KILN_VAL_IS_STRING(left) && KILN_VAL_IS_STRING(right)) {
            const kiln_string *s1 = (const kiln_string *)KILN_VAL_PTR(left);
            const kiln_string *s2 = (const kiln_string *)KILN_VAL_PTR(right);
            if (s1->unit_len != s2->unit_len) { *result = 1; return 1; }
            *result = !kiln_string_equal(s1, s2);
            return 1;
        }
    }
    return 0;
}

static kiln_val kiln_c99_val_binary_fast(uint16_t operation, kiln_val left, kiln_val right) {
    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(left) && KILN_VAL_IS_NUMBER(right))) {
        double a, b;
        memcpy(&a, &left, sizeof(a));
        memcpy(&b, &right, sizeof(b));
        switch (operation) {
            case 1:  return kiln_c99_val_from_double(a + b);
            case 2:  return kiln_c99_val_from_double(a - b);
            case 3:  return kiln_c99_val_from_double(a * b);
            case 4:  return kiln_c99_val_from_double(a / b);
            case 5:  return kiln_c99_val_from_double(fmod(a, b));
            case 6:  return kiln_c99_val_from_double(pow(a, b));
            case 7:  return (a == b) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
            case 8:  return (a != b) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
            case 9:  return (a < b)  ? KILN_VAL_TRUE : KILN_VAL_FALSE;
            case 10: return (a <= b) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
            case 11: return (a > b)  ? KILN_VAL_TRUE : KILN_VAL_FALSE;
            case 12: return (a >= b) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
            case 15: return (a == b) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
            case 16: return (a != b) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
            case 17: return kiln_c99_val_from_double(kiln_c99_signed32(
                kiln_c99_to_uint32(a) & kiln_c99_to_uint32(b)));
            case 18: return kiln_c99_val_from_double(kiln_c99_signed32(
                kiln_c99_to_uint32(a) | kiln_c99_to_uint32(b)));
            case 19: return kiln_c99_val_from_double(kiln_c99_signed32(
                kiln_c99_to_uint32(a) ^ kiln_c99_to_uint32(b)));
            case 20: return kiln_c99_val_from_double(kiln_c99_signed32(
                kiln_c99_to_uint32(a) << (kiln_c99_to_uint32(b) & UINT32_C(31))));
            case 21: return kiln_c99_val_from_double(kiln_c99_signed32(kiln_c99_shift_right(
                kiln_c99_to_uint32(a), kiln_c99_to_uint32(b))));
            case 22: return kiln_c99_val_from_double((double)(
                kiln_c99_to_uint32(a) >> (kiln_c99_to_uint32(b) & UINT32_C(31))));
            default: break;
        }
    }
    if ((operation == 7 || operation == 15) && left == right) return KILN_VAL_TRUE;
    if ((operation == 8 || operation == 16) && left == right) return KILN_VAL_FALSE;
    if (operation == 1 && KILN_VAL_IS_STRING(left) && KILN_VAL_IS_STRING(right) &&
        !kiln_string_is_utf16((const kiln_string *)KILN_VAL_PTR(left)) &&
        !kiln_string_is_utf16((const kiln_string *)KILN_VAL_PTR(right))) {
        /* Latin-1 on both sides only: the copy below is bytes, and a UTF-16 operand's units are
         * two each. Anything else takes the general path, which widens. */
        const kiln_string *s1 = (const kiln_string *)KILN_VAL_PTR(left);
        const kiln_string *s2 = (const kiln_string *)KILN_VAL_PTR(right);
        if (s1 != NULL && s2 != NULL) {
            if (s1->unit_len == 0) return right;
            if (s2->unit_len == 0) return left;
            if (!kiln_string_is_utf16(s1) && !kiln_string_is_utf16(s2)) {
                size_t total_bytes = s1->unit_len + s2->unit_len;
                char *buf = kiln_c99_gc_alloc_text(total_bytes);
                if (buf != NULL) {
                    memcpy(buf, s1->data, s1->unit_len);
                    memcpy(buf + s1->unit_len, s2->data, s2->unit_len);
                    kiln_string *s = ((kiln_string *)buf) - 1;
                    s->unit_len = (uint32_t)total_bytes;
                    s->flags = KILN_STRING_LATIN1;
                    s->data = buf;
                    return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, s);
                }
            }
        }
    }
    /* Equality is over code units, whichever way each side stores them: a `memcmp` of
     * `unit_len` bytes compared a Latin-1 "null" against the first four bytes of a UTF-16 one and
     * called them different, which is how every string `JSON.parse` read out of a document with
     * one non-Latin-1 character in it stopped being `===` to its literal (WPT url/, 2026-09-19). */
    if ((operation == 7 || operation == 15) && KILN_VAL_IS_STRING(left) && KILN_VAL_IS_STRING(right)) {
        if (left == right) return KILN_VAL_TRUE;
        const kiln_string *s1 = (const kiln_string *)KILN_VAL_PTR(left);
        const kiln_string *s2 = (const kiln_string *)KILN_VAL_PTR(right);
        if (s1 == NULL || s2 == NULL) return (s1 == s2) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
        if (s1->unit_len != s2->unit_len) return KILN_VAL_FALSE;
        if (s1->unit_len == 0) return KILN_VAL_TRUE;
        return kiln_string_equal(s1, s2) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
    }
    if ((operation == 8 || operation == 16) && KILN_VAL_IS_STRING(left) && KILN_VAL_IS_STRING(right)) {
        if (left == right) return KILN_VAL_FALSE;
        const kiln_string *s1 = (const kiln_string *)KILN_VAL_PTR(left);
        const kiln_string *s2 = (const kiln_string *)KILN_VAL_PTR(right);
        if (s1 == NULL || s2 == NULL) return (s1 != s2) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
        if (s1->unit_len != s2->unit_len) return KILN_VAL_TRUE;
        if (s1->unit_len == 0) return KILN_VAL_FALSE;
        return !kiln_string_equal(s1, s2) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
    }
    return kiln_val_from_boundary(kiln_c99_binary(operation, kiln_val_to_boundary(left), kiln_val_to_boundary(right)));
}

static kiln_val kiln_c99_val_binary(uint16_t operation, kiln_host *host, kiln_val left, kiln_val right, int *status, int depth, kiln_value *thrown) {
    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(left) && KILN_VAL_IS_NUMBER(right))) {
        *status = KILN_C99_OK;
        return kiln_c99_val_binary_fast(operation, left, right);
    }
    if ((operation == 1 || operation == 7 || operation == 8 || operation == 15 || operation == 16) &&
        KILN_VAL_IS_STRING(left) && KILN_VAL_IS_STRING(right)) {
        *status = KILN_C99_OK;
        return kiln_c99_val_binary_fast(operation, left, right);
    }
    {
        kiln_val immediate = KILN_VAL_EQUALITY_IMMEDIATE(operation, left, right);
        if (immediate != KILN_VAL_HOLE) {
            *status = KILN_C99_OK;
            return immediate;
        }
    }
    kiln_value l = kiln_val_to_boundary(left);
    kiln_value r = kiln_val_to_boundary(right);
    kiln_value ans;
    *status = kiln_c99_binary_checked(operation, host, l, r, &ans, depth, thrown);
    if (*status == KILN_C99_OK) return kiln_val_from_boundary(ans);
    return KILN_VAL_UNDEFINED;
}

static kiln_val kiln_c99_val_unary_fast(uint16_t operation, kiln_val value) {
    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(value))) {
        double a;
        memcpy(&a, &value, sizeof(a));
        switch (operation) {
            case 1: return kiln_c99_val_from_double(+a);
            case 2: return kiln_c99_val_from_double(-a);
            case 3: return (a == 0.0 || isnan(a)) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
            case 4: return kiln_val_from_boundary(kiln_c99_typeof(kiln_val_to_boundary(value)));
            case 5: return kiln_c99_val_from_double(kiln_c99_signed32(~kiln_c99_to_uint32(a)));
            case 6: return KILN_VAL_UNDEFINED;
            case 7: return value;
            case 8: return kiln_c99_val_from_double(a + 1.0);
            case 9: return kiln_c99_val_from_double(a - 1.0);
            default: break;
        }
    }
    return kiln_val_from_boundary(kiln_c99_unary(operation, kiln_val_to_boundary(value)));
}

static int kiln_c99_read_property(kiln_host *host, kiln_value target,
                                  const char *key, size_t key_len,
                                  int depth, kiln_value *thrown,
                                  kiln_c99_site *site, kiln_value *out) {
    if (site != NULL) {
        if (KILN_LIKELY(site->pic_count > 0 && target.tag == KILN_VALUE_OBJECT)) {
            const kiln_object *object = kiln_c99_object_of(target);
            size_t p;
            for (p = 0; p < site->pic_count; ++p) {
                if (object->shape == site->entries[p].shape &&
                    site->entries[p].slot < object->slot_capacity) {
                    *out = kiln_val_to_boundary(object->slots[site->entries[p].slot]);
                    return KILN_C99_OK;
                }
            }
        }
        if (KILN_LIKELY(site->has_cached_method && target.tag == site->cached_tag)) {
            if (target.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(target)->properties.tag == KILN_VALUE_UNDEFINED) {
                *out = kiln_c99_method(site->cached_method);
                return KILN_C99_OK;
            }
        }
    }
    if (KILN_UNLIKELY(key_len == 6 && key[0] == 'l' && memcmp(key, "length", 6) == 0)) {
        if (target.tag == KILN_VALUE_ARRAY) {
            *out = kiln_c99_number((double)kiln_c99_array_of(target)->count);
            return KILN_C99_OK;
        }
        if (target.tag == KILN_VALUE_STRING) {
            *out = kiln_c99_number((double)kiln_c99_string_length(target));
            return KILN_C99_OK;
        }
        if (target.tag == KILN_VALUE_TYPED_ARRAY) {
            *out = kiln_c99_number((double)kiln_c99_typed_array_of(target)->length);
            return KILN_C99_OK;
        }
    }
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    int status = kiln_c99_get_site(host, target, &view, NULL, out, depth, thrown, site);
    if (site != NULL && status == KILN_C99_OK && out->tag == KILN_VALUE_METHOD && !site->has_cached_method) {
        if (target.tag == KILN_VALUE_STRING ||
            target.tag == KILN_VALUE_NUMBER ||
            target.tag == KILN_VALUE_BOOLEAN ||
            (target.tag == KILN_VALUE_ARRAY && kiln_c99_array_of(target)->properties.tag == KILN_VALUE_UNDEFINED) ||
            target.tag == KILN_VALUE_TYPED_ARRAY ||
            target.tag == KILN_VALUE_COLLECTION ||
            target.tag == KILN_VALUE_BUILTIN) {
            site->cached_tag = (uint8_t)target.tag;
            site->cached_method = out->bits;
            site->has_cached_method = 1;
            if (target.tag == KILN_VALUE_BUILTIN) kiln_c99_site_stamp_builtin(site, (uint32_t)target.bits);
        }
    }
    return status;
}

static int kiln_c99_write_property(kiln_host *host, kiln_value target,
                                   const char *key, size_t key_len,
                                   kiln_value value, int strict,
                                   int depth, kiln_value *thrown,
                                   kiln_c99_site *site) {
    if (site != NULL) {
        if (KILN_LIKELY(site->pic_count > 0 && target.tag == KILN_VALUE_OBJECT)) {
            kiln_object *object = kiln_c99_object_of(target);
            size_t p;
            kiln_c99_prototype_touched(object);
            for (p = 0; p < site->pic_count; ++p) {
                if (object->shape == site->entries[p].shape) {
                    if (site->entries[p].transition_to == NULL) {
                        if (KILN_LIKELY(site->entries[p].slot < object->slot_capacity && !object->frozen)) {
                            if ((object->shape->double_slots_mask & (1u << site->entries[p].slot)) &&
                                value.tag != KILN_VALUE_NUMBER) {
                                kiln_c99_ensure_dictionary(object);
                                return kiln_c99_put_site(host, target, key, key_len, value, strict, depth, thrown, site);
                            }
                            object->slots[site->entries[p].slot] = kiln_val_from_boundary(value);
                            return KILN_C99_OK;
                        }
                    } else if (!object->not_extensible &&
                               site->entries[p].slot < object->slot_capacity &&
                               kiln_c99_transition_store_clear(site, &site->entries[p], object,
                                                               NULL, NULL)) {
                        const kiln_shape *next_shape = site->entries[p].transition_to;
                        if ((next_shape->double_slots_mask & (1u << site->entries[p].slot)) &&
                            value.tag != KILN_VALUE_NUMBER) {
                            /* Non-double written to double slot transition; fall through */
                        } else {
                            object->slots[site->entries[p].slot] = kiln_val_from_boundary(value);
                            object->shape = (kiln_shape *)site->entries[p].transition_to;
                            return KILN_C99_OK;
                        }
                    }
                    break;
                }
            }
        }
    }
    return kiln_c99_put_site(host, target, key, key_len, value, strict, depth, thrown, site);
}

#ifndef KILN_C99_CACHED_METHOD_MATCHES_DEFINED
#define KILN_C99_CACHED_METHOD_MATCHES_DEFINED
static inline int kiln_c99_cached_method_matches(const kiln_c99_site *site, kiln_val target) {
    if (site == NULL || !site->has_cached_method) return 0;
    uint8_t ctag = site->cached_tag;
    if (ctag == KILN_VALUE_COLLECTION && (KILN_VAL_IS_OBJECT(target) || ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
        void *ptr = KILN_VAL_PTR(target);
        return kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION;
    } else if (ctag == KILN_VALUE_STRING && KILN_VAL_IS_STRING(target)) {
        return 1;
    } else if (ctag == KILN_VALUE_NUMBER && KILN_VAL_IS_NUMBER(target)) {
        return 1;
    } else if (ctag == KILN_VALUE_BOOLEAN && KILN_VAL_IS_BOOLEAN(target)) {
        return 1;
    } else if (ctag == KILN_VALUE_BUILTIN) {
        /* `Promise.resolve`, `Object.keys`, `Math.max`: the same builtin at the same site answers
         * the same method until an own property is written onto some builtin or the realm
         * changes, and either moves a stamp the site compares. */
        return KILN_VAL_IS_BUILTIN(target) && KILN_VAL_BUILTIN_ID(target) == site->cached_builtin &&
               site->cached_epoch == kiln_c99_builtin_store_epoch &&
               site->cached_generation == kiln_c99_realm_generation();
    } else if (ctag == KILN_VALUE_ARRAY && ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {
        void *ptr = KILN_VAL_PTR(target);
        if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {
            const kiln_array *arr = (const kiln_array *)ptr;
            return arr->properties.tag == KILN_VALUE_UNDEFINED;
        }
    } else if (ctag == KILN_VALUE_TYPED_ARRAY && ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {
        void *ptr = KILN_VAL_PTR(target);
        return kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY;
    } else if (ctag == KILN_VALUE_REGEXP && (KILN_VAL_IS_OBJECT(target) || ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
        void *ptr = KILN_VAL_PTR(target);
        return kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_REGEXP;
    }
    return 0;
}
#endif

/* The negative read cache: see `kiln_c99_site_miss_holds` in the header for what an entry says
 * and why a hit is exact. This file learns entries and checks them under `KILN_IC_CHECK`. */
#ifdef KILN_IC_CHECK
/* The oracle for the negative cache: the slow path, run again, must find nothing. */
static void kiln_c99_site_miss_check(kiln_host *host, kiln_val target, const kiln_string *key,
                                     const kiln_string *canonical, int depth, kiln_value *thrown) {
    kiln_value slow = kiln_c99_undefined();
    int status = kiln_c99_get_key(host, kiln_val_to_boundary(target), key, canonical, &slow, depth,
                                  thrown);
    if (status != KILN_C99_OK || slow.tag != KILN_VALUE_UNDEFINED) {
        fputs("kiln: inline-cache oracle: a remembered property miss is present; a layout or a live prototype check is missing\n", stderr);
        __builtin_trap();
    }
}
#endif

/* Remember that the read of `key` on `target` just missed, when the miss is one a path of layouts
 * proves again (see the header). The slow path has already answered undefined; this only decides
 * whether that answer is a function of what an entry compares. `canonical` is the site's pointer
 * for the key; an entry with a dictionary level is checked by that pointer, so it is learned only
 * when the site has one and it is the realm's canonical pointer for the spelling. */
static void kiln_c99_site_learn_miss(kiln_c99_site *site, kiln_val target, const kiln_string *key,
                                     const kiln_string *canonical) {
    const kiln_object *object;
    const kiln_object *step;
    const kiln_string *ikey;
    const kiln_shape *levels[2] = {NULL, NULL};
    kiln_c99_pic_entry remembered;
    uint32_t depth = 0;
    uint32_t path;
    uint32_t p;
    int has_dictionary = 0;
    void *ptr = KILN_VAL_PTR(target);
    if (ptr == NULL || kiln_c99_gc_payload_kind(ptr) != KILN_GC_OBJECT) return;
    object = (const kiln_object *)ptr;
    /* Only shared shapes (ADR 0071): a realm-local shape is freed with its heap, and a later
     * realm's shape at the same address must never match an entry learned from it. A shared
     * shape is never freed and means the same layout in every realm. */
    if (object->shape == NULL || !object->shape->shared || object->properties != NULL ||
        object->elements != NULL ||
        object->error_brand || object->arguments_brand || object->module_namespace) {
        return;
    }
    /* Names the slow path answers from the receiver's kind, or with rules of their own. */
    if (kiln_c99_key_index_key(key) >= 0.0 || kiln_c99_key_is_reserved(key) ||
        kiln_c99_same_name_key(key, "__proto__") || kiln_c99_same_name_key(key, "constructor") ||
        kiln_c99_same_name_key(key, "name") || kiln_c99_same_name_key(key, "message") ||
        kiln_c99_same_name_key(key, "toString")) {
        return;
    }
    /* By bytes, not from the site: the site's pointer is only an optimisation, and one that is not
     * this realm's canonical pointer would find nothing and prove a false absence. */
    ikey = kiln_c99_interned_key(key);
    if (ikey == NULL) return;
    if (kiln_c99_shape_find_slot(object->shape, ikey, NULL)) return;
    step = object;
    while (step->prototype.tag == KILN_VALUE_OBJECT) {
        if (depth == 2) return;
        step = kiln_c99_object_of(step->prototype);
        if (step == NULL || kiln_c99_gc_payload_kind((void *)step) != KILN_GC_OBJECT ||
            step->module_namespace) {
            return;
        }
        if (step->shape != NULL) {
            if (!step->shape->shared || step->properties != NULL ||
                kiln_c99_shape_find_slot(step->shape, ikey, NULL)) {
                return;
            }
            levels[depth] = step->shape;
        } else {
            /* Bounded, since every hit scans it. */
            if (step->count > 32 || !kiln_c99_miss_list_lacks(step, ikey)) return;
            levels[depth] = KILN_C99_MISS_DICTIONARY;
            has_dictionary = 1;
        }
        depth += 1;
    }
    if (step->prototype.tag == KILN_VALUE_NULL) {
        path = depth | KILN_C99_MISS_ENDS_NULL;
    } else if (step->prototype.tag == KILN_VALUE_UNDEFINED) {
        /* Object.prototype: its method table is fixed, and a key it answers from there never
         * reaches here as a miss; its own-property store is scanned live on every hit, and must
         * lack the key now. */
        const kiln_object *store = kiln_c99_object_prototype_store;
        if (store != NULL && !kiln_c99_miss_object_lacks(store, ikey)) return;
        path = depth;
    } else {
        return;
    }
    /* Every hit compares by the site's pointer, so it must be the canonical one. */
    if (canonical != ikey) return;
    (void)has_dictionary;
    memset(&remembered, 0, sizeof remembered);
    remembered.shape = object->shape;
    remembered.slot = UINT32_MAX;
    remembered.holder_slot = path;
    remembered.via_shape = levels[0];
    remembered.holder_shape = levels[1];
    for (p = 0; p < site->pic_count; ++p) {
        if (site->entries[p].shape == object->shape) {
            site->entries[p] = remembered;
            return;
        }
    }
    if (site->pic_count < KILN_C99_PIC_SIZE) {
        site->entries[site->pic_count++] = remembered;
    } else {
        site->entries[KILN_C99_PIC_SIZE - 1] = remembered;
    }
}

static int kiln_c99_val_read_property_view(kiln_host *host, kiln_val target,
                                           const kiln_string *key,
                                           const kiln_string *canonical, int depth,
                                           kiln_value *thrown, kiln_c99_site *site,
                                           kiln_val *out) {
    if (KILN_LIKELY(site != NULL && site->pic_count > 0 && KILN_VAL_IS_OBJECT(target))) {
        void *ptr = KILN_VAL_PTR(target);
        if (KILN_LIKELY(ptr != NULL && kiln_c99_gc_payload_kind(ptr) == KILN_GC_OBJECT)) {
            const kiln_object *object = (const kiln_object *)ptr;
            size_t p;
            for (p = 0; p < site->pic_count; ++p) {
                if (object->shape == site->entries[p].shape) {
                    if (site->entries[p].holder != NULL) {
                        kiln_c99_pic_entry *entry = (kiln_c99_pic_entry *)&site->entries[p];
                        const kiln_object *first = entry->via != NULL ? entry->via : entry->holder;
                        if (entry->epoch == kiln_c99_proto_epoch &&
                            object->prototype.tag == KILN_VALUE_OBJECT &&
                            (const kiln_object *)(uintptr_t)object->prototype.bits == first) {
                            *out = entry->holder_shape != NULL
                                       ? entry->holder->slots[entry->holder_slot]
                                       : entry->holder_val;
                            return KILN_C99_OK;
                        }
                        kiln_val slow_val;
                        if (kiln_c99_site_validate_holder_val(entry, object, &slow_val)) {
                            entry->epoch = kiln_c99_proto_epoch;
                            *out = slow_val;
                            return KILN_C99_OK;
                        }
                    } else if (site->entries[p].slot < object->slot_capacity) {
                        *out = object->slots[site->entries[p].slot];
                        return KILN_C99_OK;
                    } else if (site->entries[p].slot == UINT32_MAX &&
                               kiln_c99_site_miss_holds(&site->entries[p], object, canonical)) {
#ifdef KILN_IC_CHECK
                        kiln_c99_site_miss_check(host, target, key, canonical, depth, thrown);
#endif
                        *out = KILN_VAL_UNDEFINED;
                        return KILN_C99_OK;
                    }
                }
            }
        }
    }
    if (KILN_LIKELY(kiln_c99_cached_method_matches(site, target))) {
        *out = KILN_VAL_METHOD(site->cached_method);
        return KILN_C99_OK;
    }
    if (KILN_VAL_IS_OBJECT(target)) {
        void *ptr = KILN_VAL_PTR(target);
        if (KILN_LIKELY(ptr != NULL && kiln_c99_gc_payload_kind(ptr) == KILN_GC_OBJECT)) {
            const kiln_object *object = (const kiln_object *)ptr;
            if (object->shape != NULL) {
                const kiln_string *stable = canonical != NULL ? canonical : kiln_c99_interned_key(key);
                if (stable != NULL) {
                    uint32_t own_slot = UINT32_MAX;
                    if (kiln_c99_shape_find_slot(object->shape, stable, &own_slot)) {
                        if (own_slot < object->slot_capacity) {
                            *out = object->slots[own_slot];
                            if (site != NULL) {
                                uint32_t p;
                                int found = 0;
                                for (p = 0; p < site->pic_count; ++p) {
                                    if (site->entries[p].shape == object->shape) {
                                        site->entries[p].transition_to = NULL;
                                        site->entries[p].slot = own_slot;
                                        site->entries[p].holder = NULL;
                                        found = 1;
                                        break;
                                    }
                                }
                                if (!found) {
                                    if (site->pic_count < KILN_C99_PIC_SIZE) {
                                        site->entries[site->pic_count].shape = object->shape;
                                        site->entries[site->pic_count].transition_to = NULL;
                                        site->entries[site->pic_count].slot = own_slot;
                                        site->entries[site->pic_count].holder = NULL;
                                        site->pic_count += 1;
                                    } else {
                                        uint32_t last = KILN_C99_PIC_SIZE - 1;
                                        site->entries[last].shape = object->shape;
                                        site->entries[last].transition_to = NULL;
                                        site->entries[last].slot = own_slot;
                                        site->entries[last].holder = NULL;
                                    }
                                }
                            }
                            return KILN_C99_OK;
                        }
                    }
                    if (object->properties == NULL) {
                        const kiln_object *via = NULL;
                        const kiln_object *step = NULL;
                        uint32_t proto_slot = UINT32_MAX;
                        kiln_val proto_val = KILN_VAL_UNDEFINED;
                        int steps;
                        kiln_value link = object->prototype;
                        for (steps = 0; steps < 2 && link.tag == KILN_VALUE_OBJECT; ++steps) {
                            const kiln_object *candidate = (const kiln_object *)(uintptr_t)link.bits;
                            if (candidate == NULL || kiln_c99_gc_payload_kind(candidate) != KILN_GC_OBJECT) {
                                break;
                            }
                            if (candidate->shape != NULL) {
                                if (kiln_c99_shape_find_slot(candidate->shape, stable, &proto_slot)) {
                                    if (proto_slot < candidate->slot_capacity) {
                                        step = candidate;
                                        proto_val = candidate->slots[proto_slot];
                                    }
                                    break;
                                }
                            } else {
                                break;
                            }
                            via = candidate;
                            link = candidate->prototype;
                        }
                        if (step != NULL) {
                            *out = proto_val;
                            if (site != NULL && site->pic_count < KILN_C99_PIC_SIZE) {
                                kiln_c99_pic_entry remembered;
                                memset(&remembered, 0, sizeof remembered);
                                remembered.shape = object->shape;
                                remembered.slot = UINT32_MAX;
                                remembered.holder = step;
                                remembered.holder_shape = step->shape;
                                remembered.holder_slot = proto_slot;
                                remembered.holder_val = proto_val;
                                remembered.epoch = kiln_c99_proto_epoch;
                                ((kiln_object *)step)->cache_holder = 1;
                                if (via != NULL) {
                                    remembered.via = via;
                                    remembered.via_shape = via->shape;
                                    ((kiln_object *)via)->cache_holder = 1;
                                }
                                site->entries[site->pic_count++] = remembered;
                                kiln_c99_gc_register_site(site);
                            }
                            return KILN_C99_OK;
                        }
                    }
                }
            }
        }
    } else if (KILN_VAL_IS_STRING(target)) {
        if (key != NULL && key->unit_len == 6 && kiln_c99_same_name_key(key, "length")) {
            if (site != NULL) {
                site->cached_tag = KILN_VALUE_STRING;
                site->has_cached_method = 0;
            }
            const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(target);
            *out = kiln_c99_val_from_double((double)(s != NULL ? s->unit_len : 0));
            return KILN_C99_OK;
        }
    } else if ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {
        void *ptr = KILN_VAL_PTR(target);
        if (site != NULL && (site->cached_tag == KILN_VALUE_ARRAY || kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY)) {
            if (key != NULL && key->unit_len == 6 && kiln_c99_same_name_key(key, "length")) {
                if (site != NULL) {
                    site->cached_tag = KILN_VALUE_ARRAY;
                    site->has_cached_method = 0;
                }
                const kiln_array *arr = (const kiln_array *)ptr;
                *out = kiln_c99_val_from_double((double)arr->count);
                return KILN_C99_OK;
            }
        }
    }
    kiln_value t = kiln_val_to_boundary(target);
    kiln_value o = kiln_c99_undefined();
    int status = kiln_c99_get_site(host, t, key, canonical, &o, depth, thrown, site);
    if (status == KILN_C99_OK) {
        *out = kiln_val_from_boundary(o);
        if (o.tag == KILN_VALUE_UNDEFINED && site != NULL && KILN_VAL_IS_OBJECT(target)) {
            kiln_c99_site_learn_miss(site, target, key, canonical);
        }
    }
    return status;
}

static int kiln_c99_val_read_property_key(kiln_host *host, kiln_val target,
                                          const kiln_string *key, int depth,
                                          kiln_value *thrown, kiln_c99_site *site,
                                          kiln_val *out) {
    return kiln_c99_val_read_property_view(host, target, key, key, depth, thrown, site, out);
}

static int kiln_c99_val_read_property(kiln_host *host, kiln_val target,
                                      const char *key, size_t key_len,
                                      int depth, kiln_value *thrown,
                                      kiln_c99_site *site, kiln_val *out) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_OUT_OF_MEMORY;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    return kiln_c99_val_read_property_view(host, target, &view, NULL, depth, thrown, site, out);
}

static int kiln_c99_val_write_property_view(kiln_host *host, kiln_val target,
                                            const kiln_string *key,
                                            const kiln_string *canonical, kiln_val value,
                                            int strict, int depth, kiln_value *thrown,
                                            kiln_c99_site *site) {
    if (KILN_LIKELY(site != NULL && site->pic_count > 0 && KILN_VAL_IS_OBJECT(target))) {
        void *ptr = KILN_VAL_PTR(target);
        if (KILN_LIKELY(ptr != NULL && kiln_c99_gc_payload_kind(ptr) == KILN_GC_OBJECT)) {
            kiln_object *object = (kiln_object *)ptr;
            size_t p;
            kiln_c99_prototype_touched(object);
            for (p = 0; p < site->pic_count; ++p) {
                if (object->shape == site->entries[p].shape) {
                    if (site->entries[p].transition_to == NULL) {
                        if (KILN_LIKELY(site->entries[p].slot < object->slot_capacity && !object->frozen)) {
                            if ((object->shape->double_slots_mask & (1u << site->entries[p].slot)) &&
                                !KILN_VAL_IS_NUMBER(value)) {
                                kiln_c99_ensure_dictionary(object);
                                kiln_value t = kiln_val_to_boundary(target);
                                kiln_value v = kiln_val_to_boundary(value);
                                return kiln_c99_put_site_key(host, t, key, canonical, v, strict,
                                                             depth, thrown, site);
                            }
                            object->slots[site->entries[p].slot] = value;
                            return KILN_C99_OK;
                        }
                    } else if (!object->not_extensible &&
                               site->entries[p].slot < object->slot_capacity &&
                               kiln_c99_transition_store_clear(
                                   site, &site->entries[p], object,
                                   canonical != NULL ? canonical : kiln_c99_interned_key(key),
                                   key)) {
                        const kiln_shape *next_shape = site->entries[p].transition_to;
                        if ((next_shape->double_slots_mask & (1u << site->entries[p].slot)) &&
                            !KILN_VAL_IS_NUMBER(value)) {
                            /* Non-double written to double slot transition; fall through */
                        } else {
                            object->slots[site->entries[p].slot] = value;
                            object->shape = (kiln_shape *)site->entries[p].transition_to;
                            return KILN_C99_OK;
                        }
                    }
                    break;
                }
            }
        }
    }
    if (KILN_VAL_IS_OBJECT(target)) {
        void *ptr = KILN_VAL_PTR(target);
        if (KILN_LIKELY(ptr != NULL && kiln_c99_gc_payload_kind(ptr) == KILN_GC_OBJECT)) {
            kiln_object *object = (kiln_object *)ptr;
            if (object->shape != NULL) {
            const kiln_string *stable = canonical != NULL ? canonical : kiln_c99_interned_key(key);
            if (stable != NULL) {
                uint32_t slot = UINT32_MAX;
                if (kiln_c99_shape_find_slot(object->shape, stable, &slot)) {
                    if (slot < object->slot_capacity && !object->frozen) {
                        if ((object->shape->double_slots_mask & (1u << slot)) && !KILN_VAL_IS_NUMBER(value)) {
                            kiln_c99_ensure_dictionary(object);
                        } else {
                            kiln_c99_prototype_touched(object);
                            object->slots[slot] = value;
                            if (site != NULL) {
                                uint32_t p;
                                int found = 0;
                                for (p = 0; p < site->pic_count; ++p) {
                                    if (site->entries[p].shape == object->shape) {
                                        site->entries[p].transition_to = NULL;
                                        site->entries[p].slot = slot;
                                        site->entries[p].holder = NULL;
                                        found = 1;
                                        break;
                                    }
                                }
                                if (!found) {
                                    if (site->pic_count < KILN_C99_PIC_SIZE) {
                                        site->entries[site->pic_count].shape = object->shape;
                                        site->entries[site->pic_count].transition_to = NULL;
                                        site->entries[site->pic_count].slot = slot;
                                        site->entries[site->pic_count].holder = NULL;
                                        site->pic_count += 1;
                                    } else {
                                        uint32_t last = KILN_C99_PIC_SIZE - 1;
                                        site->entries[last].shape = object->shape;
                                        site->entries[last].transition_to = NULL;
                                        site->entries[last].slot = slot;
                                        site->entries[last].holder = NULL;
                                    }
                                }
                            }
                            return KILN_C99_OK;
                        }
                    }
                }
            }
        }
    }
    }
    kiln_value t = kiln_val_to_boundary(target);
    kiln_value v = kiln_val_to_boundary(value);
    return kiln_c99_put_site_key(host, t, key, canonical, v, strict, depth, thrown, site);
}

static int kiln_c99_val_write_property_key(kiln_host *host, kiln_val target,
                                           const kiln_string *key, kiln_val value, int strict,
                                           int depth, kiln_value *thrown,
                                           kiln_c99_site *site) {
    return kiln_c99_val_write_property_view(host, target, key, key, value, strict, depth, thrown,
                                             site);
}

static int kiln_c99_val_write_property(kiln_host *host, kiln_val target,
                                       const char *key, size_t key_len,
                                       kiln_val value, int strict,
                                       int depth, kiln_value *thrown,
                                       kiln_c99_site *site) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_OUT_OF_MEMORY;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    return kiln_c99_val_write_property_view(host, target, &view, NULL, value, strict, depth,
                                            thrown, site);
}

static int kiln_c99_val_get_private_fast(kiln_val *out, kiln_val receiver, kiln_val brand,
                                         const char *brand_key, size_t brand_len,
                                         const char *member_key, size_t member_len,
                                         kiln_c99_private_site *site) {
    if (!KILN_VAL_IS_OBJECT(receiver)) {
        return 0;
    }
    kiln_object *obj = (kiln_object *)KILN_VAL_PTR(receiver);
    if (obj == NULL || obj->shape == NULL || kiln_c99_gc_kind_of_ptr(obj) != KILN_GC_OBJECT) {
        return 0;
    }
    if (obj->accessor_count != 0 || obj->properties != NULL) {
        return 0;
    }
    uint32_t bslot = UINT32_MAX;
    if (brand != KILN_VAL_UNDEFINED) {
        bslot = kiln_c99_shape_slot_named(obj->shape, brand_key, brand_len);
        if (bslot == UINT32_MAX || bslot >= obj->slot_capacity || obj->slots[bslot] != brand) {
            return 0;
        }
    }
    uint32_t mslot = kiln_c99_shape_slot_named(obj->shape, member_key, member_len);
    if (mslot == UINT32_MAX || mslot >= obj->slot_capacity) {
        return 0;
    }
    if (site != NULL) {
        site->shape = obj->shape;
        site->brand_slot = bslot;
        site->member_slot = mslot;
    }
    *out = obj->slots[mslot];
    return 1;
}

static int kiln_c99_val_set_private_fast(kiln_val receiver, kiln_val brand, kiln_val member,
                                         const char *brand_key, size_t brand_len,
                                         const char *member_key, size_t member_len,
                                         kiln_c99_private_site *site) {
    if (!KILN_VAL_IS_OBJECT(receiver)) {
        return 0;
    }
    kiln_object *obj = (kiln_object *)KILN_VAL_PTR(receiver);
    if (obj == NULL || obj->shape == NULL || obj->frozen || kiln_c99_gc_kind_of_ptr(obj) != KILN_GC_OBJECT) {
        return 0;
    }
    if (obj->accessor_count != 0 || obj->properties != NULL) {
        return 0;
    }
    uint32_t bslot = UINT32_MAX;
    if (brand != KILN_VAL_UNDEFINED) {
        bslot = kiln_c99_shape_slot_named(obj->shape, brand_key, brand_len);
        if (bslot == UINT32_MAX || bslot >= obj->slot_capacity || obj->slots[bslot] != brand) {
            return 0;
        }
    }
    uint32_t mslot = kiln_c99_shape_slot_named(obj->shape, member_key, member_len);
    if (mslot == UINT32_MAX || mslot >= obj->slot_capacity) {
        return 0;
    }
    if ((obj->shape->double_slots_mask & (1u << mslot)) != 0 && !KILN_VAL_IS_NUMBER(member)) {
        return 0;
    }
    if (site != NULL) {
        site->shape = obj->shape;
        site->brand_slot = bslot;
        site->member_slot = mslot;
    }
    kiln_c99_prototype_touched(obj);
    obj->slots[mslot] = member;
    return 1;
}

static void kiln_c99_note_get_property_refusal(kiln_value target) {
    if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = target.tag;
    if (target.tag == KILN_VALUE_BUILTIN && kiln_c99_diagnostic_function == NULL &&
        kiln_c99_diagnostic_function_key == NULL) {
        const char *named = kiln_c99_builtin_name(target.bits);
        if (named != NULL) {
            kiln_c99_diagnostic_function = named;
            kiln_c99_diagnostic_function_len = strlen(named);
        }
    }
    if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = "GetProperty lookup refused";
}

static void kiln_c99_note_get_computed_refusal(kiln_value target, kiln_value key) {
    if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = target.tag;
    if (kiln_c99_diagnostic_function == NULL && kiln_c99_diagnostic_function_key == NULL &&
        kiln_c99_diagnostic_detail != kiln_c99_diagnostic_compose) {
        const char *named = target.tag == KILN_VALUE_BUILTIN ? kiln_c99_builtin_name(target.bits) : NULL;
        if (key.tag == KILN_VALUE_STRING && key.ptr != NULL) {
            uint8_t *key_owned = NULL;
            size_t key_bytes_len = 0;
            if (kiln_c99_string_to_utf8_lossy(key, &key_owned, &key_bytes_len) != KILN_C99_OK) {
                key_owned = NULL;
                key_bytes_len = 0;
            }
            size_t pos = 0;
            if (named != NULL) {
                size_t nlen = strlen(named);
                if (nlen < sizeof kiln_c99_diagnostic_compose - 1) {
                    memcpy(kiln_c99_diagnostic_compose, named, nlen);
                    pos = nlen;
                    kiln_c99_diagnostic_compose[pos++] = '.';
                }
            }
            size_t klen = key_bytes_len;
            if (pos + klen >= sizeof kiln_c99_diagnostic_compose) {
                klen = sizeof kiln_c99_diagnostic_compose - 1 - pos;
            }
            if (klen != 0) memcpy(kiln_c99_diagnostic_compose + pos, key_owned, klen);
            pos += klen;
            kiln_c99_diagnostic_compose[pos] = '\0';
            kiln_c99_diagnostic_function = kiln_c99_diagnostic_compose;
            kiln_c99_diagnostic_function_len = pos;
            free(key_owned);
        } else if (named != NULL) {
            kiln_c99_diagnostic_function = named;
            kiln_c99_diagnostic_function_len = strlen(named);
        }
    }
    if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = "GetComputed lookup refused";
}

static int kiln_c99_read_element(kiln_host *host, kiln_value target, kiln_value key,
                                 int depth, kiln_value *thrown, kiln_value *out) {
    if (KILN_LIKELY(target.tag == KILN_VALUE_ARRAY && key.tag == KILN_VALUE_NUMBER)) {
        double idx = kiln_c99_as_number(key);
        if (KILN_LIKELY(idx >= 0.0 && idx <= 4294967294.0 && floor(idx) == idx)) {
            size_t slot = (size_t)idx;
            const kiln_array *array = kiln_c99_array_of(target);
            if (KILN_LIKELY(slot < array->count && array->elements[slot] != KILN_VAL_HOLE)) {
                *out = kiln_val_to_boundary(array->elements[slot]);
                return KILN_C99_OK;
            }
        }
    }
    if (target.tag == KILN_VALUE_OBJECT && key.tag == KILN_VALUE_NUMBER) {
        double idx = kiln_c99_as_number(key);
        if (KILN_LIKELY(idx >= 0.0 && idx <= 4294967294.0 && floor(idx) == idx)) {
            size_t slot = (size_t)idx;
            const kiln_object *object = kiln_c99_object_of(target);
            if (object->elements != NULL && slot < object->element_count && object->elements[slot] != KILN_VAL_HOLE) {
                *out = kiln_val_to_boundary(object->elements[slot]);
                return KILN_C99_OK;
            }
        }
    }
    return kiln_c99_get_computed(host, target, key, out, depth, thrown);
}

static int kiln_c99_write_element(kiln_host *host, kiln_value target, kiln_value key,
                                  kiln_value value, int strict, int depth,
                                  kiln_value *thrown) {
    if (KILN_LIKELY(target.tag == KILN_VALUE_ARRAY && key.tag == KILN_VALUE_NUMBER)) {
        double idx = kiln_c99_as_number(key);
        if (KILN_LIKELY(idx >= 0.0 && idx <= 4294967294.0 && floor(idx) == idx)) {
            size_t slot = (size_t)idx;
            kiln_array *array = kiln_c99_array_of(target);
            if (KILN_LIKELY(!array->frozen && slot < array->count)) {
                array->elements[slot] = kiln_val_from_boundary(value);
                return KILN_C99_OK;
            }
        }
    }
    if (target.tag == KILN_VALUE_OBJECT && key.tag == KILN_VALUE_NUMBER) {
        double idx = kiln_c99_as_number(key);
        if (KILN_LIKELY(idx >= 0.0 && idx <= 4294967294.0 && floor(idx) == idx)) {
            size_t slot = (size_t)idx;
            kiln_object *object = kiln_c99_object_of(target);
            if (KILN_LIKELY(object->shape != NULL && !object->frozen && object->elements != NULL &&
                            slot < object->element_count && object->elements[slot] != KILN_VAL_HOLE)) {
                object->elements[slot] = kiln_val_from_boundary(value);
                return KILN_C99_OK;
            }
        }
    }
    return kiln_c99_set_computed_mode(host, target, key, value, strict, depth, thrown);
}

static int kiln_c99_val_read_element(kiln_host *host, kiln_val target, kiln_val key,
                                     int depth, kiln_value *thrown, kiln_val *out) {
    if (KILN_LIKELY((KILN_VAL_IS_OBJECT(target) || ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) && KILN_VAL_IS_NUMBER(key))) {
        double idx;
        memcpy(&idx, &key, sizeof(idx));
        if (KILN_LIKELY(idx >= 0.0 && idx <= 4294967294.0 && floor(idx) == idx)) {
            size_t slot = (size_t)idx;
            void *ptr = KILN_VAL_PTR(target);
            uint32_t gc_kind = kiln_c99_gc_kind_of_ptr(ptr);
            if (gc_kind == KILN_GC_ARRAY) {
                const kiln_array *array = (const kiln_array *)ptr;
                if (KILN_LIKELY(slot < array->count && array->elements[slot] != KILN_VAL_HOLE)) {
                    *out = array->elements[slot];
                    return KILN_C99_OK;
                }
            } else if (gc_kind == KILN_GC_OBJECT) {
                const kiln_object *object = (const kiln_object *)ptr;
                if (object->elements != NULL && slot < object->element_count && object->elements[slot] != KILN_VAL_HOLE) {
                    *out = object->elements[slot];
                    return KILN_C99_OK;
                }
            }
        }
    }
    if (KILN_VAL_IS_STRING(target) && KILN_VAL_IS_NUMBER(key)) {
        double idx;
        memcpy(&idx, &key, sizeof(idx));
        if (idx >= 0.0 && idx <= 4294967294.0 && floor(idx) == idx) {
            size_t slot = (size_t)idx;
            const kiln_string *str = (const kiln_string *)KILN_VAL_PTR(target);
            if (str != NULL && slot < str->unit_len) {
                *out = kiln_c99_val_from_unit(kiln_string_unit_at(str, slot));
                return KILN_C99_OK;
            } else if (str != NULL && slot >= str->unit_len) {
                *out = KILN_VAL_UNDEFINED;
                return KILN_C99_OK;
            }
        }
    }
    if (KILN_LIKELY(KILN_VAL_IS_OBJECT(target) && KILN_VAL_IS_STRING(key))) {
        kiln_object *obj = (kiln_object *)KILN_VAL_PTR(target);
        const kiln_string *keystr = (const kiln_string *)KILN_VAL_PTR(key);
        if (KILN_LIKELY(obj != NULL && keystr != NULL && kiln_c99_gc_kind_of_ptr(obj) == KILN_GC_OBJECT)) {
            if (obj->shape != NULL && obj->accessor_count == 0) {
                if (obj->elements != NULL) {
                    double index_num = kiln_c99_key_index_key(keystr);
                    if (index_num >= 0.0 && index_num < 100000.0) {
                        size_t elem_idx = (size_t)index_num;
                        if (elem_idx < obj->element_count &&
                            obj->elements[elem_idx] != KILN_VAL_HOLE) {
                            *out = obj->elements[elem_idx];
                            return KILN_C99_OK;
                        }
                    }
                }
                uint32_t slot;
                const kiln_string *ikey = kiln_c99_intern_key(keystr);
                if (ikey != NULL && kiln_c99_shape_find_slot(obj->shape, ikey, &slot)) {
                    if (slot < obj->slot_capacity) {
                        *out = obj->slots[slot];
                        return KILN_C99_OK;
                    }
                }
                if (obj->prototype.tag == KILN_VALUE_NULL) {
                    *out = KILN_VAL_UNDEFINED;
                    return KILN_C99_OK;
                }
            } else if (obj->properties != NULL && obj->accessor_count == 0 && obj->prototype.tag == KILN_VALUE_NULL) {
                const kiln_string *ikey = kiln_c99_intern_key(keystr);
                size_t idx = kiln_c99_own_index_key(obj, ikey, keystr);
                if (idx != obj->count) {
                    *out = kiln_val_from_boundary(obj->properties[idx].value);
                    return KILN_C99_OK;
                } else {
                    *out = KILN_VAL_UNDEFINED;
                    return KILN_C99_OK;
                }
            }
        }
    }
    kiln_value t = kiln_val_to_boundary(target);
    kiln_value k = kiln_val_to_boundary(key);
    kiln_value o;
    int status = kiln_c99_read_element(host, t, k, depth, thrown, &o);
    if (status == KILN_C99_OK) *out = kiln_val_from_boundary(o);
    return status;
}

static int kiln_c99_val_write_element(kiln_host *host, kiln_val target, kiln_val key,
                                      kiln_val value, int strict, int depth, kiln_value *thrown) {
    if (KILN_LIKELY((KILN_VAL_IS_OBJECT(target) || ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) && KILN_VAL_IS_NUMBER(key))) {
        double idx;
        memcpy(&idx, &key, sizeof(idx));
        if (KILN_LIKELY(idx >= 0.0 && idx <= 4294967294.0 && floor(idx) == idx)) {
            size_t slot = (size_t)idx;
            void *ptr = KILN_VAL_PTR(target);
            uint32_t gc_kind = kiln_c99_gc_kind_of_ptr(ptr);
            if (gc_kind == KILN_GC_ARRAY) {
                kiln_array *array = (kiln_array *)ptr;
                if (KILN_LIKELY(!array->frozen && slot < array->count)) {
                    if (array->is_numeric && !KILN_VAL_IS_NUMBER(value)) {
                        array->is_numeric = 0;
                    }
                    array->elements[slot] = value;
                    return KILN_C99_OK;
                }
            } else if (gc_kind == KILN_GC_OBJECT) {
                kiln_object *object = (kiln_object *)ptr;
                if (KILN_LIKELY(object->shape != NULL && !object->frozen && object->elements != NULL &&
                                slot < object->element_count && object->elements[slot] != KILN_VAL_HOLE)) {
                    object->elements[slot] = value;
                    return KILN_C99_OK;
                }
            }
        }
    }
    kiln_value t = kiln_val_to_boundary(target);
    kiln_value k = kiln_val_to_boundary(key);
    kiln_value v = kiln_val_to_boundary(value);
    return kiln_c99_write_element(host, t, k, v, strict, depth, thrown);
}

static int kiln_c99_val_invoke_cached_method(kiln_host *host, uint64_t method_code, kiln_val receiver,
                                            const kiln_val *args, size_t argc, int depth,
                                            kiln_value *thrown, kiln_val *out) {
    if (depth >= KILN_C99_MAX_CALL_DEPTH) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Maximum call stack size exceeded");
    }
    if (method_code == KILN_C99_METHOD_CHAR_CODE_AT) {
        if (KILN_VAL_IS_STRING(receiver)) {
            const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(receiver);
            if (s != NULL) {
                size_t idx = 0;
                if (argc > 0) {
                    if (KILN_VAL_IS_NUMBER(args[0])) {
                        double d;
                        memcpy(&d, &args[0], sizeof(d));
                        if (isnan(d)) d = 0.0;
                        d = trunc(d);
                        if (d >= 0.0 && d < (double)s->unit_len) {
                            idx = (size_t)d;
                            *out = kiln_c99_val_from_double((double)kiln_string_unit_at(s, idx));
                            return KILN_C99_OK;
                        } else {
                            *out = KILN_QNAN_BASE;
                            return KILN_C99_OK;
                        }
                    }
                } else {
                    if (s->unit_len > 0) {
                        *out = kiln_c99_val_from_double((double)kiln_string_unit_at(s, 0));
                        return KILN_C99_OK;
                    } else {
                        *out = KILN_QNAN_BASE;
                        return KILN_C99_OK;
                    }
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_CHAR_AT) {
        if (KILN_VAL_IS_STRING(receiver)) {
            const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(receiver);
            if (s != NULL) {
                size_t idx = 0;
                if (argc > 0) {
                    if (KILN_VAL_IS_NUMBER(args[0])) {
                        double d;
                        memcpy(&d, &args[0], sizeof(d));
                        if (isnan(d)) d = 0.0;
                        d = trunc(d);
                        if (d >= 0.0 && d < (double)s->unit_len) {
                            idx = (size_t)d;
                            *out = kiln_c99_val_from_unit(kiln_string_unit_at(s, idx));
                            return KILN_C99_OK;
                        } else {
                            *out = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
                            return KILN_C99_OK;
                        }
                    }
                } else {
                    if (s->unit_len > 0) {
                        *out = kiln_c99_val_from_unit(kiln_string_unit_at(s, 0));
                        return KILN_C99_OK;
                    } else {
                        *out = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
                        return KILN_C99_OK;
                    }
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_SUBSTRING || method_code == KILN_C99_METHOD_STRING_SLICE || method_code == KILN_C99_METHOD_SLICE) {
        if (KILN_VAL_IS_STRING(receiver)) {
            const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(receiver);
            if (s != NULL && (argc == 0 || KILN_VAL_IS_NUMBER(args[0]) || args[0] == KILN_VAL_UNDEFINED) && (argc < 2 || KILN_VAL_IS_NUMBER(args[1]) || args[1] == KILN_VAL_UNDEFINED)) {
                size_t ulen = s->unit_len;
                int relative = (method_code == KILN_C99_METHOD_STRING_SLICE || method_code == KILN_C99_METHOD_SLICE);
                size_t start = 0;
                size_t end = ulen;
                if (argc > 0 && KILN_VAL_IS_NUMBER(args[0])) {
                    double d;
                    memcpy(&d, &args[0], sizeof(d));
                    if (isnan(d)) d = 0.0;
                    if (relative && d < 0.0) {
                        d += (double)ulen;
                        if (d < 0.0) d = 0.0;
                    } else if (d < 0.0) {
                        d = 0.0;
                    }
                    if (d > (double)ulen) d = (double)ulen;
                    start = (size_t)d;
                }
                if (argc > 1 && KILN_VAL_IS_NUMBER(args[1])) {
                    double d;
                    memcpy(&d, &args[1], sizeof(d));
                    if (isnan(d)) d = 0.0;
                    if (relative && d < 0.0) {
                        d += (double)ulen;
                        if (d < 0.0) d = 0.0;
                    } else if (d < 0.0) {
                        d = 0.0;
                    }
                    if (d > (double)ulen) d = (double)ulen;
                    end = (size_t)d;
                }
                if (!relative && start > end) {
                    size_t tmp = start;
                    start = end;
                    end = tmp;
                } else if (relative && start > end) {
                    end = start;
                }
                *out = kiln_c99_val_slice_string(s, start, end - start);
                return KILN_C99_OK;
            }
        }
    } else if (method_code == KILN_C99_METHOD_PUSH) {
        if (argc == 1) {
            if (kiln_c99_val_array_push_fast(receiver, args[0], out) == KILN_C99_OK) {
                return KILN_C99_OK;
            }
        } else if ((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {
            void *ptr = KILN_VAL_PTR(receiver);
            if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {
                kiln_array *arr = (kiln_array *)ptr;
                if (!arr->frozen && !arr->not_extensible) {
                    if (argc == 0) {
                        *out = kiln_c99_val_from_double((double)arr->count);
                        return KILN_C99_OK;
                    }
                    if (arr->count + argc > arr->capacity) {
                        int status = kiln_c99_array_reserve(arr, arr->count + argc);
                        if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
                    }
                    for (size_t i = 0; i < argc; ++i) {
                        if (arr->is_numeric && !KILN_VAL_IS_NUMBER(args[i])) {
                            arr->is_numeric = 0;
                        }
                        arr->elements[arr->count++] = args[i];
                    }
                    *out = kiln_c99_val_from_double((double)arr->count);
                    return KILN_C99_OK;
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_POP) {
        if (KILN_LIKELY((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {
            void *ptr = KILN_VAL_PTR(receiver);
            if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {
                kiln_array *arr = (kiln_array *)ptr;
                if (!arr->frozen && !arr->not_extensible) {
                    if (arr->count == 0) {
                        *out = KILN_VAL_UNDEFINED;
                        return KILN_C99_OK;
                    }
                    arr->count -= 1;
                    *out = arr->elements[arr->count];
                    return KILN_C99_OK;
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_ARRAY_SHIFT) {
        if (KILN_LIKELY((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {
            void *ptr = KILN_VAL_PTR(receiver);
            if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {
                kiln_array *arr = (kiln_array *)ptr;
                if (!arr->frozen && !arr->not_extensible) {
                    if (arr->count == 0) {
                        *out = KILN_VAL_UNDEFINED;
                        return KILN_C99_OK;
                    }
                    kiln_val first = arr->elements[0] == KILN_VAL_HOLE ? KILN_VAL_UNDEFINED : arr->elements[0];
                    arr->count -= 1;
                    if (arr->count > 0) {
                        memmove(arr->elements, arr->elements + 1, arr->count * sizeof *arr->elements);
                    }
                    *out = first;
                    return KILN_C99_OK;
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_SPLIT) {
        if (KILN_VAL_IS_STRING(receiver) && argc == 1 && KILN_VAL_IS_STRING(args[0])) {
            const kiln_string *src = (const kiln_string *)KILN_VAL_PTR(receiver);
            const kiln_string *sep = (const kiln_string *)KILN_VAL_PTR(args[0]);
            if (src != NULL && sep != NULL && !kiln_string_is_utf16(src) && !kiln_string_is_utf16(sep) && sep->unit_len == 1) {
                kiln_value bresult;
                int st = kiln_c99_split_string_ascii_byte(&bresult, src, (char)kiln_string_unit_at(sep, 0), (size_t)0xFFFFFFFFu);
                if (KILN_LIKELY(st == KILN_C99_OK)) {
                    *out = kiln_val_from_boundary(bresult);
                    return KILN_C99_OK;
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_COLLECTION_GET) {
        if (argc > 0 && (KILN_VAL_IS_OBJECT(receiver) || ((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
            void *ptr = KILN_VAL_PTR(receiver);
            if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {
                kiln_collection *col = (kiln_collection *)ptr;
                if (col->is_map && !col->is_weak) {
                    *out = kiln_c99_collection_get_val_fast(col, args[0]);
                    return KILN_C99_OK;
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_COLLECTION_HAS) {
        if (argc > 0 && (KILN_VAL_IS_OBJECT(receiver) || ((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
            void *ptr = KILN_VAL_PTR(receiver);
            if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {
                kiln_collection *col = (kiln_collection *)ptr;
                if (!col->is_weak) {
                    *out = kiln_c99_collection_has_val_fast(col, args[0]);
                    return KILN_C99_OK;
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_COLLECTION_SET) {
        if (argc >= 2 && (KILN_VAL_IS_OBJECT(receiver) || ((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
            void *ptr = KILN_VAL_PTR(receiver);
            if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {
                kiln_collection *col = (kiln_collection *)ptr;
                if (col->is_map && !col->is_weak) {
                    int st = kiln_c99_collection_put_val_fast(col, args[0], args[1]);
                    if (KILN_UNLIKELY(st != KILN_C99_OK)) return st;
                    *out = receiver;
                    return KILN_C99_OK;
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_COLLECTION_ADD) {
        if (argc >= 1 && (KILN_VAL_IS_OBJECT(receiver) || ((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
            void *ptr = KILN_VAL_PTR(receiver);
            if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {
                kiln_collection *col = (kiln_collection *)ptr;
                if (!col->is_map && !col->is_weak) {
                    int st = kiln_c99_collection_put_val_fast(col, args[0], args[0]);
                    if (KILN_UNLIKELY(st != KILN_C99_OK)) return st;
                    *out = receiver;
                    return KILN_C99_OK;
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_COLLECTION_DELETE) {
        if (argc >= 1 && (KILN_VAL_IS_OBJECT(receiver) || ((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
            void *ptr = KILN_VAL_PTR(receiver);
            if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION) {
                kiln_collection *col = (kiln_collection *)ptr;
                if (!col->is_weak) {
                    *out = kiln_c99_collection_delete_val_fast(col, args[0]);
                    return KILN_C99_OK;
                }
            }
        }
    } else if (method_code == KILN_C99_METHOD_SLICE) {
        if ((receiver & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {
            void *ptr = KILN_VAL_PTR(receiver);
            if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {
                if ((argc > 0 && !KILN_VAL_IS_NUMBER(args[0]) && args[0] != KILN_VAL_UNDEFINED) ||
                    (argc > 1 && !KILN_VAL_IS_NUMBER(args[1]) && args[1] != KILN_VAL_UNDEFINED)) {
                    goto fallback;
                }
                const kiln_array *array = (const kiln_array *)ptr;
                size_t start = 0;
                size_t end = array->count;
                if (argc > 0 && KILN_VAL_IS_NUMBER(args[0])) {
                    double d;
                    memcpy(&d, &args[0], sizeof(d));
                    if (isnan(d)) d = 0.0;
                    if (d < 0.0) {
                        d += (double)array->count;
                        if (d < 0.0) d = 0.0;
                    }
                    if (d > (double)array->count) d = (double)array->count;
                    start = (size_t)d;
                }
                if (argc > 1 && KILN_VAL_IS_NUMBER(args[1])) {
                    double d;
                    memcpy(&d, &args[1], sizeof(d));
                    if (isnan(d)) d = 0.0;
                    if (d < 0.0) {
                        d += (double)array->count;
                        if (d < 0.0) d = 0.0;
                    }
                    if (d > (double)array->count) d = (double)array->count;
                    end = (size_t)d;
                }
                size_t slice_len = (end > start && start < array->count) ? (end < array->count ? end - start : array->count - start) : 0;
                kiln_value built;
                int st = kiln_c99_new_array_with_capacity(&built, slice_len, array->is_numeric);
                if (KILN_UNLIKELY(st != KILN_C99_OK)) return st;
                if (slice_len > 0) {
                    kiln_array *b_arr = kiln_c99_array_of(built);
                    memcpy(b_arr->elements, array->elements + start, slice_len * sizeof(kiln_val));
                    b_arr->count = slice_len;
                }
                *out = kiln_val_from_boundary(built);
                return KILN_C99_OK;
            }
        }
    } else if (method_code == KILN_C99_METHOD_REGEXP_TEST && argc > 0) {
        if (kiln_c99_val_regexp_test_fast(receiver, args[0], out) == KILN_C99_OK) {
            return KILN_C99_OK;
        }
    } else if (method_code == KILN_C99_METHOD_REGEXP_EXEC && argc > 0) {
        if (kiln_c99_val_regexp_exec_fast(receiver, args[0], out) == KILN_C99_OK) {
            return KILN_C99_OK;
        }
    }

    fallback:

    kiln_value bself = kiln_val_to_boundary(receiver);
    kiln_value bargs[argc > 0 ? argc : 1];
    for (size_t bi = 0; bi < argc; ++bi) bargs[bi] = kiln_val_to_boundary(args[bi]);
    kiln_value bresult = kiln_c99_undefined();
    int status = kiln_c99_invoke_method(host, &bresult, method_code, bself, argc > 0 ? bargs : NULL, argc, depth, thrown);
    *out = kiln_val_from_boundary(bresult);
    return status;
}

/* A method call on machine values, end to end. The property is read once through the site's
 * inline cache on `kiln_val`; a plain strict or arrow closure that publishes no frame is entered
 * directly with the machine arguments; only a built-in method or an exotic callee crosses to the
 * boundary representation, and then only its arguments. This is the seam the shape and slot
 * migration left in front of every method call: the stack-shaped op below converts the whole
 * frame both ways and re-reads nothing, but it does convert. */
static int kiln_c99_val_call_method_view(kiln_host *host, kiln_val receiver, const kiln_val *args,
                                         size_t argc, const kiln_string *key,
                                         const kiln_string *canonical, int depth,
                                         kiln_value *thrown, kiln_c99_site *site,
                                         const char *inst_site, size_t inst_idx,
                                         const char *inst_op, kiln_val *out) {
    kiln_val method = 0;
    int from_cache = 0;
    if (KILN_LIKELY(kiln_c99_cached_method_matches(site, receiver))) {
        method = KILN_VAL_METHOD(site->cached_method);
        from_cache = 1;
    } else if (KILN_LIKELY(site != NULL && site->pic_count > 0 && KILN_VAL_IS_OBJECT(receiver))) {
        void *ptr = KILN_VAL_PTR(receiver);
        if (KILN_LIKELY(ptr != NULL && kiln_c99_gc_payload_kind(ptr) == KILN_GC_OBJECT)) {
            const kiln_object *object = (const kiln_object *)ptr;
            size_t p;
            (void)p;
            method = kiln_c99_site_cached_callee(site, object);
            if (method != 0) {
                from_cache = 1;
            }
        }
    }
    int status = KILN_C99_OK;
    if (!from_cache) {
        status = kiln_c99_val_read_property_view(host, receiver, key, canonical, depth, thrown,
                                                 NULL, &method);
        if (status == KILN_C99_CONTROL_ERROR) {
            if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(receiver).tag;
            if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = "CallMethod property lookup refused";
            kiln_c99_note_refusal(inst_site, inst_idx, inst_op, (size_t)kiln_c99_diagnostic_active_depth);
        } else if (status == KILN_C99_THROWN) {
            if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = "CallMethod property lookup threw";
            kiln_c99_note_refusal(inst_site, inst_idx, inst_op, (size_t)kiln_c99_diagnostic_active_depth);
        }
        if (status != KILN_C99_OK) return status;

        /* A runtime method found on an ordinary object -- `res.end` on a `node:http` response,
         * `emitter.on` -- is remembered where it lives, like any other prototype member, by the
         * shape branch below. The receiver-tag cache is for receivers whose tag alone decides
         * the method (a string, an array, a builtin namespace); `kiln_c99_cached_method_matches`
         * never matches an object, so recording one there only made every later call through
         * this site miss it first and then resolve the method by name again. */
        if (site != NULL && KILN_VAL_IS_METHOD(method) &&
            !(KILN_VAL_IS_OBJECT(receiver) &&
              kiln_c99_gc_kind_of_ptr(KILN_VAL_PTR(receiver)) == KILN_GC_OBJECT &&
              ((const kiln_object *)KILN_VAL_PTR(receiver))->shape != NULL)) {
            site->cached_tag = (uint8_t)kiln_val_to_boundary(receiver).tag;
            site->cached_method = KILN_VAL_METHOD_ID(method);
            site->has_cached_method = 1;
            if (KILN_VAL_IS_BUILTIN(receiver)) kiln_c99_site_stamp_builtin(site, KILN_VAL_BUILTIN_ID(receiver));
        } else if (site != NULL && KILN_VAL_IS_OBJECT(receiver) &&
                   /* A proxy, a generator and a collection are boxed with the object prefix and
                    * are not objects; a site learns a shape only from an object, so a hit -- a
                    * pointer compare against learned shapes -- cannot be one of theirs. The kind
                    * lookup is a heap search, affordable once per shape here and not per call. */
                   kiln_c99_gc_kind_of_ptr(KILN_VAL_PTR(receiver)) == KILN_GC_OBJECT) {
            const kiln_object *object = (const kiln_object *)KILN_VAL_PTR(receiver);
            if (object->shape != NULL) {
                /* An own property is remembered by its slot. A prototype's is remembered by
                 * where it lives, one or two steps up the chain, when the holder is a shaped
                 * object and the slot there holds exactly the callee just resolved -- a data
                 * property, not an accessor. Anything else is not remembered, and the site
                 * resolves it again next time. */
                uint32_t own_slot = UINT32_MAX;
                int own = kiln_c99_shape_find_slot(object->shape, key, &own_slot);
                kiln_c99_pic_entry remembered;
                size_t p;
                int found = 0;
                memset(&remembered, 0, sizeof remembered);
                remembered.shape = object->shape;
                remembered.slot = own ? own_slot : UINT32_MAX;
                if (!own) {
                    const kiln_object *via = NULL;
                    const kiln_object *step = NULL;
                    uint32_t slot = UINT32_MAX;
                    int steps;
                    kiln_value link = object->prototype;
                    const kiln_string *holder_key = NULL;
                    for (steps = 0; steps < 2 && link.tag == KILN_VALUE_OBJECT; ++steps) {
                        const kiln_object *candidate = (const kiln_object *)(uintptr_t)link.bits;
                        if (kiln_c99_gc_kind_of_ptr((void *)candidate) != KILN_GC_OBJECT) {
                            break;
                        }
                        if (candidate->shape != NULL) {
                            if (kiln_c99_shape_find_slot(candidate->shape, key, &slot)) {
                                if (slot < candidate->slot_capacity && candidate->slots[slot] == method) {
                                    step = candidate;
                                }
                                break;
                            }
                        } else {
                            /* A list-mode holder: the record is valid while it carries this
                             * key pointer, so only a record keyed by the interned pointer the
                             * site resolved with is remembered. An intermediate in list mode
                             * cannot promise it has not gained the key, so the walk stops. */
                            size_t i;
                            int seen = 0;
                            for (i = 0; i < candidate->count; ++i) {
                                const kiln_property *property = &candidate->properties[i];
                                if (property->key == key || kiln_string_equal(property->key, key)) {
                                    seen = 1;
                                    if (property->key == key && !property->accessor &&
                                        !property->refused &&
                                        kiln_val_from_boundary(property->value) == method) {
                                        step = candidate;
                                        slot = (uint32_t)i;
                                        holder_key = property->key;
                                        remembered.holder_value = property->value;
                                        remembered.holder_val = method;
                                    }
                                    break;
                                }
                            }
                            (void)seen;
                            break;
                        }
                        via = candidate;
                        link = candidate->prototype;
                    }
                    if (step != NULL) {
                        remembered.holder = step;
                        remembered.holder_shape = step->shape;
                        remembered.holder_key = holder_key;
                        remembered.holder_slot = slot;
                        remembered.epoch = kiln_c99_proto_epoch;
                        ((kiln_object *)step)->cache_holder = 1;
                        if (via != NULL) {
                            remembered.via = via;
                            remembered.via_shape = via->shape;
                            ((kiln_object *)via)->cache_holder = 1;
                        }
                    }
                }
                for (p = 0; p < site->pic_count; ++p) {
                    if (site->entries[p].shape == object->shape) {
                        site->entries[p] = remembered;
                        found = 1;
                        break;
                    }
                }
                if (!found) {
                    if (site->pic_count < KILN_C99_PIC_SIZE) {
                        site->entries[site->pic_count] = remembered;
                        site->pic_count += 1;
                    } else {
                        site->entries[KILN_C99_PIC_SIZE - 1] = remembered;
                    }
                }
                if (remembered.holder != NULL) kiln_c99_gc_register_site(site);
            }
        }
    }

    if (KILN_VAL_IS_CLOSURE(method)) {
        kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(method);
        if (!plain->generator && !kiln_c99_inheriting_new_target &&
            depth < KILN_C99_MAX_CALL_DEPTH) {
            kiln_val vreceiver = plain->arrow ? plain->captured_this : receiver;
            if (!plain->arrow && !plain->strict && (vreceiver == KILN_VAL_UNDEFINED || vreceiver == KILN_VAL_NULL)) {
                (void)kiln_c99_global_object_val(&vreceiver);
            }
            *out = KILN_VAL_UNDEFINED;
            status = kiln_c99_invoke_closure(host, out, plain, vreceiver, args, argc, depth, thrown, NULL, 0);
            goto done;
        }
    }
    if (KILN_VAL_IS_METHOD(method) && depth < KILN_C99_MAX_CALL_DEPTH) {
        status = kiln_c99_val_invoke_cached_method(host, KILN_VAL_METHOD_ID(method), receiver, args, argc, depth, thrown, out);
        goto done;
    }
    {
        /* A built-in method or an exotic callee: the boundary representation, arguments only. */
        kiln_value bmethod = kiln_val_to_boundary(method);
        kiln_value bself = kiln_val_to_boundary(receiver);
        kiln_value bargs[argc > 0 ? argc : 1];
        kiln_value bresult = kiln_c99_undefined();
        for (size_t i = 0; i < argc; ++i) bargs[i] = kiln_val_to_boundary(args[i]);
        if (bmethod.tag == KILN_VALUE_METHOD && depth < KILN_C99_MAX_CALL_DEPTH) {
            status = kiln_c99_invoke_method(host, &bresult, bmethod.bits, bself, argc > 0 ? bargs : NULL, argc, depth, thrown);
        } else {
            status = kiln_c99_call(host, &bresult, bmethod, bself, argc > 0 ? bargs : NULL, argc, depth, thrown);
        }
        *out = kiln_val_from_boundary(bresult);
    }
done:
    if (status == KILN_C99_CONTROL_ERROR) {
        if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(receiver).tag;
        if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = "CallMethod invocation refused";
        kiln_c99_note_refusal(inst_site, inst_idx, inst_op, (size_t)kiln_c99_diagnostic_active_depth);
    } else if (status == KILN_C99_THROWN) {
        if (kiln_c99_diagnostic_detail == NULL) {
            kiln_c99_diagnostic_receiver_tag = kiln_val_to_boundary(receiver).tag;
            kiln_c99_diagnostic_detail = "CallMethod invocation threw";
        }
        kiln_c99_note_refusal(inst_site, inst_idx, inst_op, (size_t)kiln_c99_diagnostic_active_depth);
    }
    return status;
}

static int kiln_c99_val_call_method_key(kiln_host *host, kiln_val receiver, const kiln_val *args,
                                        size_t argc, const kiln_string *key, int depth,
                                        kiln_value *thrown, kiln_c99_site *site,
                                        const char *inst_site, size_t inst_idx, const char *inst_op,
                                        kiln_val *out) {
    return kiln_c99_val_call_method_view(host, receiver, args, argc, key, key, depth, thrown, site,
                                         inst_site, inst_idx, inst_op, out);
}

/* Keep the byte-shaped entry point for handwritten/runtime callers. Generated code uses the
 * headed form above so a constant's storage header is never mistaken for its payload. */
static int kiln_c99_val_call_method(kiln_host *host, kiln_val receiver, const kiln_val *args,
                                    size_t argc, const char *key, size_t key_len, int depth,
                                    kiln_value *thrown, kiln_c99_site *site,
                                    const char *inst_site, size_t inst_idx, const char *inst_op,
                                    kiln_val *out) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_OUT_OF_MEMORY;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    return kiln_c99_val_call_method_view(host, receiver, args, argc, &view, NULL, depth, thrown,
                                         site, inst_site, inst_idx, inst_op, out);
}

/* A call on machine values. A plain closure is entered directly with the receiver the language
 * gives it: an arrow's captured `this`, the global object for sloppy code, `undefined` otherwise.
 * Anything else -- a built-in method, a generator, a bound or proxied callable -- crosses the
 * boundary once. A refusal or a throw is attributed to the instruction that made the call. */
static int kiln_c99_val_call(kiln_host *host, kiln_val callee, const kiln_val *args, size_t argc,
                             int depth, kiln_value *thrown, const char *inst_site, size_t inst_idx,
                             const char *inst_op, kiln_val *out) {
    kiln_closure *plain = KILN_VAL_IS_CLOSURE(callee) ? (kiln_closure *)KILN_VAL_PTR(callee) : NULL;
    if (plain != NULL && !plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH) {
        kiln_val call_result = KILN_VAL_UNDEFINED;
        kiln_val recv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED;
        int status;
        if (!plain->arrow && !plain->strict) (void)kiln_c99_global_object_val(&recv);
        status = kiln_c99_invoke_closure(host, &call_result, plain, recv, args, argc, depth, thrown, NULL, 0);
        if (status == KILN_C99_CONTROL_ERROR && (call_result & KILN_TAG_MASK) == KILN_TAG_STRING_PREFIX) {
            return status;
        }
        if (status == KILN_C99_CONTROL_ERROR) {
            if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = KILN_VALUE_FUNCTION;
            if (kiln_c99_diagnostic_detail == NULL) kiln_c99_diagnostic_detail = "Call invocation refused";
            kiln_c99_note_refusal(inst_site, inst_idx, inst_op, (size_t)kiln_c99_diagnostic_active_depth);
        } else if (status == KILN_C99_THROWN) {
            if (kiln_c99_diagnostic_detail == NULL) {
                kiln_c99_diagnostic_receiver_tag = KILN_VALUE_FUNCTION;
                kiln_c99_diagnostic_detail = "Call invocation threw";
            }
            kiln_c99_note_refusal(inst_site, inst_idx, inst_op, (size_t)kiln_c99_diagnostic_active_depth);
        }
        if (status != KILN_C99_OK) return status;
        *out = call_result;
        return KILN_C99_OK;
    }
    {
        kiln_value bcallee = kiln_val_to_boundary(callee);
        kiln_value bargs[argc > 0 ? argc : 1];
        kiln_value call_result = kiln_c99_undefined();
        int status;
        for (size_t i = 0; i < argc; ++i) bargs[i] = kiln_val_to_boundary(args[i]);
        if (bcallee.tag == KILN_VALUE_METHOD && depth < KILN_C99_MAX_CALL_DEPTH) {
            status = kiln_c99_invoke_method(host, &call_result, bcallee.bits, kiln_c99_undefined(), argc > 0 ? bargs : NULL, argc, depth, thrown);
        } else {
            status = kiln_c99_call(host, &call_result, bcallee, kiln_c99_undefined(), argc > 0 ? bargs : NULL, argc, depth, thrown);
        }
        if (status == KILN_C99_CONTROL_ERROR && call_result.tag == KILN_VALUE_STRING) {
            return status;
        }
        if (status == KILN_C99_CONTROL_ERROR) {
            if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = bcallee.tag;
            if (kiln_c99_diagnostic_site == NULL && kiln_c99_diagnostic_operation == NULL && kiln_c99_diagnostic_detail == NULL) {
                kiln_c99_diagnostic_detail = "Call callee is unsupported by the native callable model";
            }
            kiln_c99_note_refusal(inst_site, inst_idx, inst_op, (size_t)kiln_c99_diagnostic_active_depth);
        } else if (status == KILN_C99_THROWN) {
            if (kiln_c99_diagnostic_detail == NULL) {
                kiln_c99_diagnostic_receiver_tag = bcallee.tag;
                kiln_c99_diagnostic_detail = "Call invocation threw";
            }
            kiln_c99_note_refusal(inst_site, inst_idx, inst_op, (size_t)kiln_c99_diagnostic_active_depth);
        }
        if (status != KILN_C99_OK) return status;
        *out = kiln_val_from_boundary(call_result);
        return KILN_C99_OK;
    }
}

/* `new` on machine values: the constructor and its arguments cross the boundary once, and the
 * instance comes back as a machine value. A refusal names the built-in it was refused on. */
static int kiln_c99_val_new(kiln_host *host, kiln_val callee, const kiln_val *args, size_t argc,
                            int depth, kiln_value *thrown, kiln_val *out) {
    kiln_value bcallee = kiln_val_to_boundary(callee);
    kiln_value bargs[argc > 0 ? argc : 1];
    kiln_value built;
    int status;
    for (size_t i = 0; i < argc; ++i) bargs[i] = kiln_val_to_boundary(args[i]);
    status = kiln_c99_construct(host, &built, bcallee, argc > 0 ? bargs : NULL, argc, depth, thrown);
    if (status == KILN_C99_CONTROL_ERROR) {
        if (kiln_c99_diagnostic_receiver_tag == 0) kiln_c99_diagnostic_receiver_tag = bcallee.tag;
        if (bcallee.tag == KILN_VALUE_BUILTIN && kiln_c99_diagnostic_function == NULL &&
            kiln_c99_diagnostic_function_key == NULL) {
            const char *named = kiln_c99_builtin_name(bcallee.bits);
            if (named != NULL) {
                kiln_c99_diagnostic_function = named;
                kiln_c99_diagnostic_function_len = strlen(named);
            }
        }
        if (kiln_c99_diagnostic_detail == NULL) {
            kiln_c99_diagnostic_detail = "New target is unsupported by the native constructor model";
        }
    }
    if (status != KILN_C99_OK) return status;
    *out = kiln_val_from_boundary(built);
    return KILN_C99_OK;
}

/* `ToPropertyKey` on machine values, against the object it will index. */
static int kiln_c99_val_to_property_key(kiln_host *host, kiln_val target, kiln_val key,
                                        int depth, kiln_value *thrown, kiln_val *out) {
    kiln_value reduced;
    int status = kiln_c99_reduce_key(host, kiln_val_to_boundary(target), kiln_val_to_boundary(key), &reduced, depth, thrown);
    if (status != KILN_C99_OK) return status;
    *out = kiln_val_from_boundary(reduced);
    return KILN_C99_OK;
}

static int kiln_c99_val_check_tdz(kiln_val val, kiln_value *thrown) {
    if (val == KILN_VAL_UNINITIALIZED) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, "cannot access a binding before its declaration");
    }
    return KILN_C99_OK;
}

static int kiln_c99_val_make_cell(kiln_val *slot) {
    kiln_val *box = (kiln_val *)kiln_c99_gc_alloc_hot(KILN_GC_CELL, sizeof *box, 0);
    if (box == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    *box = *slot;
    *slot = KILN_VAL_TAG_PTR(KILN_TAG_CELL_PREFIX, box);
    return KILN_C99_OK;
}

/* This unit's own roots (ADR 0041): the symbols it made, the intrinsic prototypes it filled in
 * lazily, and the call-transient statics, which are marked rather than cleared so a stale read
 * after a collection still names live memory. Module-scope slots are marked by the emitted root
 * function that calls this. At teardown the statics are reset, because a later realm in the same
 * process must not find the previous realm's freed objects in them. */
static void kiln_c99_gc_unit_roots(void *opaque, void *context, int phase) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    size_t index;
    size_t constructors = sizeof kiln_c99_native_constructors / sizeof *kiln_c99_native_constructors;
    (void)context;
    if (phase == KILN_GC_PHASE_RESET) {
        free(kiln_c99_symbols);
        kiln_c99_symbols = NULL;
        kiln_c99_symbol_count = 0;
        kiln_c99_symbol_capacity = 0;
        for (index = 0; index < constructors; ++index)
            kiln_c99_native_constructors[index].prototype = kiln_c99_undefined();
        kiln_c99_array_unscopables = kiln_c99_undefined();
        kiln_c99_new_target = kiln_c99_undefined();
        kiln_c99_adopted_this = kiln_c99_undefined();
        kiln_c99_captured_arguments = kiln_c99_undefined();
        kiln_c99_frame_arguments_object = kiln_c99_undefined();
        kiln_c99_frame_callee = kiln_c99_undefined();
        return;
    }
    if (phase != KILN_GC_PHASE_MARK) return;
    kiln_c99_gc_mark_values(heap, kiln_c99_symbols, kiln_c99_symbol_count);
    for (index = 0; index < constructors; ++index)
        kiln_c99_gc_mark_value(heap, &kiln_c99_native_constructors[index].prototype);
    kiln_c99_gc_mark_value(heap, &kiln_c99_array_unscopables);
    kiln_c99_gc_mark_value(heap, &kiln_c99_new_target);
    kiln_c99_gc_mark_value(heap, &kiln_c99_adopted_this);
    kiln_c99_gc_mark_value(heap, &kiln_c99_captured_arguments);
    kiln_c99_gc_mark_value(heap, &kiln_c99_frame_arguments_object);
    kiln_c99_gc_mark_value(heap, &kiln_c99_frame_callee);
}
