
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
        kiln_c99_heap_copy_vals(array, array->elements, elements, count);
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
            if (KILN_VAL_IS_SHARED(v)) {
                return kiln_c99_value(KILN_VALUE_SHARED, (uint64_t)(uintptr_t)ptr, NULL, 0);
            }
            if (KILN_VAL_IS_TEMPORAL(v)) {
                return kiln_c99_value(KILN_VALUE_TEMPORAL, (uint64_t)(uintptr_t)ptr, NULL, 0);
            }
            if (KILN_VAL_IS_BIGINT(v)) {
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
                case KILN_GC_TIMER:             val_tag = KILN_VALUE_TIMER; break;
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
            if (KILN_LIKELY(!arr->has_index_descriptors && !arr->frozen && !arr->not_extensible)) {
                if (arr->is_numeric && !KILN_VAL_IS_NUMBER(arg)) {
                    arr->is_numeric = 0;
                }
                if (KILN_LIKELY(arr->count < arr->capacity)) {
                    kiln_c99_heap_store_val(arr, &arr->elements[arr->count++], arg);
                    *out = kiln_c99_val_from_double((double)arr->count);
                    return KILN_C99_OK;
                }
                int status = kiln_c99_array_reserve(arr, arr->count + 1);
                if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
                kiln_c99_heap_store_val(arr, &arr->elements[arr->count++], arg);
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
                        kiln_c99_heap_store_pointer(regexp, &regexp->cached_subject_ptr, sval.ptr);
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
                        status = kiln_c99_regexp_publish_legacy(sval, ranges, groups, matched);
                        if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
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
                        kiln_c99_heap_store_pointer(regexp, &regexp->cached_subject_ptr, sval.ptr);
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
                        status = kiln_c99_regexp_publish_legacy(sval, ranges, groups, matched);
                        if (KILN_UNLIKELY(status != KILN_C99_OK)) {
                            if (ranges != stack_ranges) free(ranges);
                            return status;
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
                            kiln_c99_heap_store_interior_pointer(arr, &arr->elements, offsetof(kiln_array, inline_elements));
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
                            kiln_c99_heap_copy_vals(arr, arr->elements, regexp->cached_elements, groups);
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
                                kiln_c99_heap_store_val(arr, &arr->elements[g], bval);
                                arr->count = g + 1;
                                if (g < 32) {
                                    kiln_c99_heap_store_val(regexp, &regexp->cached_elements[g], bval);
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
                        kiln_c99_heap_store_value(arr, &arr->match_input, sval);
                        {
                            int gstatus = kiln_c99_match_groups(regexp, arr);
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
    collection->weak_previous = NULL;
    collection->weak_next = NULL;
    collection->weak_owner = NULL;
    kiln_c99_heap_store_value(collection, &collection->properties, kiln_c99_undefined());
    kiln_c99_collection_register_weak(collection);
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

static inline double kiln_c99_val_collection_find_unleased(kiln_collection *collection, kiln_val key, uint64_t hash) {
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

static inline double kiln_c99_val_collection_find_fast(kiln_collection *collection, kiln_val key, uint64_t hash) {
    uint64_t lease = KILN_VAL_IS_SHARED(key)
        ? kiln_c99_collection_key_lease(kiln_val_to_boundary(key)) : 0;
    double at;
    if (KILN_VAL_IS_SHARED(key) && lease == 0) return -1.0;
    at = kiln_c99_val_collection_find_unleased(collection, key, hash);
    if (lease != 0) (void)kiln_shared_graph_release(lease);
    return at;
}

static int kiln_c99_collection_put_val_unleased(kiln_collection *collection, kiln_val key, kiln_val value) {
    uint64_t hash = kiln_c99_val_collection_hash(key);
    size_t empty_slot = (size_t)-1;
    if (collection->count > 0 && collection->hash_table != NULL && collection->hash_capacity > 0) {
        size_t mask = collection->hash_capacity - 1;
        size_t at = (size_t)hash & mask;
        while (collection->hash_table[at] != 0) {
            uint32_t idx = collection->hash_table[at] - 1;
            if (!collection->entries[idx].dead &&
                kiln_c99_val_collection_eq(collection->entries[idx].key, key)) {
                kiln_c99_heap_store_value(collection, &collection->entries[idx].value, kiln_val_to_boundary(value));
                return KILN_C99_OK;
            }
            at = (at + 1) & mask;
        }
        empty_slot = at;
    } else if (collection->count > 0 && collection->hash_table == NULL) {
        for (size_t index = 0; index < collection->count; ++index) {
            if (collection->entries[index].dead) continue;
            if (kiln_c99_val_collection_eq(collection->entries[index].key, key)) {
                kiln_c99_heap_store_value(collection, &collection->entries[index].value, kiln_val_to_boundary(value));
                return KILN_C99_OK;
            }
        }
    }
    int status = kiln_c99_reserve((void **)&collection->entries, &collection->capacity,
                                  collection->count + 1, sizeof *collection->entries);
    if (KILN_UNLIKELY(status != KILN_C99_OK)) return status;
    size_t new_idx = collection->count;
    kiln_c99_heap_store_value(collection, &collection->entries[new_idx].key, kiln_val_to_boundary(key));
    kiln_c99_heap_store_value(collection, &collection->entries[new_idx].value, kiln_val_to_boundary(value));
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

static int kiln_c99_collection_put_val_fast(kiln_collection *collection, kiln_val key, kiln_val value) {
    uint64_t lease = KILN_VAL_IS_SHARED(key)
        ? kiln_c99_collection_key_lease(kiln_val_to_boundary(key)) : 0;
    int status;
    if (KILN_VAL_IS_SHARED(key) && lease == 0) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_collection_put_val_unleased(collection, key, value);
    if (lease != 0) (void)kiln_shared_graph_release(lease);
    return status;
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
    kiln_c99_heap_store_value(collection, &collection->entries[(size_t)at].key, kiln_c99_undefined());
    kiln_c99_heap_store_value(collection, &collection->entries[(size_t)at].value, kiln_c99_undefined());
    kiln_c99_collection_compact_weak(collection);
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
            kiln_host call_host;
            kiln_call_context ctx_call = { kiln_c99_closure_host(ctx->host, plain, &call_host), ctx->thrown, NULL, ctx->depth + 1 };
            kiln_val recv = plain->arrow ? plain->captured_this : KILN_VAL_UNDEFINED;
            kiln_js_frame js_frame;
            kiln_c99_js_frame_enter(&js_frame, plain);
            status = plain->body(&ctx_call, &prod, plain->upvalues, &recv, args, (size_t)2);
            kiln_c99_js_frame_leave(&js_frame);
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
            kiln_c99_heap_store_val(array, &array->elements[live++], array->elements[index]);
        }
    }
    for (size_t index = live; index < array->count; ++index) {
        kiln_c99_heap_store_val(array, &array->elements[index], KILN_VAL_UNDEFINED);
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
            case 6:  return kiln_c99_val_from_double(kiln_c99_js_pow(a, b));
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

static inline int kiln_c99_site_member_matches_bytes(const kiln_c99_site *site,
                                                     const char *member, size_t len);

static int kiln_c99_read_property(kiln_host *host, kiln_value target,
                                  const char *key, size_t key_len,
                                  int depth, kiln_value *thrown,
                                  kiln_c99_site *site, kiln_value *out) {
    if (target.tag == KILN_VALUE_OBJECT && kiln_c99_object_of(target)->sealed != NULL)
        site = NULL;
    if (site != NULL) {
        if (KILN_LIKELY(target.tag == KILN_VALUE_OBJECT)) {
            const kiln_object *object = kiln_c99_object_of(target);
            const kiln_c99_pic_entry *entry =
                object->shape != NULL ? kiln_c99_site_entry(site, object->shape) : NULL;
            if (entry != NULL && entry->slot < object->slot_capacity) {
                *out = kiln_val_to_boundary(object->slots[entry->slot]);
                return KILN_C99_OK;
            }
        }
        if (KILN_LIKELY(kiln_c99_site_has_method(site) && target.tag == site->cached_tag)) {
            int builtin_key_matches = target.tag != KILN_VALUE_BUILTIN ||
                ((uint32_t)target.bits == site->cached_builtin &&
                 kiln_c99_site_member_matches_bytes(site, key, key_len) &&
                 kiln_c99_cached_builtin_member_ok(
                     site->cached_builtin, site->cached_member, site->cached_member_len));
            if (builtin_key_matches &&
                (target.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(target)->properties.tag == KILN_VALUE_UNDEFINED) &&
                (target.tag != KILN_VALUE_SHARED ||
                 kiln_c99_shared_cached_method_allowed(
                     (const kiln_shared_facade *)(uintptr_t)target.bits, site->cached_method))) {
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
    if (site != NULL && status == KILN_C99_OK && out->tag == KILN_VALUE_METHOD &&
        (target.tag != KILN_VALUE_BUILTIN || !kiln_c99_builtin_store_has_own((uint32_t)target.bits, key, key_len))) {
        if (target.tag == KILN_VALUE_STRING ||
            target.tag == KILN_VALUE_NUMBER ||
            target.tag == KILN_VALUE_BOOLEAN ||
            (target.tag == KILN_VALUE_ARRAY && kiln_c99_array_of(target)->properties.tag == KILN_VALUE_UNDEFINED) ||
            target.tag == KILN_VALUE_TYPED_ARRAY ||
            target.tag == KILN_VALUE_COLLECTION ||
            (target.tag == KILN_VALUE_SHARED &&
             kiln_c99_shared_cached_method_allowed(
                 (const kiln_shared_facade *)(uintptr_t)target.bits, out->bits)) ||
            target.tag == KILN_VALUE_BUILTIN) {
            kiln_c99_site_cache_method(site, (uint8_t)target.tag, out->bits,
                                       target.tag == KILN_VALUE_BUILTIN ? (uint32_t)target.bits : 0,
                                       key, key_len);
        }
    }
    return status;
}

static int kiln_c99_write_property(kiln_host *host, kiln_value target,
                                   const char *key, size_t key_len,
                                   kiln_value value, int strict,
                                   int depth, kiln_value *thrown,
                                   kiln_c99_site *site) {
    if (site != NULL && target.tag == KILN_VALUE_OBJECT) {
        kiln_object *object = kiln_c99_object_of(target);
        if (object->sealed != NULL)
            return kiln_c99_put_site(host, target, key, key_len, value, strict, depth,
                                     thrown, NULL);
        const kiln_c99_pic_entry *entry =
            object->shape != NULL ? kiln_c99_site_entry(site, object->shape) : NULL;
        kiln_c99_prototype_touched(object);
        if (entry != NULL) {
            /* A published entry never changes; these are its words. */
            const uint32_t slot = entry->slot;
            const kiln_shape *next_shape = entry->transition_to;
            if (next_shape == NULL) {
                if (KILN_LIKELY(slot < object->slot_capacity && !object->frozen)) {
                    if ((object->shape->double_slots_mask & (1u << slot)) &&
                        value.tag != KILN_VALUE_NUMBER) {
                        kiln_c99_ensure_dictionary(object);
                        return kiln_c99_put_site(host, target, key, key_len, value, strict, depth, thrown, site);
                    }
                    kiln_c99_heap_store_val(object, &object->slots[slot], kiln_val_from_boundary(value));
                    return KILN_C99_OK;
                }
            } else if (!object->not_extensible && slot < object->slot_capacity &&
                       kiln_c99_transition_store_clear(site, entry, object, NULL, NULL)) {
                /* A non-double written to a double slot falls through. */
                if (!((next_shape->double_slots_mask & (1u << slot)) &&
                      value.tag != KILN_VALUE_NUMBER)) {
                    kiln_c99_heap_store_val(object, &object->slots[slot], kiln_val_from_boundary(value));
                    object->shape = (kiln_shape *)next_shape;
                    return KILN_C99_OK;
                }
            }
        }
    }
    return kiln_c99_put_site(host, target, key, key_len, value, strict, depth, thrown, site);
}

#ifndef KILN_C99_CACHED_METHOD_MATCHES_DEFINED
#define KILN_C99_CACHED_METHOD_MATCHES_DEFINED
static inline int kiln_c99_site_member_matches_bytes(const kiln_c99_site *site,
                                                     const char *member, size_t len) {
    return site != NULL && member != NULL && len < sizeof site->cached_member &&
           site->cached_member_len == len && memcmp(site->cached_member, member, len) == 0;
}

static inline int kiln_c99_site_member_matches_key(const kiln_c99_site *site,
                                                   const kiln_string *key) {
    if (site == NULL || key == NULL || key->unit_len >= sizeof site->cached_member ||
        site->cached_member_len != key->unit_len) return 0;
    for (size_t index = 0; index < key->unit_len; ++index) {
        uint32_t unit = kiln_string_unit_at(key, (uint32_t)index);
        if (unit >= 0x80u || (unsigned char)site->cached_member[index] != unit) return 0;
    }
    return 1;
}

static inline void kiln_c99_site_cache_method_key(kiln_c99_site *site, uint8_t tag,
                                                  uint64_t method, uint32_t builtin,
                                                  const kiln_string *key) {
    char member[32];
    size_t len = key == NULL ? sizeof member : key->unit_len;
    if (tag != KILN_VALUE_BUILTIN) {
        kiln_c99_site_cache_method(site, tag, method, builtin, NULL, 0);
        return;
    }
    if (len >= sizeof member) {
        kiln_c99_site_cache_method(site, tag, method, builtin, NULL, sizeof member);
        return;
    }
    for (size_t index = 0; index < len; ++index) {
        uint32_t unit = kiln_string_unit_at(key, (uint32_t)index);
        if (unit >= 0x80u) {
            kiln_c99_site_cache_method(site, tag, method, builtin, NULL, sizeof member);
            return;
        }
        member[index] = (char)unit;
    }
    /* The method reached here came from the member table only if the realm's store lacked the
     * name; one that came out of the store (`Math.sqrt = Math.floor`) is that realm's alone, and
     * remembering it would hand it to every realm that reads through this site. */
    if (kiln_c99_builtin_store_has_own(builtin, member, len)) return;
    kiln_c99_site_cache_method(site, tag, method, builtin, member, len);
}

static inline int kiln_c99_cached_method_matches(const kiln_c99_site *site, kiln_val target,
                                                 const kiln_string *key) {
    if (site == NULL || !kiln_c99_site_has_method(site)) return 0;
    uint8_t ctag = site->cached_tag;
    /* Issue 30: a kind's remembered method is its table's, which a member the program wrote onto
     * the kind's prototype shadows. One compare while no realm has written onto a built-in. */
    if (ctag != KILN_VALUE_BUILTIN && KILN_UNLIKELY(!kiln_c99_tag_method_pristine(ctag, site->cached_method))) return 0;
    if (ctag == KILN_VALUE_COLLECTION && (KILN_VAL_IS_OBJECT(target) || ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
        void *ptr = KILN_VAL_PTR(target);
        if (kiln_c99_gc_kind_of_ptr(ptr) != KILN_GC_COLLECTION) return 0;
        const kiln_collection *collection = (const kiln_collection *)ptr;
        const char *method = kiln_c99_method_name(site->cached_method);
        return kiln_c99_collection_cached_method_allowed(collection, key, site->cached_method,
                                                         method);
    } else if (ctag == KILN_VALUE_SHARED && KILN_VAL_IS_SHARED(target)) {
        return kiln_c99_shared_cached_method_allowed(
            (const kiln_shared_facade *)KILN_VAL_PTR(target), site->cached_method);
    } else if (ctag == KILN_VALUE_STRING && KILN_VAL_IS_STRING(target)) {
        return 1;
    } else if (ctag == KILN_VALUE_NUMBER && KILN_VAL_IS_NUMBER(target)) {
        return 1;
    } else if (ctag == KILN_VALUE_BOOLEAN && KILN_VAL_IS_BOOLEAN(target)) {
        return 1;
    } else if (ctag == KILN_VALUE_BUILTIN) {
        if (!kiln_c99_site_member_matches_key(site, key) ||
            !kiln_c99_cached_builtin_member_ok(
                site->cached_builtin, site->cached_member, site->cached_member_len)) return 0;
        return KILN_VAL_IS_BUILTIN(target) &&
               KILN_VAL_BUILTIN_ID(target) == site->cached_builtin;
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

#ifdef KILN_IC_CHECK
/* See the declaration in the header. The pure read cannot run a getter; an absent path proved
 * there is none, and a status other than OK here means that proof no longer holds. */
static void kiln_c99_pic_absent_oracle(const kiln_object *obj, const kiln_string *key) {
    kiln_value slow = kiln_c99_undefined();
    kiln_value thrown = kiln_c99_undefined();
    kiln_value target = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)obj, NULL, 0);
    int status;
    if (key == NULL) return;
    status = kiln_c99_get_key(NULL, target, key, key, &slow, 0, &thrown);
    if (status != KILN_C99_OK || slow.tag != KILN_VALUE_UNDEFINED) {
        fputs("kiln: inline-cache oracle: a remembered absent path answered undefined for a present property\n", stderr);
        __builtin_trap();
    }
}
#endif

static int kiln_c99_val_read_property_view(kiln_host *host, kiln_val target,
                                           const kiln_string *key,
                                           const kiln_string *canonical, int depth,
                                           kiln_value *thrown, kiln_c99_site *site,
                                           kiln_val *out) {
    /* Namespace properties are live export cells, not values. Keep both shape/PIC reads and
     * cached-method answers out of this wrapper so the semantic path can unwrap the binding. */
    if (KILN_VAL_IS_OBJECT(target)) {
        void *ptr = KILN_VAL_PTR(target);
        if (ptr != NULL && kiln_c99_gc_payload_kind(ptr) == KILN_GC_OBJECT &&
            ((const kiln_object *)ptr)->module_namespace) {
            goto generic_read;
        }
    }
    if (KILN_LIKELY(site != NULL && KILN_VAL_IS_OBJECT(target))) {
        void *ptr = KILN_VAL_PTR(target);
        if (KILN_LIKELY(ptr != NULL && kiln_c99_gc_payload_kind(ptr) == KILN_GC_OBJECT)) {
            const kiln_object *object = (const kiln_object *)ptr;
            const kiln_c99_pic_entry *entry =
                object->shape != NULL ? kiln_c99_site_entry(site, object->shape) : NULL;
            if (entry != NULL) {
                if (entry->slot < object->slot_capacity) {
                    *out = object->slots[entry->slot];
                    return KILN_C99_OK;
                }
                if (kiln_c99_pic_inherited(entry, object, out)) return KILN_C99_OK;
            }
        }
    }
    if (KILN_LIKELY(kiln_c99_cached_method_matches(site, target, key))) {
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
                        /* An own accessor: the generic read below calls it. */
                        if (object->shape->accessor_mask & (1u << own_slot)) goto generic_read;
                        if (own_slot < object->slot_capacity) {
                            *out = object->slots[own_slot];
                            if (site != NULL) kiln_c99_site_remember_own(site, object->shape, own_slot);
                            return KILN_C99_OK;
                        }
                    }
                    if (object->properties == NULL && site != NULL) {
                        /* A member one or two steps up the chain: remembered as the path to it
                         * (ADR 0071), each level named by shape or layout token, and read live. */
                        kiln_c99_pic_entry remembered;
                        const kiln_object *holder = NULL;
                        memset(&remembered, 0, sizeof remembered);
                        remembered.shape = object->shape;
                        remembered.slot = UINT32_MAX;
                        if (kiln_c99_pic_learn_path(&remembered, object, stable, &holder) ==
                                KILN_C99_PATH_FOUND &&
                            kiln_c99_pic_inherited(&remembered, object, out)) {
                            kiln_c99_site_remember(site, &remembered);
                            return KILN_C99_OK;
                        }
                    }
                }
            }
        }
    } else if (KILN_VAL_IS_STRING(target)) {
        if (key != NULL && key->unit_len == 6 && kiln_c99_same_name_key(key, "length")) {
            if (site != NULL) kiln_c99_site_set_hint(site, KILN_VALUE_STRING);
            const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(target);
            *out = kiln_c99_val_from_double((double)(s != NULL ? s->unit_len : 0));
            return KILN_C99_OK;
        }
    } else if ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {
        void *ptr = KILN_VAL_PTR(target);
        /* The block's own kind, not the site's memory of one: a site that saw an array's
         * `length` also sees typed arrays, whose block is not a `kiln_array` (it read 0). */
        if (site != NULL && kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {
            if (key != NULL && key->unit_len == 6 && kiln_c99_same_name_key(key, "length")) {
                kiln_c99_site_set_hint(site, KILN_VALUE_ARRAY);
                const kiln_array *arr = (const kiln_array *)ptr;
                *out = kiln_c99_val_from_double((double)arr->count);
                return KILN_C99_OK;
            }
        }
    }
generic_read:;
    kiln_value t = kiln_val_to_boundary(target);
    kiln_value o = kiln_c99_undefined();
    int status = kiln_c99_get_site(host, t, key, canonical, &o, depth, thrown, site);
    if (status == KILN_C99_OK) {
        *out = kiln_val_from_boundary(o);
        if (o.tag == KILN_VALUE_UNDEFINED && site != NULL && KILN_VAL_IS_OBJECT(target) &&
            kiln_c99_key_index_key(key) < 0.0) {
            void *ptr = KILN_VAL_PTR(target);
            if (ptr != NULL && kiln_c99_gc_payload_kind(ptr) == KILN_GC_OBJECT) {
                const kiln_object *object = (const kiln_object *)ptr;
                if (object->shape != NULL && object->properties == NULL && object->elements == NULL) {
                    /* Absent everywhere the lookup looked: remembered as the path that proves it
                     * -- no level up to where the chain ends has the key -- when that is at most
                     * two steps; an own property holding undefined is remembered by its slot. */
                    const kiln_string *stable = canonical != NULL ? canonical : kiln_c99_interned_key(key);
                    uint32_t own_slot = UINT32_MAX;
                    kiln_c99_pic_entry remembered;
                    const kiln_object *holder = NULL;
                    memset(&remembered, 0, sizeof remembered);
                    remembered.shape = object->shape;
                    remembered.slot = UINT32_MAX;
                    if (stable != NULL && !kiln_c99_shape_find_slot(object->shape, stable, &own_slot) &&
                        kiln_c99_pic_learn_path(&remembered, object, stable, &holder) ==
                            KILN_C99_PATH_ABSENT) {
                        kiln_val check;
                        if (kiln_c99_pic_inherited(&remembered, object, &check)) {
                            kiln_c99_site_remember(site, &remembered);
                        }
                    }
                }
            }
        }
    }
    return status;
}

static const kiln_string *kiln_c99_compiler_named_key(const kiln_string *key) {
    static const struct { const char *name; size_t length; } symbols[] = {
        {"@@iterator", sizeof "@@iterator" - 1},
        {"@@asyncIterator", sizeof "@@asyncIterator" - 1},
        {"@@dispose", sizeof "@@dispose" - 1},
        {"@@asyncDispose", sizeof "@@asyncDispose" - 1}
    };
    size_t index;
    if (key == NULL || kiln_c99_key_is_symbol(key)) return key;
    /* Compiler-reserved names all begin with two ASCII at-sign code units. Most property
     * accesses are ordinary names, so reject them before checking the private prefix and the
     * fixed well-known-symbol table. Use code-unit access rather than inspecting the backing
     * bytes: callers may supply Latin-1 or UTF-16 property keys. */
    if (key->unit_len < sizeof "@@dispose" - 1 ||
        kiln_string_unit_at(key, 0) != '@' ||
        kiln_string_unit_at(key, 1) != '@') return key;
    if (key->unit_len >= sizeof "@@private:" - 1 &&
        kiln_c99_key_has_prefix(key, "@@private:")) {
        kiln_string private_key = *key;
        private_key.flags = (private_key.flags & ~KILN_STRING_SYMBOL_KEY) |
                            KILN_STRING_PRIVATE_KEY;
        return kiln_c99_intern_static_key(&private_key);
    }
    for (index = 0; index < sizeof symbols / sizeof *symbols; ++index) {
        if (key->unit_len == symbols[index].length &&
            kiln_c99_key_has_prefix(key, symbols[index].name)) {
            kiln_string symbol = *key;
            symbol.flags = (symbol.flags & ~KILN_STRING_PRIVATE_KEY) | KILN_STRING_SYMBOL_KEY;
            return kiln_c99_intern_static_key(&symbol);
        }
    }
    return key;
}

static int kiln_c99_val_read_property_key(kiln_host *host, kiln_val target,
                                          const kiln_string *key, int depth,
                                          kiln_value *thrown, kiln_c99_site *site,
                                          kiln_val *out) {
    /* Named @@ accesses are emitted only by the compiler's iterator/disposal protocols.
     * A program's `obj["@@iterator"]` uses the computed path and keeps the string domain. */
    key = kiln_c99_compiler_named_key(key);
    if (key == NULL) return KILN_C99_OUT_OF_MEMORY;
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
    if (KILN_LIKELY(site != NULL && KILN_VAL_IS_OBJECT(target))) {
        void *ptr = KILN_VAL_PTR(target);
        if (KILN_LIKELY(ptr != NULL && kiln_c99_gc_payload_kind(ptr) == KILN_GC_OBJECT)) {
            kiln_object *object = (kiln_object *)ptr;
            const kiln_c99_pic_entry *entry =
                object->shape != NULL ? kiln_c99_site_entry(site, object->shape) : NULL;
            kiln_c99_prototype_touched(object);
            if (entry != NULL) {
                /* A published entry never changes; these are its words. */
                const uint32_t slot = entry->slot;
                const kiln_shape *next_shape = entry->transition_to;
                if (next_shape == NULL) {
                    if (KILN_LIKELY(slot < object->slot_capacity && !object->frozen)) {
                        if ((object->shape->double_slots_mask & (1u << slot)) &&
                            !KILN_VAL_IS_NUMBER(value)) {
                            kiln_c99_ensure_dictionary(object);
                            kiln_value t = kiln_val_to_boundary(target);
                            kiln_value v = kiln_val_to_boundary(value);
                            return kiln_c99_put_site_key(host, t, key, canonical, v, strict,
                                                         depth, thrown, site);
                        }
                        kiln_c99_heap_store_val(object, &object->slots[slot], value);
                        return KILN_C99_OK;
                    }
                } else if (!object->not_extensible && slot < object->slot_capacity &&
                           kiln_c99_transition_store_clear(
                               site, entry, object,
                               canonical != NULL ? canonical : kiln_c99_interned_key(key), key)) {
                    /* A non-double written to a double slot falls through. */
                    if (!((next_shape->double_slots_mask & (1u << slot)) &&
                          !KILN_VAL_IS_NUMBER(value))) {
                        kiln_c99_heap_store_val(object, &object->slots[slot], value);
                        object->shape = (kiln_shape *)next_shape;
                        return KILN_C99_OK;
                    }
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
                if (kiln_c99_shape_find_slot(object->shape, stable, &slot) &&
                    (object->shape->store_mask & (1u << slot)) == 0) {
                    if (slot < object->slot_capacity && !object->frozen) {
                        if ((object->shape->double_slots_mask & (1u << slot)) && !KILN_VAL_IS_NUMBER(value)) {
                            kiln_c99_ensure_dictionary(object);
                        } else {
                            kiln_c99_prototype_touched(object);
                            kiln_c99_heap_store_val(object, &object->slots[slot], value);
                            if (site != NULL) kiln_c99_site_remember_own(site, object->shape, slot);
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
    key = kiln_c99_compiler_named_key(key);
    if (key == NULL) return KILN_C99_OUT_OF_MEMORY;
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

static void kiln_c99_gc_register_private_site(kiln_c99_private_site *site);

static void kiln_c99_private_site_learn(kiln_c99_private_site *site, const kiln_shape *shape,
                                        uint32_t brand_slot, uint32_t member_slot) {
    if (kiln_c99_slot_key(&site->shape) == shape) return;
    /* Registered before the entry is published: a teardown that frees the shape then finds the
     * site, whatever happens between the two. */
    if (!shape->shared) {
        kiln_c99_gc_register_private_site(site);
        if (__atomic_load_n(&site->registered, __ATOMIC_RELAXED) == 0) return;
    }
    if (!kiln_c99_slot_claim(&site->shape, 1)) return;
    site->brand_slot = brand_slot;
    site->member_slot = member_slot;
    kiln_c99_slot_publish(&site->shape, shape);
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
    if (site != NULL) kiln_c99_private_site_learn(site, obj->shape, bslot, mslot);
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
    /* A private method is read-only: the slow path refuses the write. */
    if ((obj->shape->store_mask & (1u << mslot)) != 0) {
        return 0;
    }
    if (site != NULL) kiln_c99_private_site_learn(site, obj->shape, bslot, mslot);
    kiln_c99_prototype_touched(obj);
    kiln_c99_heap_store_val(obj, &obj->slots[mslot], member);
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
            if (KILN_LIKELY(!array->has_index_descriptors && slot < array->count &&
                            array->elements[slot] != KILN_VAL_HOLE)) {
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
            if (KILN_LIKELY(!array->has_index_descriptors && !array->frozen &&
                            slot < array->count)) {
                kiln_c99_heap_store_val(array, &array->elements[slot], kiln_val_from_boundary(value));
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
                kiln_c99_heap_store_val(object, &object->elements[slot], kiln_val_from_boundary(value));
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
                if (KILN_LIKELY(!array->has_index_descriptors && slot < array->count &&
                                array->elements[slot] != KILN_VAL_HOLE)) {
                    *out = array->elements[slot];
                    return KILN_C99_OK;
                }
            } else if (gc_kind == KILN_GC_TYPED_ARRAY) {
                const kiln_typed_array *view = (const kiln_typed_array *)ptr;
                const kiln_typed_buffer *buffer = view->buffer;
                /* Number indices on ordinary fixed numeric views need no property-key string.
                 * Keep BigInt allocation, shared storage, detached/out-of-bounds views
                 * and all coercible keys on the authoritative integer-indexed exotic path. */
                if (buffer != NULL && !buffer->detached && buffer->shared == NULL &&
                    !kiln_c99_typed_kind_is_bigint(view->kind) &&
                    slot < view->length && view->byte_offset <= buffer->byte_length) {
                    size_t width = kiln_c99_typed_width(view->kind);
                    size_t remaining = buffer->byte_length - view->byte_offset;
                    if (slot < remaining / width) {
                        *out = kiln_c99_val_from_double(kiln_typed_load(view, slot));
                        return KILN_C99_OK;
                    }
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
            /* Namespace slots hold live export cells internally. The generic computed-property
             * path unwraps those cells (and checks their TDZ); returning the shape slot here
             * would expose the cell as a JavaScript value. */
            if (obj->shape != NULL && obj->accessor_count == 0 && !obj->module_namespace) {
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
            } else if (obj->properties != NULL && obj->accessor_count == 0 &&
                       obj->prototype.tag == KILN_VALUE_NULL && !obj->module_namespace) {
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
                if (KILN_LIKELY(!array->has_index_descriptors && !array->frozen &&
                                slot < array->count)) {
                    if (array->is_numeric && !KILN_VAL_IS_NUMBER(value)) {
                        array->is_numeric = 0;
                    }
                    kiln_c99_heap_store_val(array, &array->elements[slot], value);
                    return KILN_C99_OK;
                }
            } else if (gc_kind == KILN_GC_OBJECT) {
                kiln_object *object = (kiln_object *)ptr;
                if (KILN_LIKELY(object->shape != NULL && !object->frozen && object->elements != NULL &&
                                slot < object->element_count && object->elements[slot] != KILN_VAL_HOLE)) {
                    kiln_c99_heap_store_val(object, &object->elements[slot], value);
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

/* ---- Keyed computed-property reads (realms issue 17) ---------------------------------------
 *
 * `o[key]` with a symbol or a non-index string can use the same shape, slot and path caches as a
 * named access when the receiver is an ordinary shaped object. A computed site's key is not a
 * constant: every entry it learns is `keyed` -- it answers for a receiver shape *and* the
 * canonical key it was learned under, both compared on every hit -- and the site is found by the
 * pair (`kiln_c99_site_entry_keyed`). Numeric strings, reserved/internal keys, dictionary objects
 * and exotic receivers continue through the general element path.
 *
 * Realms (ADR 0071). A well-known or registered symbol's key is shared, so its entries serve every
 * realm as a constant name's do. A `Symbol()` key is realm-local: an entry naming it registers the
 * site with its realm's heap and is forgotten before the realm frees the key, and no other realm
 * holds that pointer meanwhile -- which is also why such an entry may give a receiver the
 * realm-local shape a store moves it to, even from a shared shape (`kiln_c99_site_remember`).
 *
 * A computed site carries one pointer for this (`kiln_c99_symbol_cell`), and the site behind it
 * is allocated the first time the site learns a keyed answer, published by
 * compare-and-swap and never freed, like the static sites beside it. Most computed sites do not
 * need a keyed cache, and pay one word. */
static __attribute__((noinline)) kiln_c99_site *kiln_c99_key_site(kiln_c99_symbol_cell *cell) {
    kiln_c99_site *site = __atomic_load_n(cell, __ATOMIC_ACQUIRE);
    kiln_c99_site *made;
    if (site != NULL) return site;
    made = (kiln_c99_site *)calloc(1, sizeof *made);
    if (made == NULL) return NULL;
    if (!__atomic_compare_exchange_n(cell, &site, made, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
        free(made);
        return site;
    }
    return made;
}

/* Remember `given` (its receiver shape and answer filled in) as a keyed entry for `ikey`. */
static void kiln_c99_key_site_remember(kiln_c99_symbol_cell *cell, kiln_c99_pic_entry *given,
                                       const kiln_string *ikey) {
    kiln_c99_site *site = kiln_c99_key_site(cell);
    if (site == NULL) return;
    given->key = ikey;
    given->keyed = 1;
    kiln_c99_site_remember(site, given);
}

/* The shaped, ordinary object a keyed access may answer from its site, or NULL. */
static inline __attribute__((always_inline)) kiln_object *kiln_c99_keyed_receiver(kiln_val target) {
    kiln_object *object;
    if (!KILN_VAL_IS_OBJECT(target)) return NULL;
    object = (kiln_object *)KILN_VAL_PTR(target);
    if (object == NULL || kiln_c99_gc_payload_kind(object) != KILN_GC_OBJECT ||
        object->shape == NULL)
        return NULL;
    return object;
}

/* A keyed read the site did not answer: learn what it can, else the element read. */
static __attribute__((noinline)) int kiln_c99_val_read_key_miss(kiln_host *host, kiln_val target,
                                                                kiln_val key,
                                                                const kiln_string *ikey, int depth,
                                                                kiln_value *thrown,
                                                                kiln_c99_symbol_cell *cell,
                                                                kiln_val *out) {
    kiln_object *object = kiln_c99_keyed_receiver(target);
    int status;
    if (object == NULL) return kiln_c99_val_read_element(host, target, key, depth, thrown, out);
    if (KILN_UNLIKELY(ikey == NULL)) return kiln_c99_val_read_element(host, target, key, depth, thrown, out);
    {
        uint32_t slot;
        kiln_c99_pic_entry remembered;
        const kiln_object *holder = NULL;
        if (kiln_c99_shape_find_slot(object->shape, ikey, &slot)) {
            /* An own accessor: reading it is a call, the element read's. */
            if ((object->shape->accessor_mask & (1u << slot)) == 0 && slot < object->slot_capacity) {
                *out = object->slots[slot];
                memset(&remembered, 0, sizeof remembered);
                remembered.shape = object->shape;
                remembered.slot = slot;
                kiln_c99_key_site_remember(cell, &remembered, ikey);
                return KILN_C99_OK;
            }
            return kiln_c99_val_read_element(host, target, key, depth, thrown, out);
        }
        /* A member one or two steps up, as a path (ADR 0071). */
        if (object->properties == NULL) {
            memset(&remembered, 0, sizeof remembered);
            remembered.shape = object->shape;
            remembered.slot = UINT32_MAX;
            if (kiln_c99_pic_learn_path_as(&remembered, object, ikey, &holder, 1) ==
                    KILN_C99_PATH_FOUND &&
                kiln_c99_pic_inherited(&remembered, object, out)) {
                kiln_c99_key_site_remember(cell, &remembered, ikey);
                return KILN_C99_OK;
            }
        }
    }
    status = kiln_c99_val_read_element(host, target, key, depth, thrown, out);
    /* Absent everywhere the read looked: remembered as the path that proves it, when that is at
     * most two steps (a getter that ran may have changed the object, so it is asked again). */
    if (status == KILN_C99_OK && *out == KILN_VAL_UNDEFINED && object->shape != NULL &&
        object->properties == NULL) {
        uint32_t own_slot;
        kiln_c99_pic_entry remembered;
        const kiln_object *holder = NULL;
        kiln_val check;
        memset(&remembered, 0, sizeof remembered);
        remembered.shape = object->shape;
        remembered.slot = UINT32_MAX;
        if (!kiln_c99_shape_find_slot(object->shape, ikey, &own_slot) &&
            kiln_c99_pic_learn_path_as(&remembered, object, ikey, &holder, 1) ==
                KILN_C99_PATH_ABSENT &&
            kiln_c99_pic_inherited(&remembered, object, &check)) {
            kiln_c99_key_site_remember(cell, &remembered, ikey);
        }
    }
    return status;
}

/* A symbol-keyed write the site did not answer: learn what it can, else the element write. */
static __attribute__((noinline)) int kiln_c99_val_write_symbol_miss(kiln_host *host,
                                                                    kiln_val target, kiln_val key,
                                                                    kiln_val value, int strict,
                                                                    int depth, kiln_value *thrown,
                                                                    kiln_c99_symbol_cell *cell) {
    kiln_object *object = kiln_c99_keyed_receiver(target);
    const kiln_string *ikey;
    const kiln_shape *before;
    int status;
    if (object == NULL || object->frozen || object->module_namespace)
        return kiln_c99_val_write_element(host, target, key, value, strict, depth, thrown);
    ikey = kiln_c99_symbol_ikey((const kiln_symbol *)KILN_VAL_PTR(key));
    if (KILN_UNLIKELY(ikey == NULL))
        return kiln_c99_val_write_element(host, target, key, value, strict, depth, thrown);
    kiln_c99_prototype_touched(object);
    {
        uint32_t slot;
        if (kiln_c99_shape_find_slot(object->shape, ikey, &slot) &&
            (object->shape->store_mask & (1u << slot)) == 0 && slot < object->slot_capacity &&
            ((object->shape->double_slots_mask & (1u << slot)) == 0 || KILN_VAL_IS_NUMBER(value))) {
            kiln_c99_pic_entry remembered;
            kiln_c99_heap_store_val(object, &object->slots[slot], value);
            memset(&remembered, 0, sizeof remembered);
            remembered.shape = object->shape;
            remembered.slot = slot;
            kiln_c99_key_site_remember(cell, &remembered, ikey);
            return KILN_C99_OK;
        }
    }
    before = object->shape;
    status = kiln_c99_val_write_element(host, target, key, value, strict, depth, thrown);
    /* Only the one edge this write took: a setter that ran instead may have moved the object
     * along some other key. */
    if (status == KILN_C99_OK && before != NULL && object->shape != NULL &&
        object->shape != before && object->shape->parent == before && object->shape->ikey == ikey &&
        (object->shape->attrs[object->shape->slot_index] == 0)) {
        kiln_c99_pic_entry remembered;
        memset(&remembered, 0, sizeof remembered);
        remembered.shape = before;
        remembered.transition_to = object->shape;
        remembered.slot = object->shape->slot_index;
        kiln_c99_key_site_remember(cell, &remembered, ikey);
    }
    return status;
}

/* `target[key]` where `key` is a symbol: the site's answer, or the miss above. Anything but a
 * shaped ordinary object, an accessor, and a miss the site cannot state are the element read's,
 * exactly as before. The hit is kept small enough to need no frame: a program's symbol carries
 * its canonical key, and only a well-known one asks for it out of line. */
static int kiln_c99_val_read_symbol(kiln_host *host, kiln_val target, kiln_val key, int depth,
                                    kiln_value *thrown, kiln_c99_symbol_cell *cell, kiln_val *out) {
    const kiln_c99_site *site = KILN_C99_LOAD_ACQUIRE(cell);
    const kiln_string *ikey = KILN_VAL_IS_OBJECT(target)
                                  ? kiln_c99_symbol_ikey((const kiln_symbol *)KILN_VAL_PTR(key))
                                  : NULL;
    if (KILN_LIKELY(site != NULL && KILN_VAL_IS_OBJECT(target))) {
        const kiln_object *object = (const kiln_object *)KILN_VAL_PTR(target);
        if (KILN_LIKELY(kiln_c99_gc_payload_kind(object) == KILN_GC_OBJECT &&
                        object->shape != NULL && ikey != NULL)) {
            const kiln_c99_pic_entry *entry =
                kiln_c99_site_entry_keyed(site, object->shape, ikey);
            if (KILN_LIKELY(entry != NULL)) {
                if (KILN_LIKELY(entry->slot < object->slot_capacity)) {
                    *out = object->slots[entry->slot];
                    return KILN_C99_OK;
                }
                if (kiln_c99_pic_inherited(entry, object, out)) return KILN_C99_OK;
            }
        }
    }
    return kiln_c99_val_read_key_miss(host, target, key, ikey, depth, thrown, cell, out);
}

/* Computed string keys share the keyed shape site, but numeric-looking and reserved keys retain
 * the indexed/internal semantics of the ordinary element path. */
int kiln_c99_val_read_string_key(kiln_host *host, kiln_val target, kiln_val key, int depth,
                                 kiln_value *thrown, kiln_c99_symbol_cell *cell, kiln_val *out) {
    const kiln_string *keystr;
    const kiln_string *ikey;
    const kiln_c99_site *site;
    kiln_object *object;
    if (!KILN_VAL_IS_STRING(key))
        return kiln_c99_val_read_element(host, target, key, depth, thrown, out);
    keystr = (const kiln_string *)KILN_VAL_PTR(key);
    if (keystr == NULL || kiln_c99_key_index_key(keystr) >= 0.0 ||
        kiln_c99_key_is_reserved(keystr))
        return kiln_c99_val_read_element(host, target, key, depth, thrown, out);
    object = kiln_c99_keyed_receiver(target);
    /* Namespace objects expose live export cells, and dictionary-mode objects do not have a
     * stable shape slot. Keep both on the authoritative computed-property path. */
    if (object == NULL || object->properties != NULL || object->module_namespace)
        return kiln_c99_val_read_element(host, target, key, depth, thrown, out);
    site = KILN_C99_LOAD_ACQUIRE(cell);
    if (site != NULL) {
        uint32_t p;
        uint32_t count = kiln_c99_site_count(site);
        for (p = 0; p < count; ++p) {
            const kiln_c99_pic_entry *entry = &site->entries[p];
            const kiln_shape *published_shape = kiln_c99_slot_key(&entry->shape);
            uint32_t slot;
            const kiln_string *shape_key;

            /* Compare the receiver shape before reading any other entry word. A keyed entry may
             * name a realm-local key and is retired before that realm frees it. Restrict this
             * allocation-free content check to an own data slot, and prove its key pointer is
             * owned by the live receiver shape before looking at its spelling. Inherited,
             * absent, and transition entries continue through the canonical pointer lookup below. */
            if (published_shape != object->shape || !entry->keyed || entry->key == NULL ||
                entry->transition_to != NULL)
                continue;
            slot = entry->slot;
            if (slot >= object->shape->property_count || slot >= object->slot_capacity ||
                (object->shape->accessor_mask & (1u << slot)) != 0)
                continue;
            shape_key = object->shape->keys[slot];
            if (shape_key == NULL || entry->key != shape_key ||
                kiln_c99_key_domain(shape_key) != kiln_c99_key_domain(keystr) ||
                !kiln_string_equal(shape_key, keystr))
                continue;
            *out = object->slots[slot];
            return KILN_C99_OK;
        }
    }
    ikey = kiln_c99_intern_key(keystr);
    if (ikey == NULL) return kiln_c99_val_read_element(host, target, key, depth, thrown, out);
    if (site != NULL) {
        const kiln_c99_pic_entry *entry = kiln_c99_site_entry_keyed(site, object->shape, ikey);
        if (entry != NULL) {
            if (entry->slot < object->slot_capacity) {
                *out = object->slots[entry->slot];
                return KILN_C99_OK;
            }
            if (kiln_c99_pic_inherited(entry, object, out)) return KILN_C99_OK;
        }
    }
    return kiln_c99_val_read_key_miss(host, target, key, ikey, depth, thrown, cell, out);
}

/* `target[key] = value` where `key` is a symbol: the stores a named write site caches -- an own
 * writable data slot, or the one transition an assignment of a new key takes -- and everything
 * else the element write's. */
static int kiln_c99_val_write_symbol(kiln_host *host, kiln_val target, kiln_val key,
                                     kiln_val value, int strict, int depth, kiln_value *thrown,
                                     kiln_c99_symbol_cell *cell) {
    kiln_c99_site *site = KILN_C99_LOAD_ACQUIRE(cell);
    if (KILN_LIKELY(site != NULL && KILN_VAL_IS_OBJECT(target))) {
        kiln_object *object = (kiln_object *)KILN_VAL_PTR(target);
        const kiln_string *ikey = kiln_c99_symbol_ikey((const kiln_symbol *)KILN_VAL_PTR(key));
        if (KILN_LIKELY(kiln_c99_gc_payload_kind(object) == KILN_GC_OBJECT &&
                        object->shape != NULL && ikey != NULL && !object->frozen &&
                        !object->module_namespace)) {
            const kiln_c99_pic_entry *entry =
                kiln_c99_site_entry_keyed(site, object->shape, ikey);
            if (KILN_LIKELY(entry != NULL)) {
                /* A published entry never changes; these are its words. */
                const uint32_t slot = entry->slot;
                const kiln_shape *next_shape = entry->transition_to;
                if (next_shape == NULL) {
                    if (KILN_LIKELY(slot < object->slot_capacity &&
                                    ((object->shape->double_slots_mask & (1u << slot)) == 0 ||
                                     KILN_VAL_IS_NUMBER(value)))) {
                        kiln_c99_prototype_touched(object);
                        kiln_c99_heap_store_val(object, &object->slots[slot], value);
                        return KILN_C99_OK;
                    }
                } else if (!object->not_extensible && slot < object->slot_capacity &&
                           ((next_shape->double_slots_mask & (1u << slot)) == 0 ||
                            KILN_VAL_IS_NUMBER(value)) &&
                           kiln_c99_transition_store_clear(site, entry, object, ikey, ikey)) {
                    kiln_c99_prototype_touched(object);
                    kiln_c99_heap_store_val(object, &object->slots[slot], value);
                    object->shape = (kiln_shape *)next_shape;
                    return KILN_C99_OK;
                }
            }
        }
    }
    return kiln_c99_val_write_symbol_miss(host, target, key, value, strict, depth, thrown, cell);
}

/* `target[key] = value` for a normal computed string key. The hit is deliberately narrower
 * than the existing named-write site: only a current own writable data slot on an ordinary
 * shaped object. Index strings, internal keys, dictionaries and exotic receivers remain on the
 * authoritative element-write path. The string is compared against the live shape-owned key
 * before any intern lookup, so freshly allocated equal-content strings take the same hit. */
int kiln_c99_val_write_string_key(kiln_host *host, kiln_val target, kiln_val key,
                                  kiln_val value, int strict, int depth, kiln_value *thrown,
                                  kiln_c99_symbol_cell *cell) {
    const kiln_string *keystr;
    kiln_object *object;
    const kiln_c99_site *site;
    uint32_t p;

    if (!KILN_VAL_IS_STRING(key))
        return kiln_c99_val_write_element(host, target, key, value, strict, depth, thrown);
    keystr = (const kiln_string *)KILN_VAL_PTR(key);
    if (keystr == NULL || kiln_c99_key_is_reserved(keystr) ||
        kiln_c99_key_index_key(keystr) >= 0.0)
        return kiln_c99_val_write_element(host, target, key, value, strict, depth, thrown);

    object = kiln_c99_keyed_receiver(target);
    if (object == NULL || object->shape == NULL || object->properties != NULL ||
        object->sealed != NULL || object->frozen || object->module_namespace)
        return kiln_c99_val_write_element(host, target, key, value, strict, depth, thrown);

    site = KILN_C99_LOAD_ACQUIRE(cell);
    if (site != NULL) {
        const uint32_t count = kiln_c99_site_count(site);
        for (p = 0; p < count; ++p) {
            const kiln_c99_pic_entry *entry = &site->entries[p];
            const kiln_shape *published_shape = kiln_c99_slot_key(&entry->shape);
            uint32_t slot;
            const kiln_string *shape_key;

            /* As on the computed-read PIC, prove the shape is live before touching the
             * realm-sensitive key word. A keyed write entry names an own slot, never a path. */
            if (published_shape != object->shape || !entry->keyed || entry->key == NULL ||
                entry->transition_to != NULL)
                continue;
            slot = entry->slot;
            if (slot >= object->shape->property_count || slot >= object->slot_capacity ||
                (object->shape->accessor_mask & (1u << slot)) != 0 ||
                (object->shape->store_mask & (1u << slot)) != 0 ||
                ((object->shape->double_slots_mask & (1u << slot)) != 0 &&
                 !KILN_VAL_IS_NUMBER(value)))
                continue;
            shape_key = object->shape->keys[slot];
            if (shape_key == NULL || entry->key != shape_key ||
                kiln_c99_key_domain(shape_key) != kiln_c99_key_domain(keystr) ||
                !kiln_string_equal(shape_key, keystr))
                continue;

            /* This is still an assignment to an object that may itself be a prototype. Keep
             * the invalidation before the store, as in the ordinary write path. */
            kiln_c99_prototype_touched(object);
            kiln_c99_heap_store_val(object, &object->slots[slot], value);
            return KILN_C99_OK;
        }
    }

    /* Preserve ToPropertyKey/[[Set]], strict errors, inherited setters and all exotic behavior
     * by letting the established path answer a miss. Learn only if that write left this receiver
     * with a current own writable data slot for this exact string. */
    {
        int status = kiln_c99_val_write_element(host, target, key, value, strict, depth, thrown);
        if (status != KILN_C99_OK) return status;
    }

    object = kiln_c99_keyed_receiver(target);
    if (object == NULL || object->shape == NULL || object->properties != NULL ||
        object->sealed != NULL || object->frozen || object->module_namespace)
        return KILN_C99_OK;
    for (p = 0; p < object->shape->property_count; ++p) {
        const kiln_string *shape_key = object->shape->keys[p];
        kiln_c99_pic_entry remembered;
        if (shape_key == NULL || kiln_c99_key_domain(shape_key) != kiln_c99_key_domain(keystr) ||
            !kiln_string_equal(shape_key, keystr))
            continue;
        if (p >= object->slot_capacity || (object->shape->accessor_mask & (1u << p)) != 0 ||
            (object->shape->store_mask & (1u << p)) != 0 ||
            ((object->shape->double_slots_mask & (1u << p)) != 0 &&
             !KILN_VAL_IS_NUMBER(value)))
            return KILN_C99_OK;
        memset(&remembered, 0, sizeof remembered);
        remembered.shape = object->shape;
        remembered.slot = p;
        remembered.keyed = 1;
        kiln_c99_key_site_remember(cell, &remembered, shape_key);
        return KILN_C99_OK;
    }
    return KILN_C99_OK;
}

static int kiln_c99_val_invoke_cached_method(kiln_host *host, uint64_t method_code, kiln_val receiver,
                                            const kiln_val *args, size_t argc, int depth,
                                            kiln_value *thrown, kiln_val *out) {
    if (depth >= KILN_C99_MAX_CALL_DEPTH || kiln_c99_stack_exhausted()) {
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
                if (!arr->has_index_descriptors && !arr->frozen && !arr->not_extensible) {
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
                        kiln_c99_heap_store_val(arr, &arr->elements[arr->count++], args[i]);
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
                if (!arr->has_index_descriptors && !arr->frozen && !arr->not_extensible) {
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
                if (!arr->has_index_descriptors && !arr->frozen && !arr->not_extensible) {
                    if (arr->count == 0) {
                        *out = KILN_VAL_UNDEFINED;
                        return KILN_C99_OK;
                    }
                    kiln_val first = arr->elements[0] == KILN_VAL_HOLE ? KILN_VAL_UNDEFINED : arr->elements[0];
                    arr->count -= 1;
                    if (arr->count > 0) {
                        kiln_c99_heap_move_vals(arr, arr->elements, arr->elements + 1, arr->count);
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
    } else if ((method_code == KILN_C99_METHOD_COLLECTION_GET ||
                method_code == KILN_C99_METHOD_COLLECTION_HAS) &&
               argc > 0 && KILN_VAL_IS_SHARED(receiver)) {
        /* A sealed Map or Set (ADR 0073): probe its arena index directly. */
        kiln_value sealed = kiln_val_to_boundary(receiver);
        int kind = kiln_c99_shared_collection_kind(sealed);
        if (kind == 1 || (kind == 0 && method_code == KILN_C99_METHOD_COLLECTION_HAS)) {
            kiln_value answer;
            int st = kiln_c99_shared_collection_lookup(
                (kiln_shared_facade *)(uintptr_t)sealed.bits, kiln_val_to_boundary(args[0]),
                method_code == KILN_C99_METHOD_COLLECTION_GET, &answer);
            if (KILN_UNLIKELY(st != KILN_C99_OK)) return st;
            *out = kiln_val_from_boundary(answer);
            return KILN_C99_OK;
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
                    kiln_c99_heap_copy_vals(b_arr, b_arr->elements, array->elements + start, slice_len);
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

    fallback:;

    kiln_value bself = kiln_val_to_boundary(receiver);
    kiln_value bargs[argc > 0 ? argc : 1];
    for (size_t bi = 0; bi < argc; ++bi) bargs[bi] = kiln_val_to_boundary(args[bi]);
    kiln_value bresult = kiln_c99_undefined();
    int status = kiln_c99_invoke_method(host, &bresult, method_code, bself, argc > 0 ? bargs : NULL, argc, depth, thrown);
    *out = kiln_val_from_boundary(bresult);
    return status;
}

/* Resolve and cache a method without invoking it. Prepared method calls use this at the
 * original property-read point, before evaluating arguments; the resulting callee remains the
 * authority at invocation time. */
static int kiln_c99_val_prepare_method_view(kiln_host *host, kiln_val receiver,
                                     const kiln_string *key, const kiln_string *canonical,
                                     int depth, kiln_value *thrown, kiln_c99_site *site,
                                     const char *inst_site, size_t inst_idx,
                                     const char *inst_op, kiln_val *out) {
    kiln_val method = 0;
    int from_cache = 0;
    if (KILN_LIKELY(kiln_c99_cached_method_matches(site, receiver, key))) {
        method = KILN_VAL_METHOD(site->cached_method);
        from_cache = 1;
    } else if (KILN_LIKELY(site != NULL && KILN_VAL_IS_OBJECT(receiver))) {
        void *ptr = KILN_VAL_PTR(receiver);
        if (KILN_LIKELY(ptr != NULL && kiln_c99_gc_payload_kind(ptr) == KILN_GC_OBJECT)) {
            const kiln_object *object = (const kiln_object *)ptr;
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
            kiln_c99_site_cache_method_key(site, (uint8_t)kiln_val_to_boundary(receiver).tag,
                                           KILN_VAL_METHOD_ID(method),
                                           KILN_VAL_IS_BUILTIN(receiver) ? KILN_VAL_BUILTIN_ID(receiver) : 0,
                                           key);
        } else if (site != NULL && KILN_VAL_IS_OBJECT(receiver) &&
                   /* A proxy, a generator and a collection are boxed with the object prefix and
                    * are not objects; a site learns a shape only from an object, so a hit -- a
                    * pointer compare against learned shapes -- cannot be one of theirs. The kind
                    * lookup is a heap search, affordable once per shape here and not per call. */
                   kiln_c99_gc_kind_of_ptr(KILN_VAL_PTR(receiver)) == KILN_GC_OBJECT) {
            const kiln_object *object = (const kiln_object *)KILN_VAL_PTR(receiver);
            if (object->shape != NULL) {
                /* An own property is remembered by its slot. A prototype's is remembered by the
                 * path to it (ADR 0071), one or two steps up the chain, when the member there
                 * holds exactly the callee just resolved -- a data property, not an accessor,
                 * and in a list-mode holder a record keyed by the canonical pointer the site
                 * resolved with. Anything else is not remembered, and the site resolves it
                 * again next time. */
                uint32_t own_slot = UINT32_MAX;
                int own = kiln_c99_shape_find_slot(object->shape, key, &own_slot);
                kiln_c99_pic_entry remembered;
                memset(&remembered, 0, sizeof remembered);
                remembered.shape = object->shape;
                /* An own accessor's slot holds its pair: remembered as nothing, so the site
                 * resolves it again. */
                remembered.slot = own && (object->shape->accessor_mask & (1u << own_slot)) == 0
                                      ? own_slot
                                      : UINT32_MAX;
                if (!own) {
                    const kiln_object *holder = NULL;
                    kiln_val found;
                    if (kiln_c99_pic_learn_path(&remembered, object, key, &holder) !=
                            KILN_C99_PATH_FOUND ||
                        !kiln_c99_pic_inherited(&remembered, object, &found) || found != method) {
                        memset(&remembered, 0, sizeof remembered);
                        remembered.shape = object->shape;
                        remembered.slot = UINT32_MAX;
                    }
                }
                kiln_c99_site_remember(site, &remembered);
            }
        }
    }

    *out = method;
    return KILN_C99_OK;
}

int kiln_c99_val_prepare_method_key(kiln_host *host, kiln_val receiver,
                                    const kiln_string *key, int depth, kiln_value *thrown,
                                    kiln_c99_site *site, const char *inst_site,
                                    size_t inst_idx, const char *inst_op, kiln_val *out) {
    key = kiln_c99_compiler_named_key(key);
    if (key == NULL) return KILN_C99_OUT_OF_MEMORY;
    return kiln_c99_val_prepare_method_view(host, receiver, key, key, depth, thrown, site,
                                            inst_site, inst_idx, inst_op, out);
}

/* Invoke a method using the callee resolved by the shared method-site lookup above. */
static int kiln_c99_val_call_method_view(kiln_host *host, kiln_val receiver, const kiln_val *args,
                                         size_t argc, const kiln_string *key,
                                         const kiln_string *canonical, int depth,
                                         kiln_value *thrown, kiln_c99_site *site,
                                         const char *inst_site, size_t inst_idx,
                                         const char *inst_op, kiln_val *out) {
    kiln_val method = KILN_VAL_UNDEFINED;
    int status = kiln_c99_val_prepare_method_view(host, receiver, key, canonical, depth, thrown,
                                                  site, inst_site, inst_idx, inst_op, &method);
    if (status != KILN_C99_OK) return status;

    if (KILN_VAL_IS_CLOSURE(method)) {
        kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(method);
        if (!plain->generator && !kiln_c99_inheriting_new_target &&
            depth < KILN_C99_MAX_CALL_DEPTH) {
            kiln_val vreceiver = plain->arrow ? plain->captured_this : receiver;
            status = kiln_c99_normalize_closure_receiver(&vreceiver, plain);
            if (status != KILN_C99_OK) goto done;
            *out = KILN_VAL_UNDEFINED;
            kiln_c99_js_frame_set_method_name(key);
            status = kiln_c99_invoke_closure(host, out, plain, vreceiver, args, argc, depth, thrown, NULL, 0);
            kiln_c99_js_frame_set_method_name(NULL);
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
    key = kiln_c99_compiler_named_key(key);
    if (key == NULL) return KILN_C99_OUT_OF_MEMORY;
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
        status = kiln_c99_normalize_closure_receiver(&recv, plain);
        if (status != KILN_C99_OK) return status;
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
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR,
                                    "cannot access a binding before its declaration");
    }
    return KILN_C99_OK;
}

/* One cell-store authority for generated stores and runtime boxing. Stack/unit-block roots
 * keep their separate root audit; heap cells contribute weak candidate owner witnesses. */
static KILN_ALWAYS_INLINE inline void kiln_c99_val_store_cell(kiln_val *cell, kiln_val value) {
    kiln_c99_heap_store_val(cell, cell, value);
}

static int kiln_c99_val_make_cell(kiln_val *slot) {
    kiln_val *box = (kiln_val *)kiln_c99_gc_alloc_hot(KILN_GC_CELL, sizeof *box, 0);
    if (box == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    kiln_c99_val_store_cell(box, *slot);
    *slot = KILN_VAL_TAG_PTR(KILN_TAG_CELL_PREFIX, box);
    return KILN_C99_OK;
}

int kiln_c99_val_make_cell_owned(kiln_val *slot, void *external_owner) {
    kiln_gc_cell *box = (kiln_gc_cell *)kiln_c99_gc_alloc_hot(KILN_GC_CELL, sizeof *box, 1);
    if (box == NULL) return KILN_C99_OUT_OF_MEMORY;
    box->value = *slot;
    kiln_c99_val_store_cell(&box->value, *slot);
    kiln_c99_heap_store_pointer(box, &box->external_owner, external_owner);
    *slot = KILN_VAL_TAG_PTR(KILN_TAG_CELL_PREFIX, &box->value);
    return KILN_C99_OK;
}

/* A shared-region unit's once-made binding already has a permanent realm-owned root in its
 * unit block. Its cell payload can live in an appended slot there: every closure reaches the
 * binding through the unit block, which also marks this value on collection. The block never
 * reallocates and dies only after the realm's frames are gone. Keep the ordinary cell tag and
 * dereference path; only the ownership of its eight-byte payload differs. */
static void kiln_c99_val_make_unit_cell(kiln_val *slot, kiln_val *storage) {
    kiln_c99_val_store_cell(storage, *slot);
    *slot = KILN_VAL_TAG_PTR(KILN_TAG_CELL_PREFIX, storage);
}

/* The frame registers (ADR 0041; `builtins.c`), which are marked rather than cleared so a stale
 * read after a collection still names live memory. The realm's own roots -- its symbols, the
 * built-in objects it made lazily, every unit's module block -- are its heap's (ADR 0070,
 * `kiln_c99_gc_mark_realm`) and die with it; nothing here holds one. At teardown the registers
 * are reset on teardown, and at a scheduled realm's lane handoff so neither lane retains a
 * stale value from its previous turn. */
static void kiln_c99_gc_unit_roots(void *opaque, void *context, int phase) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    (void)context;
    if (phase == KILN_GC_PHASE_RESET || phase == KILN_GC_PHASE_LANE_RESET) {
        kiln_c99_new_target = kiln_c99_undefined();
        kiln_c99_adopted_this = kiln_c99_undefined();
        kiln_c99_captured_arguments = kiln_c99_undefined();
        kiln_c99_frame_arguments_object = kiln_c99_undefined();
        kiln_c99_frame_callee = kiln_c99_undefined();
        return;
    }
    if (phase != KILN_GC_PHASE_MARK) return;
    kiln_c99_gc_mark_value(heap, &kiln_c99_new_target);
    kiln_c99_gc_mark_value(heap, &kiln_c99_adopted_this);
    kiln_c99_gc_mark_value(heap, &kiln_c99_captured_arguments);
    kiln_c99_gc_mark_value(heap, &kiln_c99_frame_arguments_object);
    kiln_c99_gc_mark_value(heap, &kiln_c99_frame_callee);
}

/* =========================================================================
 * Parallel built-ins, tier 4 (ADR 0069, parallel-builtins issue 09): the built-ins marked pure in
 * the member description (`kiln_builtin_surface::PURE_MEMBERS`), as a pool thread runs them.
 *
 * These are not new built-ins: each is the worker-safe path of a member both engines already
 * define, and it answers exactly what that member answers or declines with 0. A decline is not an
 * error -- the worker sets the call's failure flag and the site runs the sequential method from
 * the first element, where the member runs as it always does (and throws, where it throws). So
 * each declines wherever its sequential definition would run user code, throw, read anything but
 * its arguments and the frozen heap, or need an allocation an arena cannot give: an object
 * receiver (its own `trim` could be anyone's), a `RegExp` separator, a string or object argument
 * a conversion would parse or call `valueOf` on, a `RangeError` argument, a result array past
 * `KILN_C99_ARRAY_INBLOCK_MAX` elements.
 *
 * Strings are built flat in the worker's arena (`kiln_c99_par_string_alloc`); nothing here reads
 * a realm cache or interns. Two sequential helpers are shared outright rather than restated --
 * `kiln_c99_fixed_spelling` (`toFixed`) and `kiln_c99_parse_int_bytes`/`_float_bytes` -- so the
 * two paths cannot disagree about a digit.
 * ========================================================================= */

/* A string of `length` units in `arena`, Latin1 unless `utf16`, its units left for the caller. */
static void *kiln_c99_par_string_alloc(kiln_c99_par_arena *arena, size_t length, int utf16,
                                       kiln_val *out) {
    uint32_t flags = utf16 ? KILN_STRING_UTF16 : KILN_STRING_LATIN1;
    size_t storage;
    kiln_string *built;
    if (arena == NULL || length > UINT32_MAX ||
        !kiln_string_storage_size((uint32_t)length, flags, &storage)) {
        return NULL;
    }
    built = (kiln_string *)kiln_c99_par_alloc(arena, KILN_GC_STRING, storage + 1);
    if (built == NULL) return NULL;
    built->unit_len = (uint32_t)length;
    built->flags = flags;
    built->data = (const void *)(built + 1);
    if (utf16) {
        ((char *)(built + 1))[storage - sizeof(kiln_string)] = '\0';
    } else {
        ((char *)(built + 1))[length] = '\0';
    }
    *out = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, built);
    return (void *)(built + 1);
}

static kiln_val kiln_c99_par_empty(void) {
    return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
}

/* One code unit as a string: the static table below 256, the arena above. */
static int kiln_c99_par_unit_string(kiln_c99_par_arena *arena, uint16_t unit, kiln_val *out) {
    uint16_t *units;
    if (unit < 0x100u) {
        *out = kiln_c99_val_single_char((unsigned char)unit);
        return 1;
    }
    units = (uint16_t *)kiln_c99_par_string_alloc(arena, 1, 1, out);
    if (units == NULL) return 0;
    units[0] = unit;
    return 1;
}

/* `source`'s units `[start, start + length)`, as `kiln_c99_val_slice_string` answers them: the
 * empty string, `source` itself, a one-unit string, or a flat copy in the arena. */
static int kiln_c99_par_substring(kiln_c99_par_arena *arena, kiln_val value, size_t start,
                                  size_t length, kiln_val *out) {
    const kiln_string *source = (const kiln_string *)KILN_VAL_PTR(value);
    if (length == 0 || start >= source->unit_len) {
        *out = kiln_c99_par_empty();
        return 1;
    }
    if (length > source->unit_len - start) length = source->unit_len - start;
    if (start == 0 && length == source->unit_len) {
        *out = value;
        return 1;
    }
    if (length == 1) return kiln_c99_par_unit_string(arena, kiln_string_unit_at(source, start), out);
    if (kiln_string_is_utf16(source)) {
        uint16_t *units = (uint16_t *)kiln_c99_par_string_alloc(arena, length, 1, out);
        if (units == NULL) return 0;
        memcpy(units, (const uint16_t *)source->data + start, length * sizeof(uint16_t));
    } else {
        char *bytes = (char *)kiln_c99_par_string_alloc(arena, length, 0, out);
        if (bytes == NULL) return 0;
        memcpy(bytes, (const char *)kiln_string_latin1(source) + start, length);
    }
    return 1;
}

/* `ToIntegerOrInfinity` of a value that converts without running code. */
static int kiln_c99_par_integer(kiln_val value, double *out) {
    double d;
    if (!kiln_c99_par_number(value, &d)) return 0;
    *out = isnan(d) ? 0.0 : trunc(d);
    if (*out == 0.0) *out = 0.0;
    return 1;
}

/* A position from `ToIntegerOrInfinity`, clamped into `[0, length]`; a relative one (`slice`,
 * `at`) counts back from the end when negative. */
static size_t kiln_c99_par_clamp(double position, size_t length, int relative) {
    if (position < 0.0) {
        if (!relative) return 0;
        position += (double)length;
        return position < 0.0 ? 0 : (size_t)position;
    }
    return position > (double)length ? length : (size_t)position;
}

/* The argument at `index`, or undefined. */
static kiln_val kiln_c99_par_arg(size_t argc, const kiln_val *args, size_t index) {
    return index < argc ? args[index] : KILN_VAL_UNDEFINED;
}

/* `ToString` of an argument, for a primitive whose conversion runs no code: `text` is the string
 * (a number, boolean, undefined or null spelled into `buffer`/`view`). */
static int kiln_c99_par_arg_text(kiln_val value, char buffer[32], kiln_string *view,
                                 const kiln_string **text) {
    if (KILN_VAL_IS_OBJECT(value) || KILN_VAL_IS_CLOSURE(value) || KILN_VAL_IS_SYMBOL(value)) return 0;
    return kiln_c99_par_text(value, buffer, view, text);
}

/* Where `needle` occurs in `haystack` at or after `from`, or -1. */
static double kiln_c99_par_find(const kiln_string *haystack, const kiln_string *needle, size_t from) {
    size_t h = haystack->unit_len;
    size_t n = needle->unit_len;
    size_t at;
    if (n == 0) return (double)(from > h ? h : from);
    if (from >= h || n > h - from) return -1.0;
    if (!kiln_string_is_utf16(haystack) && !kiln_string_is_utf16(needle)) {
        const char *hb = (const char *)kiln_string_latin1(haystack);
        const char *found = kiln_c99_memmem(hb + from, h - from, (const char *)kiln_string_latin1(needle), n);
        return found != NULL ? (double)(found - hb) : -1.0;
    }
    for (at = from; at + n <= h; ++at) {
        size_t k = 0;
        while (k < n && kiln_string_unit_at(haystack, at + k) == kiln_string_unit_at(needle, k)) ++k;
        if (k == n) return (double)at;
    }
    return -1.0;
}

static int kiln_c99_par_units_equal(const kiln_string *a, size_t at, const kiln_string *b) {
    size_t k;
    for (k = 0; k < b->unit_len; ++k) {
        if (kiln_string_unit_at(a, at + k) != kiln_string_unit_at(b, k)) return 0;
    }
    return 1;
}

/* An ordinary array with no named properties: what `Array.prototype`'s pure members read. */
static int kiln_c99_par_plain_array(kiln_val value, const kiln_array **out) {
    const kiln_array *array;
    if ((value & KILN_TAG_MASK) != KILN_TAG_BUFFER_PREFIX) return 0;
    if (kiln_c99_gc_payload_kind(KILN_VAL_PTR(value)) != KILN_GC_ARRAY) return 0;
    array = (const kiln_array *)KILN_VAL_PTR(value);
    if (array->properties.tag != KILN_VALUE_UNDEFINED) return 0;
    *out = array;
    return 1;
}

/* A new array of `count` holes in the arena, for the caller to fill and then seal. */
static int kiln_c99_par_array_sized(kiln_c99_par_arena *arena, size_t count, kiln_val *out,
                                    kiln_array **array) {
    if (!kiln_c99_par_array(arena, NULL, count, out)) return 0;
    *array = (kiln_array *)KILN_VAL_PTR(*out);
    return 1;
}

/* The array is numeric when every element it ended with is a number. */
static void kiln_c99_par_array_seal(kiln_array *array) {
    size_t index;
    int numeric = 1;
    for (index = 0; index < array->count && numeric; ++index) {
        numeric = KILN_VAL_IS_NUMBER(array->elements[index]);
    }
    array->is_numeric = numeric;
}

static int kiln_c99_par_is_bigint(kiln_val value) {
    return KILN_VAL_IS_BIGINT(value);
}

/* --- callable globals ------------------------------------------------------------------ */

/* `Number(x)`: `ToNumeric` of a primitive; an ASCII string parses as the sequential conversion
 * parses it (`kiln_c99_as_number`); other strings, BigInts, symbols and objects decline. */
static int kiln_c99_par_f_Number(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                 size_t argc, const kiln_val *args, kiln_val *out) {
    kiln_val value;
    double d;
    (void)env;
    (void)arena;
    if (argc == 0) {
        *out = kiln_c99_val_from_double(0.0);
        return 1;
    }
    value = args[0];
    if (KILN_VAL_IS_STRING(value)) {
        const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(value);
        const unsigned char *bytes;
        size_t start = 0;
        size_t end;
        size_t k;
        if (kiln_string_is_utf16(s)) {
            /* The same over UTF-16 units: white space trimmed, anything else non-ASCII is NaN,
             * and the ASCII rest parsed from a copy (a literal longer than the copy declines). */
            char ascii[256];
            end = s->unit_len;
            while (start < end && kiln_c99_is_trimmed(kiln_string_unit_at(s, start))) start += 1;
            while (end > start && kiln_c99_is_trimmed(kiln_string_unit_at(s, end - 1))) end -= 1;
            if (end - start > sizeof ascii) return 0;
            for (k = start; k < end; ++k) {
                uint16_t unit = kiln_string_unit_at(s, k);
                if (unit >= 0x80) {
                    *out = KILN_QNAN_BASE;
                    return 1;
                }
                ascii[k - start] = (char)unit;
            }
            *out = kiln_c99_val_from_double(kiln_string_to_number(ascii, end - start));
            return 1;
        }
        bytes = (const unsigned char *)kiln_string_latin1(s);
        end = s->unit_len;
        /* Latin1's one non-ASCII white space (U+00A0) may pad a numeric literal; any other
         * non-ASCII unit makes it no literal at all. The ASCII rest is what the provider parses,
         * exactly as it parses the sequential conversion's UTF-8 (it trims the same white space). */
        while (start < end && kiln_c99_is_trimmed(bytes[start])) start += 1;
        while (end > start && kiln_c99_is_trimmed(bytes[end - 1])) end -= 1;
        for (k = start; k < end; ++k) {
            if (bytes[k] >= 0x80) {
                *out = KILN_QNAN_BASE;
                return 1;
            }
        }
        *out = kiln_c99_val_from_double(kiln_string_to_number((const char *)bytes + start, end - start));
        return 1;
    }
    if (!kiln_c99_par_number(value, &d)) return 0;
    *out = kiln_c99_val_from_double(d);
    return 1;
}

static int kiln_c99_par_f_Boolean(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                  size_t argc, const kiln_val *args, kiln_val *out) {
    (void)env;
    (void)arena;
    *out = argc > 0 && kiln_c99_val_truthy(args[0]) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
    return 1;
}

static int kiln_c99_par_f_String(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                 size_t argc, const kiln_val *args, kiln_val *out) {
    (void)env;
    if (argc == 0) {
        *out = kiln_c99_par_empty();
        return 1;
    }
    if (KILN_VAL_IS_SYMBOL(args[0])) return 0;
    return kiln_c99_par_to_string(arena, args[0], out);
}

/* The Latin1 units `parseInt`/`parseFloat` read: `ToString` of a primitive; a UTF-16 string
 * declines (the sequential reader borrows only Latin1 bytes). */
static int kiln_c99_par_parse_subject(kiln_val value, char buffer[32], kiln_string *view,
                                      const char **bytes, size_t *length) {
    const kiln_string *text;
    if (!kiln_c99_par_arg_text(value, buffer, view, &text)) return 0;
    if (kiln_string_is_utf16(text)) return 0;
    *bytes = (const char *)kiln_string_latin1(text);
    *length = text->unit_len;
    return 1;
}

static int kiln_c99_par_f_parseInt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                   size_t argc, const kiln_val *args, kiln_val *out) {
    char buffer[32];
    kiln_string view;
    const char *bytes;
    size_t length;
    double radix = 0.0;
    (void)env;
    (void)arena;
    if (!kiln_c99_par_parse_subject(kiln_c99_par_arg(argc, args, 0), buffer, &view, &bytes, &length)) {
        return 0;
    }
    if (argc > 1 && !kiln_c99_par_number(args[1], &radix)) return 0;
    *out = kiln_c99_val_from_double(kiln_c99_parse_int_bytes(bytes, length, radix));
    return 1;
}

static int kiln_c99_par_f_parseFloat(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                     size_t argc, const kiln_val *args, kiln_val *out) {
    char buffer[32];
    kiln_string view;
    const char *bytes;
    size_t length;
    (void)env;
    (void)arena;
    if (!kiln_c99_par_parse_subject(kiln_c99_par_arg(argc, args, 0), buffer, &view, &bytes, &length)) {
        return 0;
    }
    *out = kiln_c99_val_from_double(kiln_c99_parse_float_bytes(bytes, length));
    return 1;
}

/* --- Number's statics: none converts its argument ---------------------------------------- */

static int kiln_c99_par_number_arg(size_t argc, const kiln_val *args, double *d) {
    if (argc == 0 || !KILN_VAL_IS_NUMBER(args[0])) return 0;
    memcpy(d, &args[0], sizeof *d);
    return 1;
}

static int kiln_c99_par_f_Number_isInteger(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                           size_t argc, const kiln_val *args, kiln_val *out) {
    double d;
    (void)env;
    (void)arena;
    *out = kiln_c99_par_number_arg(argc, args, &d) && isfinite(d) && trunc(d) == d ? KILN_VAL_TRUE
                                                                                 : KILN_VAL_FALSE;
    return 1;
}

static int kiln_c99_par_f_Number_isSafeInteger(const kiln_c99_par_env *env,
                                               kiln_c99_par_arena *arena, size_t argc,
                                               const kiln_val *args, kiln_val *out) {
    double d;
    (void)env;
    (void)arena;
    *out = kiln_c99_par_number_arg(argc, args, &d) && isfinite(d) && trunc(d) == d &&
                   fabs(d) <= 9007199254740991.0
               ? KILN_VAL_TRUE
               : KILN_VAL_FALSE;
    return 1;
}

static int kiln_c99_par_f_Number_isNaN(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                       size_t argc, const kiln_val *args, kiln_val *out) {
    double d;
    (void)env;
    (void)arena;
    *out = kiln_c99_par_number_arg(argc, args, &d) && isnan(d) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
    return 1;
}

static int kiln_c99_par_f_Number_isFinite(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                          size_t argc, const kiln_val *args, kiln_val *out) {
    double d;
    (void)env;
    (void)arena;
    *out = kiln_c99_par_number_arg(argc, args, &d) && isfinite(d) ? KILN_VAL_TRUE : KILN_VAL_FALSE;
    return 1;
}

static int kiln_c99_par_f_Number_parseInt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                          size_t argc, const kiln_val *args, kiln_val *out) {
    return kiln_c99_par_f_parseInt(env, arena, argc, args, out);
}

static int kiln_c99_par_f_Number_parseFloat(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                            size_t argc, const kiln_val *args, kiln_val *out) {
    return kiln_c99_par_f_parseFloat(env, arena, argc, args, out);
}

/* --- JSON.stringify of plain data ------------------------------------------------------- */

#define KILN_C99_PAR_JSON_DEPTH 64

/* Whether `object` or any object on its prototype chain could answer `toJSON`: its shaped keys
 * are compared with the one interned spelling (`tojson`; NULL when the realm never interned it, so
 * no shape holds it); the chain ends at the built-in `Object.prototype` (undefined) or null. A
 * dictionary-mode level, or a prototype that is not an ordinary object, is a maybe. */
static int kiln_c99_par_json_maybe_tojson(const kiln_object *object, const kiln_string *tojson) {
    int hops = 0;
    for (;;) {
        size_t index;
        if (object->properties != NULL || object->shape == NULL) return 1;
        if (tojson != NULL) {
            for (index = 0; index < object->shape->property_count; ++index) {
                if (object->shape->keys[index] == tojson) return 1;
            }
        }
        if (object->prototype.tag == KILN_VALUE_UNDEFINED || object->prototype.tag == KILN_VALUE_NULL) {
            return 0;
        }
        if (object->prototype.tag != KILN_VALUE_OBJECT || ++hops > KILN_C99_MAX_PROTOTYPE_DEPTH) return 1;
        object = kiln_c99_object_of(object->prototype);
        if (object == NULL || kiln_c99_gc_payload_kind(object) != KILN_GC_OBJECT) return 1;
    }
}

static int kiln_c99_par_json_value(kiln_text *text, kiln_val value, int *wrote,
                                   const void **stack, size_t depth, const kiln_string *tojson) {
    int status = KILN_C99_OK;
    size_t index;
    *wrote = 1;
    if (kiln_c99_json_scalar(text, value, &status)) return status == KILN_C99_OK;
    if (value == KILN_VAL_UNDEFINED || value == KILN_VAL_HOLE || KILN_VAL_IS_CLOSURE(value) ||
        KILN_VAL_IS_SYMBOL(value)) {
        *wrote = 0;
        return 1;
    }
    if (depth >= KILN_C99_PAR_JSON_DEPTH) return 0;
    if ((value & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {
        const kiln_array *array;
        if (!kiln_c99_par_plain_array(value, &array)) return 0;
        for (index = 0; index < depth; ++index) {
            if (stack[index] == (const void *)array) return 0;
        }
        stack[depth] = array;
        if (kiln_c99_text_append(text, "[", 1) != KILN_C99_OK) return 0;
        for (index = 0; index < array->count; ++index) {
            int member;
            if (index != 0 && kiln_c99_text_append(text, ",", 1) != KILN_C99_OK) return 0;
            if (!kiln_c99_par_json_value(text, array->elements[index], &member, stack, depth + 1, tojson)) {
                return 0;
            }
            if (!member && kiln_c99_text_append(text, "null", 4) != KILN_C99_OK) return 0;
        }
        return kiln_c99_text_append(text, "]", 1) == KILN_C99_OK;
    }
    if (KILN_VAL_IS_OBJECT(value)) {
        const kiln_object *object = (const kiln_object *)KILN_VAL_PTR(value);
        size_t written = 0;
        if (kiln_c99_gc_payload_kind(object) != KILN_GC_OBJECT || object->arguments_brand ||
            object->error_brand || object->module_namespace || object->shape == NULL ||
            object->properties != NULL || object->shape->accessor_mask != 0 ||
            kiln_c99_par_json_maybe_tojson(object, tojson)) {
            return 0;
        }
        for (index = 0; index < depth; ++index) {
            if (stack[index] == (const void *)object) return 0;
        }
        stack[depth] = object;
        if (kiln_c99_text_append(text, "{", 1) != KILN_C99_OK) return 0;
        if (object->elements != NULL) {
            for (index = 0; index < object->element_count; ++index) {
                char digits[32];
                size_t length;
                size_t mark;
                int member;
                if (object->elements[index] == KILN_VAL_HOLE) continue;
                length = kiln_c99_double_to_js((double)index, digits);
                mark = text->len;
                if ((written != 0 && kiln_c99_text_append(text, ",", 1) != KILN_C99_OK) ||
                    kiln_c99_json_quote(text, digits, length) != KILN_C99_OK ||
                    kiln_c99_text_append(text, ":", 1) != KILN_C99_OK) {
                    return 0;
                }
                if (!kiln_c99_par_json_value(text, object->elements[index], &member, stack, depth + 1, tojson)) {
                    return 0;
                }
                if (!member) {
                    text->len = mark;
                    continue;
                }
                written += 1;
            }
        }
        for (index = 0; index < object->shape->property_count; ++index) {
            const kiln_string *key = object->shape->keys[index];
            kiln_val held = index < object->slot_capacity ? object->slots[index] : KILN_VAL_UNDEFINED;
            size_t mark;
            int member;
            if (kiln_c99_key_is_reserved(key) || (object->shape->attrs[index] & KILN_C99_ATTR_ENUM_FLIP) != 0) {
                continue;
            }
            mark = text->len;
            if ((written != 0 && kiln_c99_text_append(text, ",", 1) != KILN_C99_OK) ||
                kiln_c99_json_quote_string(text, key) != KILN_C99_OK ||
                kiln_c99_text_append(text, ":", 1) != KILN_C99_OK) {
                return 0;
            }
            if (!kiln_c99_par_json_value(text, held, &member, stack, depth + 1, tojson)) return 0;
            if (!member) {
                text->len = mark;
                continue;
            }
            written += 1;
        }
        return kiln_c99_text_append(text, "}", 1) == KILN_C99_OK;
    }
    /* A BigInt throws; a Map, a Date, a view, a proxy, a boxed primitive each has its own rule. */
    return 0;
}

/* The UTF-8 `bytes` the serialiser wrote, as a string in the arena: Latin1 when every code point
 * fits, UTF-16 otherwise (the serialiser escapes lone surrogates, so the bytes are well formed). */
static int kiln_c99_par_utf8_string(kiln_c99_par_arena *arena, const unsigned char *bytes,
                                    size_t length, kiln_val *out) {
    size_t index = 0;
    size_t units = 0;
    int wide = 0;
    int ascii = 1;
    for (index = 0; index < length; ++index) {
        if (bytes[index] >= 0x80) {
            ascii = 0;
            break;
        }
    }
    if (ascii) {
        char *built = (char *)kiln_c99_par_string_alloc(arena, length, 0, out);
        if (built == NULL) return 0;
        if (length != 0) memcpy(built, bytes, length);
        return 1;
    }
    for (index = 0; index < length;) {
        uint32_t point;
        unsigned char lead = bytes[index];
        size_t width = lead < 0x80 ? 1 : lead < 0xE0 ? 2 : lead < 0xF0 ? 3 : 4;
        if (index + width > length) return 0;
        if (width == 1) point = lead;
        else if (width == 2) point = ((uint32_t)(lead & 0x1F) << 6) | (bytes[index + 1] & 0x3F);
        else if (width == 3) {
            point = ((uint32_t)(lead & 0x0F) << 12) | ((uint32_t)(bytes[index + 1] & 0x3F) << 6) |
                    (bytes[index + 2] & 0x3F);
        } else {
            point = ((uint32_t)(lead & 0x07) << 18) | ((uint32_t)(bytes[index + 1] & 0x3F) << 12) |
                    ((uint32_t)(bytes[index + 2] & 0x3F) << 6) | (bytes[index + 3] & 0x3F);
        }
        if (point > 0xFF) wide = 1;
        units += point >= 0x10000 ? 2 : 1;
        index += width;
    }
    if (!wide) {
        unsigned char *built = (unsigned char *)kiln_c99_par_string_alloc(arena, units, 0, out);
        size_t at = 0;
        if (built == NULL) return 0;
        for (index = 0; index < length;) {
            unsigned char lead = bytes[index];
            if (lead < 0x80) {
                built[at++] = lead;
                index += 1;
            } else {
                built[at++] = (unsigned char)(((lead & 0x1F) << 6) | (bytes[index + 1] & 0x3F));
                index += 2;
            }
        }
        return 1;
    }
    {
        uint16_t *built = (uint16_t *)kiln_c99_par_string_alloc(arena, units, 1, out);
        size_t at = 0;
        if (built == NULL) return 0;
        for (index = 0; index < length;) {
            uint32_t point;
            unsigned char lead = bytes[index];
            size_t width = lead < 0x80 ? 1 : lead < 0xE0 ? 2 : lead < 0xF0 ? 3 : 4;
            if (width == 1) point = lead;
            else if (width == 2) point = ((uint32_t)(lead & 0x1F) << 6) | (bytes[index + 1] & 0x3F);
            else if (width == 3) {
                point = ((uint32_t)(lead & 0x0F) << 12) | ((uint32_t)(bytes[index + 1] & 0x3F) << 6) |
                        (bytes[index + 2] & 0x3F);
            } else {
                point = ((uint32_t)(lead & 0x07) << 18) | ((uint32_t)(bytes[index + 1] & 0x3F) << 12) |
                        ((uint32_t)(bytes[index + 2] & 0x3F) << 6) | (bytes[index + 3] & 0x3F);
            }
            if (point >= 0x10000) {
                point -= 0x10000;
                built[at++] = (uint16_t)(0xD800 + (point >> 10));
                built[at++] = (uint16_t)(0xDC00 + (point & 0x3FF));
            } else {
                built[at++] = (uint16_t)point;
            }
            index += width;
        }
        return 1;
    }
}

/* `JSON.stringify(value)`, one argument: the compact form of `kiln_c99_json_value`'s walk over
 * plain data -- numbers, strings, booleans, null, arrays with no named properties, and ordinary
 * shaped objects no level of whose chain could answer `toJSON`; undefined, functions and symbols
 * omitted as the walk omits them. A cycle (a `TypeError`), a BigInt (a `TypeError`) and every
 * other object decline. */
static int kiln_c99_par_f_JSON_stringify(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                         size_t argc, const kiln_val *args, kiln_val *out) {
    static const kiln_string tojson_view = {6, KILN_STRING_LATIN1, "toJSON"};
    const void *stack[KILN_C99_PAR_JSON_DEPTH];
    const kiln_string *tojson;
    kiln_text text = {NULL, 0, 0};
    int wrote = 0;
    int ok;
    if (argc != 1 || env == NULL) return 0;
    tojson = kiln_c99_interned_key_in(env->heap, &tojson_view);
    ok = kiln_c99_par_json_value(&text, args[0], &wrote, stack, 0, tojson);
    if (ok && !wrote) {
        *out = KILN_VAL_UNDEFINED;
    } else if (ok) {
        ok = kiln_c99_par_utf8_string(arena, (const unsigned char *)text.bytes, text.len, out);
    }
    free(text.bytes);
    return ok;
}

/* --- String.prototype, Number.prototype and Array.prototype, by name -------------------------
 * `kiln_c99_par_m_<key>`: the member `key` of whichever of the three prototypes marks it pure,
 * chosen by the receiver's kind; a receiver of another kind declines. */

#define KILN_C99_PAR_STRING_SELF(self)                                                            \
    const kiln_string *s;                                                                         \
    if (!KILN_VAL_IS_STRING(self)) return 0;                                                      \
    s = (const kiln_string *)KILN_VAL_PTR(self)

static int kiln_c99_par_trim_mode(kiln_c99_par_arena *arena, kiln_val self, int mode, kiln_val *out) {
    size_t start = 0;
    size_t end;
    KILN_C99_PAR_STRING_SELF(self);
    end = s->unit_len;
    if (mode != 2) {
        while (start < end && kiln_c99_is_trimmed(kiln_string_unit_at(s, start))) start += 1;
    }
    if (mode != 1) {
        while (end > start && kiln_c99_is_trimmed(kiln_string_unit_at(s, end - 1))) end -= 1;
    }
    return kiln_c99_par_substring(arena, self, start, end - start, out);
}

static int kiln_c99_par_m_trim(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
                               size_t argc, const kiln_val *args, kiln_val *out) {
    (void)env; (void)argc; (void)args;
    return kiln_c99_par_trim_mode(arena, self, 0, out);
}

static int kiln_c99_par_m_trimStart(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                    kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    (void)env; (void)argc; (void)args;
    return kiln_c99_par_trim_mode(arena, self, 1, out);
}

static int kiln_c99_par_m_trimEnd(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                  kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    (void)env; (void)argc; (void)args;
    return kiln_c99_par_trim_mode(arena, self, 2, out);
}

/* ASCII only: every other letter has a mapping that is not one unit to one unit (`ß`, `ÿ`). */
static int kiln_c99_par_case(kiln_c99_par_arena *arena, kiln_val self, int upper, kiln_val *out) {
    const unsigned char *bytes;
    char *built;
    size_t k;
    int changes = 0;
    KILN_C99_PAR_STRING_SELF(self);
    if (kiln_string_is_utf16(s)) return 0;
    bytes = (const unsigned char *)kiln_string_latin1(s);
    for (k = 0; k < s->unit_len; ++k) {
        if (bytes[k] >= 0x80) return 0;
        changes |= upper ? (bytes[k] >= 'a' && bytes[k] <= 'z') : (bytes[k] >= 'A' && bytes[k] <= 'Z');
    }
    if (!changes) {
        *out = self;
        return 1;
    }
    built = (char *)kiln_c99_par_string_alloc(arena, s->unit_len, 0, out);
    if (built == NULL) return 0;
    for (k = 0; k < s->unit_len; ++k) {
        unsigned char c = bytes[k];
        if (upper && c >= 'a' && c <= 'z') c = (unsigned char)(c - 32);
        if (!upper && c >= 'A' && c <= 'Z') c = (unsigned char)(c + 32);
        built[k] = (char)c;
    }
    return 1;
}

static int kiln_c99_par_m_toUpperCase(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                      kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    (void)env; (void)argc; (void)args;
    return kiln_c99_par_case(arena, self, 1, out);
}

static int kiln_c99_par_m_toLowerCase(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                      kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    (void)env; (void)argc; (void)args;
    return kiln_c99_par_case(arena, self, 0, out);
}

static int kiln_c99_par_m_at(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
                             size_t argc, const kiln_val *args, kiln_val *out) {
    double position;
    KILN_C99_PAR_STRING_SELF(self);
    (void)env;
    if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 0), &position)) return 0;
    if (position < 0.0) position += (double)s->unit_len;
    if (position < 0.0 || position >= (double)s->unit_len) {
        *out = KILN_VAL_UNDEFINED;
        return 1;
    }
    return kiln_c99_par_unit_string(arena, kiln_string_unit_at(s, (size_t)position), out);
}

static int kiln_c99_par_m_charAt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                 kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    double position;
    KILN_C99_PAR_STRING_SELF(self);
    (void)env;
    if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 0), &position)) return 0;
    if (position < 0.0 || position >= (double)s->unit_len) {
        *out = kiln_c99_par_empty();
        return 1;
    }
    return kiln_c99_par_unit_string(arena, kiln_string_unit_at(s, (size_t)position), out);
}

static int kiln_c99_par_m_codePointAt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                      kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    double position;
    size_t at;
    uint16_t lead;
    KILN_C99_PAR_STRING_SELF(self);
    (void)env;
    (void)arena;
    if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 0), &position)) return 0;
    if (position < 0.0 || position >= (double)s->unit_len) {
        *out = KILN_VAL_UNDEFINED;
        return 1;
    }
    at = (size_t)position;
    lead = kiln_string_unit_at(s, at);
    if (lead >= 0xD800 && lead <= 0xDBFF && at + 1 < s->unit_len) {
        uint16_t trail = kiln_string_unit_at(s, at + 1);
        if (trail >= 0xDC00 && trail <= 0xDFFF) {
            *out = kiln_c99_val_from_double(
                (double)(0x10000u + (((uint32_t)lead - 0xD800u) << 10) + ((uint32_t)trail - 0xDC00u)));
            return 1;
        }
    }
    *out = kiln_c99_val_from_double((double)lead);
    return 1;
}

static int kiln_c99_par_m_substring(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                    kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    double a = 0.0;
    double b;
    size_t start;
    size_t end;
    KILN_C99_PAR_STRING_SELF(self);
    (void)env;
    b = (double)s->unit_len;
    if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 0), &a)) return 0;
    if (argc > 1 && args[1] != KILN_VAL_UNDEFINED && !kiln_c99_par_integer(args[1], &b)) return 0;
    start = kiln_c99_par_clamp(a, s->unit_len, 0);
    end = kiln_c99_par_clamp(b, s->unit_len, 0);
    if (start > end) {
        size_t swap = start;
        start = end;
        end = swap;
    }
    return kiln_c99_par_substring(arena, self, start, end - start, out);
}

static int kiln_c99_par_m_slice(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    double a = 0.0;
    double b;
    size_t length;
    size_t start;
    size_t end;
    const kiln_array *array = NULL;
    (void)env;
    if (KILN_VAL_IS_STRING(self)) {
        length = ((const kiln_string *)KILN_VAL_PTR(self))->unit_len;
    } else if (kiln_c99_par_plain_array(self, &array)) {
        length = array->count;
    } else {
        return 0;
    }
    b = (double)length;
    if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 0), &a)) return 0;
    if (argc > 1 && args[1] != KILN_VAL_UNDEFINED && !kiln_c99_par_integer(args[1], &b)) return 0;
    start = kiln_c99_par_clamp(a, length, 1);
    end = kiln_c99_par_clamp(b, length, 1);
    if (end < start) end = start;
    if (array == NULL) return kiln_c99_par_substring(arena, self, start, end - start, out);
    /* An array's slice keeps its holes: `slice` skips an absent index and moves on. */
    return kiln_c99_par_array(arena, array->elements + start, end - start, out);
}

static int kiln_c99_par_m_indexOf(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                  kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    double from = 0.0;
    const kiln_array *array;
    (void)env;
    (void)arena;
    if (KILN_VAL_IS_STRING(self)) {
        const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(self);
        char buffer[32];
        kiln_string view;
        const kiln_string *needle;
        if (!kiln_c99_par_arg_text(kiln_c99_par_arg(argc, args, 0), buffer, &view, &needle)) return 0;
        if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 1), &from)) return 0;
        *out = kiln_c99_val_from_double(kiln_c99_par_find(s, needle, kiln_c99_par_clamp(from, s->unit_len, 0)));
        return 1;
    }
    if (kiln_c99_par_plain_array(self, &array)) {
        kiln_val target = kiln_c99_par_arg(argc, args, 0);
        size_t k;
        if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 1), &from)) return 0;
        for (k = kiln_c99_par_clamp(from, array->count, 1); k < array->count; ++k) {
            int same;
            if (array->elements[k] == KILN_VAL_HOLE) continue;
            same = kiln_c99_par_strict_equal(array->elements[k], target);
            if (same < 0) return 0;
            if (same) {
                *out = kiln_c99_val_from_double((double)k);
                return 1;
            }
        }
        *out = kiln_c99_val_from_double(-1.0);
        return 1;
    }
    return 0;
}

static int kiln_c99_par_m_includes(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                   kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    double from = 0.0;
    const kiln_array *array;
    (void)env;
    (void)arena;
    if (KILN_VAL_IS_STRING(self)) {
        const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(self);
        char buffer[32];
        kiln_string view;
        const kiln_string *needle;
        if (!kiln_c99_par_arg_text(kiln_c99_par_arg(argc, args, 0), buffer, &view, &needle)) return 0;
        if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 1), &from)) return 0;
        *out = kiln_c99_par_find(s, needle, kiln_c99_par_clamp(from, s->unit_len, 0)) >= 0.0
                   ? KILN_VAL_TRUE
                   : KILN_VAL_FALSE;
        return 1;
    }
    if (kiln_c99_par_plain_array(self, &array)) {
        kiln_val target = kiln_c99_par_arg(argc, args, 0);
        size_t k;
        double t;
        int target_nan = KILN_VAL_IS_NUMBER(target) && (memcpy(&t, &target, sizeof t), isnan(t));
        if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 1), &from)) return 0;
        for (k = kiln_c99_par_clamp(from, array->count, 1); k < array->count; ++k) {
            /* `includes` reads a hole as undefined, and SameValueZero finds NaN. */
            kiln_val element = array->elements[k] == KILN_VAL_HOLE ? KILN_VAL_UNDEFINED : array->elements[k];
            int same;
            double e;
            if (target_nan && KILN_VAL_IS_NUMBER(element) && (memcpy(&e, &element, sizeof e), isnan(e))) {
                *out = KILN_VAL_TRUE;
                return 1;
            }
            same = kiln_c99_par_strict_equal(element, target);
            if (same < 0) return 0;
            if (same) {
                *out = KILN_VAL_TRUE;
                return 1;
            }
        }
        *out = KILN_VAL_FALSE;
        return 1;
    }
    return 0;
}

static int kiln_c99_par_m_lastIndexOf(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                      kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    char buffer[32];
    kiln_string view;
    const kiln_string *needle;
    double position = INFINITY;
    size_t limit;
    size_t h;
    size_t n;
    size_t at;
    KILN_C99_PAR_STRING_SELF(self);
    (void)env;
    (void)arena;
    if (!kiln_c99_par_arg_text(kiln_c99_par_arg(argc, args, 0), buffer, &view, &needle)) return 0;
    if (argc > 1) {
        double d;
        if (!kiln_c99_par_number(args[1], &d)) return 0;
        position = isnan(d) ? INFINITY : trunc(d);
    }
    h = s->unit_len;
    n = needle->unit_len;
    limit = kiln_c99_par_clamp(position, h, 0);
    if (n > h) {
        *out = kiln_c99_val_from_double(-1.0);
        return 1;
    }
    if (limit > h - n) limit = h - n;
    for (at = limit + 1; at > 0; --at) {
        if (kiln_c99_par_units_equal(s, at - 1, needle)) {
            *out = kiln_c99_val_from_double((double)(at - 1));
            return 1;
        }
    }
    *out = kiln_c99_val_from_double(-1.0);
    return 1;
}

static int kiln_c99_par_m_startsWith(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                     kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    char buffer[32];
    kiln_string view;
    const kiln_string *needle;
    double position = 0.0;
    size_t start;
    KILN_C99_PAR_STRING_SELF(self);
    (void)env;
    (void)arena;
    if (!kiln_c99_par_arg_text(kiln_c99_par_arg(argc, args, 0), buffer, &view, &needle)) return 0;
    if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 1), &position)) return 0;
    start = kiln_c99_par_clamp(position, s->unit_len, 0);
    *out = needle->unit_len <= s->unit_len - start && kiln_c99_par_units_equal(s, start, needle)
               ? KILN_VAL_TRUE
               : KILN_VAL_FALSE;
    return 1;
}

static int kiln_c99_par_m_endsWith(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                   kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    char buffer[32];
    kiln_string view;
    const kiln_string *needle;
    double position;
    size_t end;
    KILN_C99_PAR_STRING_SELF(self);
    (void)env;
    (void)arena;
    position = (double)s->unit_len;
    if (!kiln_c99_par_arg_text(kiln_c99_par_arg(argc, args, 0), buffer, &view, &needle)) return 0;
    if (argc > 1 && args[1] != KILN_VAL_UNDEFINED && !kiln_c99_par_integer(args[1], &position)) return 0;
    end = kiln_c99_par_clamp(position, s->unit_len, 0);
    *out = needle->unit_len <= end && kiln_c99_par_units_equal(s, end - needle->unit_len, needle)
               ? KILN_VAL_TRUE
               : KILN_VAL_FALSE;
    return 1;
}

/* The most units `padStart`, `padEnd` and `repeat` build on a pool thread; a longer answer runs
 * sequentially (where a string past the engine's limit throws). */
#define KILN_C99_PAR_BUILT_UNITS ((size_t)1 << 24)

static int kiln_c99_par_pad(kiln_c99_par_arena *arena, kiln_val self, size_t argc,
                            const kiln_val *args, int at_start, kiln_val *out) {
    char buffer[32];
    kiln_string view;
    kiln_string space = {1, KILN_STRING_LATIN1, " "};
    const kiln_string *fill = &space;
    double wanted;
    size_t target;
    size_t pad;
    size_t k;
    int utf16;
    KILN_C99_PAR_STRING_SELF(self);
    if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 0), &wanted)) return 0;
    if (argc > 1 && args[1] != KILN_VAL_UNDEFINED &&
        !kiln_c99_par_arg_text(args[1], buffer, &view, &fill)) {
        return 0;
    }
    if (wanted <= (double)s->unit_len || fill->unit_len == 0) {
        *out = self;
        return 1;
    }
    if (wanted > (double)KILN_C99_PAR_BUILT_UNITS) return 0;
    target = (size_t)wanted;
    pad = target - s->unit_len;
    utf16 = kiln_string_is_utf16(s) || kiln_string_is_utf16(fill);
    if (utf16) {
        uint16_t *units = (uint16_t *)kiln_c99_par_string_alloc(arena, target, 1, out);
        size_t base = at_start ? 0 : s->unit_len;
        size_t own = at_start ? pad : 0;
        if (units == NULL) return 0;
        for (k = 0; k < pad; ++k) units[base + k] = kiln_string_unit_at(fill, k % fill->unit_len);
        for (k = 0; k < s->unit_len; ++k) units[own + k] = kiln_string_unit_at(s, k);
    } else {
        char *bytes = (char *)kiln_c99_par_string_alloc(arena, target, 0, out);
        size_t base = at_start ? 0 : s->unit_len;
        size_t own = at_start ? pad : 0;
        if (bytes == NULL) return 0;
        for (k = 0; k < pad; ++k) bytes[base + k] = ((const char *)kiln_string_latin1(fill))[k % fill->unit_len];
        if (s->unit_len != 0) memcpy(bytes + own, kiln_string_latin1(s), s->unit_len);
    }
    return 1;
}

static int kiln_c99_par_m_padStart(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                   kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    (void)env;
    return kiln_c99_par_pad(arena, self, argc, args, 1, out);
}

static int kiln_c99_par_m_padEnd(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                 kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    (void)env;
    return kiln_c99_par_pad(arena, self, argc, args, 0, out);
}

static int kiln_c99_par_m_repeat(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                 kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    double count;
    size_t times;
    size_t total;
    size_t k;
    KILN_C99_PAR_STRING_SELF(self);
    (void)env;
    if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 0), &count)) return 0;
    /* A negative or infinite count is a RangeError, which the sequential run throws. */
    if (count < 0.0 || isinf(count)) return 0;
    if (count == 0.0 || s->unit_len == 0) {
        *out = kiln_c99_par_empty();
        return 1;
    }
    if (count > (double)KILN_C99_PAR_BUILT_UNITS / (double)s->unit_len) return 0;
    times = (size_t)count;
    if (times == 1) {
        *out = self;
        return 1;
    }
    total = times * s->unit_len;
    if (kiln_string_is_utf16(s)) {
        uint16_t *units = (uint16_t *)kiln_c99_par_string_alloc(arena, total, 1, out);
        if (units == NULL) return 0;
        for (k = 0; k < times; ++k) memcpy(units + k * s->unit_len, s->data, s->unit_len * sizeof(uint16_t));
    } else {
        char *bytes = (char *)kiln_c99_par_string_alloc(arena, total, 0, out);
        if (bytes == NULL) return 0;
        for (k = 0; k < times; ++k) memcpy(bytes + k * s->unit_len, kiln_string_latin1(s), s->unit_len);
    }
    return 1;
}

/* `ToUint32` of a limit. */
static uint32_t kiln_c99_par_uint32(double d) {
    double m;
    if (!isfinite(d) || d == 0.0) return 0;
    m = fmod(trunc(d), 4294967296.0);
    if (m < 0.0) m += 4294967296.0;
    return (uint32_t)m;
}

/* `split(separator, limit)` with a separator that is not an object (a `RegExp` splits by its own
 * `@@split`, which could be anyone's): the specification's loop over code units. */
static int kiln_c99_par_m_split(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    kiln_val parts[KILN_C99_ARRAY_INBLOCK_MAX];
    size_t count = 0;
    size_t limit = (size_t)0xFFFFFFFFu;
    kiln_val separator = kiln_c99_par_arg(argc, args, 0);
    char buffer[32];
    kiln_string view;
    const kiln_string *r;
    size_t i = 0;
    double j;
    KILN_C99_PAR_STRING_SELF(self);
    (void)env;
    if (KILN_VAL_IS_OBJECT(separator) || KILN_VAL_IS_CLOSURE(separator) ||
        (separator & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {
        return 0;
    }
    if (argc > 1 && args[1] != KILN_VAL_UNDEFINED) {
        double d;
        if (!kiln_c99_par_number(args[1], &d)) return 0;
        limit = kiln_c99_par_uint32(d);
    }
    if (separator != KILN_VAL_UNDEFINED && !kiln_c99_par_arg_text(separator, buffer, &view, &r)) return 0;
    if (limit == 0) return kiln_c99_par_array(arena, NULL, 0, out);
    if (separator == KILN_VAL_UNDEFINED) return kiln_c99_par_array(arena, &self, 1, out);
    if (r->unit_len == 0) {
        size_t taken = s->unit_len < limit ? s->unit_len : limit;
        size_t k;
        if (taken > KILN_C99_ARRAY_INBLOCK_MAX) return 0;
        for (k = 0; k < taken; ++k) {
            if (!kiln_c99_par_unit_string(arena, kiln_string_unit_at(s, k), &parts[k])) return 0;
        }
        return kiln_c99_par_array(arena, parts, taken, out);
    }
    if (s->unit_len == 0) return kiln_c99_par_array(arena, &self, 1, out);
    j = kiln_c99_par_find(s, r, 0);
    while (j >= 0.0) {
        size_t at = (size_t)j;
        if (count >= KILN_C99_ARRAY_INBLOCK_MAX) return 0;
        if (!kiln_c99_par_substring(arena, self, i, at - i, &parts[count])) return 0;
        count += 1;
        if (count == limit) return kiln_c99_par_array(arena, parts, count, out);
        i = at + r->unit_len;
        j = kiln_c99_par_find(s, r, i);
    }
    if (count >= KILN_C99_ARRAY_INBLOCK_MAX) return 0;
    if (!kiln_c99_par_substring(arena, self, i, s->unit_len - i, &parts[count])) return 0;
    count += 1;
    return kiln_c99_par_array(arena, parts, count, out);
}

/* `x.toString()`: a string is itself; a number is spelled in radix 10 (another radix declines). */
static int kiln_c99_par_m_toString(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                   kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    (void)env;
    if (KILN_VAL_IS_STRING(self)) {
        *out = self;
        return 1;
    }
    if (KILN_VAL_IS_NUMBER(self)) {
        if (argc > 0 && args[0] != KILN_VAL_UNDEFINED) {
            double radix;
            if (!KILN_VAL_IS_NUMBER(args[0])) return 0;
            memcpy(&radix, &args[0], sizeof radix);
            if (radix != 10.0) return 0;
        }
        return kiln_c99_par_to_string(arena, self, out);
    }
    return 0;
}

static int kiln_c99_par_m_toFixed(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                                  kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    double value;
    double places;
    char spelled[KILN_C99_FIXED_SPELLING_MAX];
    size_t length;
    char *bytes;
    (void)env;
    if (!KILN_VAL_IS_NUMBER(self)) return 0;
    memcpy(&value, &self, sizeof value);
    if (!kiln_c99_par_integer(kiln_c99_par_arg(argc, args, 0), &places)) return 0;
    /* Outside [0, 100] is a RangeError, which the sequential run throws. */
    if (!(places >= 0.0 && places <= 100.0)) return 0;
    if (!isfinite(value) || fabs(value) >= 1e21) return kiln_c99_par_to_string(arena, self, out);
    length = kiln_c99_fixed_spelling(value, (int)places, spelled);
    bytes = (char *)kiln_c99_par_string_alloc(arena, length, 0, out);
    if (bytes == NULL) return 0;
    memcpy(bytes, spelled, length);
    return 1;
}

/* `array.join(separator)` over elements that spell without running code: strings, numbers,
 * booleans, and undefined, null and holes as the empty string. */
static int kiln_c99_par_m_join(const kiln_c99_par_env *env, kiln_c99_par_arena *arena,
                               kiln_val self, size_t argc, const kiln_val *args, kiln_val *out) {
    const kiln_array *array;
    kiln_string comma = {1, KILN_STRING_LATIN1, ","};
    const kiln_string *separator = &comma;
    char buffer[32];
    kiln_string view;
    size_t total = 0;
    size_t k;
    int utf16 = 0;
    (void)env;
    if (!kiln_c99_par_plain_array(self, &array)) return 0;
    if (argc > 0 && args[0] != KILN_VAL_UNDEFINED &&
        !kiln_c99_par_arg_text(args[0], buffer, &view, &separator)) {
        return 0;
    }
    utf16 = kiln_string_is_utf16(separator);
    for (k = 0; k < array->count; ++k) {
        kiln_val e = array->elements[k];
        char spelled[32];
        kiln_string spelled_view;
        const kiln_string *part;
        if (k != 0) total += separator->unit_len;
        if (e == KILN_VAL_HOLE || e == KILN_VAL_UNDEFINED || e == KILN_VAL_NULL) continue;
        if (!KILN_VAL_IS_STRING(e) && !KILN_VAL_IS_NUMBER(e) && !KILN_VAL_IS_BOOLEAN(e)) return 0;
        if (!kiln_c99_par_text(e, spelled, &spelled_view, &part)) return 0;
        total += part->unit_len;
        utf16 |= kiln_string_is_utf16(part);
    }
    if (total > KILN_C99_PAR_BUILT_UNITS) return 0;
    if (total == 0) {
        *out = kiln_c99_par_empty();
        return 1;
    }
    {
        void *built = kiln_c99_par_string_alloc(arena, total, utf16, out);
        size_t at = 0;
        if (built == NULL) return 0;
        for (k = 0; k < array->count; ++k) {
            kiln_val e = array->elements[k];
            char spelled[32];
            kiln_string spelled_view;
            const kiln_string *part = NULL;
            size_t u;
            if (k != 0) {
                for (u = 0; u < separator->unit_len; ++u) {
                    if (utf16) ((uint16_t *)built)[at + u] = kiln_string_unit_at(separator, u);
                    else ((char *)built)[at + u] = (char)kiln_string_unit_at(separator, u);
                }
                at += separator->unit_len;
            }
            if (e == KILN_VAL_HOLE || e == KILN_VAL_UNDEFINED || e == KILN_VAL_NULL) continue;
            if (!kiln_c99_par_text(e, spelled, &spelled_view, &part)) return 0;
            for (u = 0; u < part->unit_len; ++u) {
                if (utf16) ((uint16_t *)built)[at + u] = kiln_string_unit_at(part, u);
                else ((char *)built)[at + u] = (char)kiln_string_unit_at(part, u);
            }
            at += part->unit_len;
        }
    }
    return 1;
}

#undef KILN_C99_PAR_STRING_SELF
