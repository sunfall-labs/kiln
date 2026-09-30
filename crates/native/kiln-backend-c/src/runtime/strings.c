
/* Bulk position-taking string methods count UTF-16 code units (ADR 0022), so they convert once,
 * work on units, and re-encode. One conversion pair rather than a dozen ad-hoc walks is what
 * keeps them agreeing with each other and with the interpreter. Single-unit access has a direct
 * iterator path below and does not need that temporary array.
 *
 * The caller owns the returned array and frees it. */
static int kiln_c99_to_units(kiln_value value, uint16_t **out, size_t *count) {
    const kiln_string *string = value.ptr == NULL ? NULL : (const kiln_string *)value.ptr;
    kiln_c99_utf16_iter iterator;
    uint16_t *units;
    size_t capacity;
    size_t length;
    uint16_t unit;
    /* ASCII text has one code unit per byte and no surrogate pairing to do, so the whole
     * decode state machine (kiln_c99_utf8_next by way of kiln_c99_utf16_next) is replaced with
     * a straight widening copy -- still O(len), but branch-free per byte instead of walking a
     * UTF-8 length table and a surrogate split for every unit. */
    if (string != NULL && !kiln_string_is_utf16(string)) {
        size_t index;
        if (string->unit_len == 0) {
            *out = NULL;
            *count = 0;
            return KILN_C99_OK;
        }
        units = (uint16_t *)malloc(string->unit_len * sizeof *units);
        if (units == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < string->unit_len; ++index) {
            units[index] = kiln_string_unit_at(string, index);
        }
        *out = units;
        *count = string->unit_len;
        return KILN_C99_OK;
    }
    if (string == NULL) { *out = NULL; *count = 0; return KILN_C99_OK; }
    if (kiln_string_is_utf16(string)) {
        if (string->unit_len == 0) { *out = NULL; *count = 0; return KILN_C99_OK; }
        units = (uint16_t *)malloc(string->unit_len * sizeof *units);
        if (units == NULL) return KILN_C99_OUT_OF_MEMORY;
        memcpy(units, string->data, string->unit_len * sizeof *units);
        *out = units; *count = string->unit_len; return KILN_C99_OK;
    }
    iterator.text = value.ptr;
    iterator.length = value.len;
    iterator.offset = 0;
    iterator.pending = 0;
    iterator.has_pending = 0;
    units = NULL;
    capacity = 0;
    length = 0;
    while (kiln_c99_utf16_next(&iterator, &unit)) {
        if (kiln_c99_reserve((void **)&units, &capacity, length + 1, sizeof *units) !=
            KILN_C99_OK) {
            free(units);
            return KILN_C99_OUT_OF_MEMORY;
        }
        units[length] = unit;
        length += 1;
    }
    *out = units;
    *count = length;
    return KILN_C99_OK;
}

/* Every unit below 0x80, 4 at a time: a code unit is ASCII exactly when its top 9 bits (the
 * surrogate range and the rest of the non-Latin1 unit space) are clear, so masking each 16-bit
 * lane's high byte down to the 0x80 bit and comparing the 64-bit word to zero answers for four
 * units at once. */
static int kiln_c99_units_ascii(const uint16_t *units, size_t count) {
    size_t index = 0;
    while (index + 4 <= count) {
        uint64_t word;
        memcpy(&word, units + index, sizeof word);
        if ((word & UINT64_C(0xFF80FF80FF80FF80)) != 0) return 0;
        index += 4;
    }
    while (index < count) {
        if (units[index] >= 0x80) return 0;
        ++index;
    }
    return 1;
}

/* Re-encode, pairing surrogates back up.  An *unpaired* surrogate has no scalar value, so it
 * becomes U+FFFD -- a kiln string cannot hold one, which is the divergence ADR 0022 records and
 * the interpreter's `String::from_utf16_lossy` makes the same substitution.
 *
 * When every unit is ASCII there is no pairing, no variable width and no growable builder to
 * drive: a unit is a byte, so this narrows straight into a fresh block and marks it known-ASCII
 * on the way out, which is what buys the *next* read of a slice or split result a free length
 * and a free re-decode (ADR 0041's block header is the cache; kiln_c99_string_is_ascii is the
 * reader). */
static kiln_value kiln_c99_from_units(const uint16_t *units, size_t count) {
    kiln_string *string;
    if (count == 0 || units == NULL) {
        return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)&kiln_c99_empty_string, 0);
    }
    if (kiln_c99_units_ascii(units, count)) {
        string = kiln_c99_gc_alloc_units(count, KILN_STRING_LATIN1);
        if (string == NULL) return kiln_c99_undefined();
        for (size_t index = 0; index < count; ++index)
            ((uint8_t *)(uintptr_t)string->data)[index] = (uint8_t)units[index];
    } else {
        string = kiln_c99_gc_alloc_units(count, KILN_STRING_UTF16);
        if (string == NULL) return kiln_c99_undefined();
        memcpy((void *)string->data, units, count * sizeof(uint16_t));
    }
    return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)string, count);
}

/* Zero-copy sliced string: points directly into parent string buffer with start offset and length.
 * The slice is backed by KILN_GC_SLICED_STRING (24 bytes) which roots parent, guaranteeing zero copies.
 * Chains of slices are flattened to the root parent on creation, keeping reference depth strictly <= 1. */
kiln_val kiln_c99_val_slice_string(const kiln_string *source, size_t start, size_t len) {
    if (source == NULL || len == 0 || start >= source->unit_len) {
        return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
    }
    if (len > source->unit_len - start) {
        len = source->unit_len - start;
    }
    if (len == 0) {
        return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
    }
    if (start == 0 && len == source->unit_len) {
        return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, source);
    }
    if (len == 1) {
        if (!kiln_string_is_utf16(source)) {
            return kiln_c99_val_single_char(((const unsigned char *)kiln_string_latin1(source))[start]);
        }
        uint16_t unit = ((const uint16_t *)source->data)[start];
        if (unit < 0x100u) {
            return kiln_c99_val_single_char((unsigned char)unit);
        }
    }
    while (kiln_string_is_sliced(source)) {
        const kiln_sliced_string *sliced_src = (const kiln_sliced_string *)source;
        if (kiln_string_is_utf16(source)) {
            start += (size_t)((const uint16_t *)source->data - (const uint16_t *)sliced_src->parent->data);
        } else {
            start += (size_t)((const char *)source->data - (const char *)sliced_src->parent->data);
        }
        source = sliced_src->parent;
    }
    kiln_sliced_string *sliced = (kiln_sliced_string *)kiln_c99_gc_alloc_hot(KILN_GC_SLICED_STRING, sizeof(kiln_sliced_string), 1);
    if (KILN_UNLIKELY(sliced == NULL)) {
        return KILN_VAL_UNDEFINED;
    }
    sliced->base.unit_len = (uint32_t)len;
    sliced->base.flags = (source->flags & (KILN_STRING_UTF16 | KILN_STRING_LATIN1)) | KILN_STRING_SLICED;
    if (kiln_string_is_utf16(source)) {
        kiln_c99_heap_store_pointer(sliced, &sliced->base.data, (const void *)(((const uint16_t *)source->data) + start));
    } else {
        kiln_c99_heap_store_pointer(sliced, &sliced->base.data, (const void *)(((const char *)source->data) + start));
    }
    kiln_c99_heap_store_pointer(sliced, &sliced->parent, source);
    return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &sliced->base);
}

/* Copy a range without crossing a codec boundary.  Latin1 storage is already one code unit per
 * byte, but those bytes are not UTF-8: passing a range through `kiln_c99_copy_text` would decode
 * 0xE9 (or any other high byte) as malformed input and replace it. */
static int kiln_c99_copy_string_slice(const kiln_string *source, size_t start, size_t end,
                                      kiln_value *out) {
    size_t count;
    kiln_string *copy;
    if (source == NULL || out == NULL || start > end || end > source->unit_len) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    count = end - start;
    if (count == 0) {
        *out = kiln_c99_static_text("", 0);
        return KILN_C99_OK;
    }
    copy = kiln_c99_gc_alloc_units(count, source->flags);
    if (copy == NULL) return KILN_C99_OUT_OF_MEMORY;
    if (kiln_string_is_utf16(source)) {
        memcpy((void *)copy->data, (const uint16_t *)source->data + start,
               count * sizeof(uint16_t));
    } else {
        memcpy((void *)copy->data, (const uint8_t *)source->data + start, count);
    }
    *out = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)copy, count);
    return KILN_C99_OK;
}

/* The first position at or after `from` where `needle` occurs, or -1. */
static double kiln_c99_units_find(const uint16_t *haystack, size_t haystack_len,
                                  const uint16_t *needle, size_t needle_len, size_t from) {
    size_t at;
    if (needle_len > haystack_len) {
        return -1.0;
    }
    for (at = from; at + needle_len <= haystack_len; ++at) {
        if (memcmp(haystack + at, needle, needle_len * sizeof *needle) == 0) {
            return (double)at;
        }
    }
    return -1.0;
}

/* `split`'s limit (parallel issue 30), for the two paths that find every piece first -- a regexp
 * separator, and a string separator on a UTF-16 receiver: the answer is the first `ToUint32(limit)`
 * of those pieces, which is exactly the array the specification's walk stops at, because it pushes
 * the pieces (captures included) in this order and stops when the count reaches the limit. The
 * Latin1 walk above reads the limit itself. */
static void kiln_c99_split_limit(kiln_value pieces, const kiln_value *args, size_t argc) {
    uint32_t lim;
    kiln_array *array;
    if (argc < 2 || args[1].tag == KILN_VALUE_UNDEFINED || pieces.tag != KILN_VALUE_ARRAY) return;
    lim = kiln_c99_to_uint32(kiln_c99_as_number(args[1]));
    array = kiln_c99_array_of(pieces);
    if (array->count > (size_t)lim) array->count = (size_t)lim;
}

/* A code unit that `trim` removes: the specification's WhiteSpace plus LineTerminator. */
static int kiln_c99_is_trimmed(uint16_t unit) {
    switch (unit) {
    case 0x09: case 0x0A: case 0x0B: case 0x0C: case 0x0D: case 0x20:
    case 0xA0: case 0x1680: case 0x2000: case 0x2001: case 0x2002: case 0x2003:
    case 0x2004: case 0x2005: case 0x2006: case 0x2007: case 0x2008: case 0x2009:
    case 0x200A: case 0x2028: case 0x2029: case 0x202F: case 0x205F: case 0x3000:
    case 0xFEFF:
        return 1;
    default:
        return 0;
    }
}

/* `includes` uses SameValueZero, which differs from `===` in exactly one place: NaN matches
 * itself.  `indexOf` uses `===` and so never finds one. */
static int kiln_c99_same_value_zero(kiln_value left, kiln_value right) {
    if (left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_NUMBER) {
        double a = kiln_c99_as_number(left);
        double b = kiln_c99_as_number(right);
        if (isnan(a) && isnan(b)) {
            return 1;
        }
    }
    return kiln_c99_strict_equal(left, right);
}

/* A relative index: negative counts from the end, and the result is clamped into range.  This is
 * `slice`'s rule and it is shared by every method that takes one. */
static size_t kiln_c99_relative_index(kiln_value value, size_t length, size_t fallback) {
    double index;
    if (value.tag == KILN_VALUE_UNDEFINED) {
        return fallback;
    }
    index = kiln_c99_as_number(value);
    if (isnan(index)) {
        return 0;
    }
    if (index < 0.0) {
        index += (double)length;
        return index < 0.0 ? 0 : (size_t)index;
    }
    return index > (double)length ? length : (size_t)index;
}

/* A position argument in code units: negative counts from the end for `slice` and `at`, and
 * `substring` clamps instead -- which is `relative` false. */
static size_t kiln_c99_unit_position(kiln_value value, size_t length, size_t fallback,
                                     int relative) {
    double index;
    if (value.tag == KILN_VALUE_UNDEFINED) {
        return fallback;
    }
    index = kiln_c99_as_number(value);
    if (isnan(index) || index < 0.0) {
        if (!relative || isnan(index)) {
            return 0;
        }
        index += (double)length;
        return index < 0.0 ? 0 : (size_t)index;
    }
    return index > (double)length ? length : (size_t)index;
}


/* `split` on a one-byte separator over text whose code units are its bytes.
 *
 * Every string method has a shape it is asked for far more often than the rest, and for `split`
 * it is this one: an ASCII line cut on a single ASCII byte. Both the emitted C's intrinsic and
 * the dispatch below used to spell it out, and both spelled it out the expensive way -- a byte
 * at a time to count the parts, then a fresh trip through the allocator per part, in one case
 * through `kiln_c99_copy_text` and a second copy on top. Splitting a nine-field log line spent
 * more of its time going in and out of the allocator than it spent copying bytes.
 *
 * One pass now: `kiln_c99_find_byte` finds every separator and the parts are recorded as they are
 * found, so the count that sizes the array falls out of the scan rather than costing a scan of
 * its own.
 * The array's elements come out of the array's own block (`kiln_c99_new_array_with_capacity`),
 * and the parts themselves out of one nursery reservation (`kiln_c99_gc_alloc_text_run`), so a
 * split of nine takes two trips through the allocator rather than eleven.
 *
 * `limit` truncates: the tail after the last kept piece is dropped rather than merged into it,
 * so the last kept piece is searched for like every other. A string with more parts than the
 * scan buffer holds falls back to the growable path, which answers identically and is bounded by
 * the array rather than by the stack. */
/* Zero-copy split: splits an ASCII/Latin1 string by a 1-byte separator using sliced strings.
 * Each part is an O(1) slice pointing directly into source->data without allocating text buffers. */
int kiln_c99_split_string_ascii_byte(kiln_value *result, const kiln_string *source,
                                      char separator, size_t limit) {
    enum { KILN_C99_SPLIT_SCAN = 64 };
    const char *starts[KILN_C99_SPLIT_SCAN];
    size_t lens[KILN_C99_SPLIT_SCAN];
    const char *bytes = source != NULL ? (const char *)kiln_string_latin1(source) : "";
    size_t byte_len = source != NULL ? source->unit_len : 0;
    const char *p = bytes;
    const char *end = bytes + byte_len;
    size_t count = 0;
    size_t index;
    int exhausted = 0;
    kiln_value built;
    kiln_array *array;
    int status;

    if (limit == 0) {
        return kiln_c99_new_array_with_capacity(result, 0, 0);
    }

    while (count < KILN_C99_SPLIT_SCAN && count < limit) {
        const char *next = kiln_c99_find_byte(p, end, separator);
        size_t len = next != NULL ? (size_t)(next - p) : (size_t)(end - p);
        starts[count] = p;
        lens[count] = len;
        count += 1;
        if (next == NULL) {
            exhausted = 1;
            break;
        }
        p = next + 1;
    }

    if (!exhausted && count < limit) {
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) return status;
        array = kiln_c99_array_of(built);
        array->is_numeric = 0;
        p = bytes;
        for (index = 0; index < limit; ++index) {
            const char *next = kiln_c99_find_byte(p, end, separator);
            size_t len = next != NULL ? (size_t)(next - p) : (size_t)(end - p);
            status = kiln_c99_array_reserve(array, array->count + 1);
            if (status != KILN_C99_OK) return status;
            kiln_val part = kiln_c99_val_slice_string(source, (size_t)(p - bytes), len);
            if (part == KILN_VAL_UNDEFINED) return KILN_C99_OUT_OF_MEMORY;
            kiln_c99_heap_store_val(array, &array->elements[array->count], part);
            array->count += 1;
            if (next == NULL) break;
            p = next + 1;
        }
        *result = built;
        return KILN_C99_OK;
    }

    status = kiln_c99_new_array_with_capacity(&built, count, 0);
    if (status != KILN_C99_OK) return status;
    array = kiln_c99_array_of(built);
    {
        kiln_val *slots = array->elements;
        for (index = 0; index < count; ++index) {
            kiln_val part = kiln_c99_val_slice_string(source, (size_t)(starts[index] - bytes), lens[index]);
            if (part == KILN_VAL_UNDEFINED) {
                array->count = index;
                return KILN_C99_OUT_OF_MEMORY;
            }
            kiln_c99_heap_store_val(array, &slots[index], part);
            array->count = index + 1;
        }
    }
    *result = built;
    return KILN_C99_OK;
}

static int kiln_c99_split_ascii_byte(kiln_value *result, const char *bytes, size_t byte_len,
                                     char separator, size_t limit) {
    enum { KILN_C99_SPLIT_SCAN = 64 };
    const char *starts[KILN_C99_SPLIT_SCAN];
    size_t lens[KILN_C99_SPLIT_SCAN];
    size_t run_lens[KILN_C99_SPLIT_SCAN];
    char *places[KILN_C99_SPLIT_SCAN];
    const char *p = bytes;
    const char *end = bytes + byte_len;
    size_t count = 0;
    size_t run_count = 0;
    size_t placed = 0;
    size_t taken = 0;
    size_t index;
    int exhausted = 0;
    kiln_value built;
    kiln_array *array;
    int status;

    /* A part of no bytes and a part of one byte both answer with a string that is already made
     * and need no block, so the reservation is gathered here and covers only the parts that do. */
    while (count < KILN_C99_SPLIT_SCAN && count < limit) {
        const char *next = kiln_c99_find_byte(p, end, separator);
        size_t len = next != NULL ? (size_t)(next - p) : (size_t)(end - p);
        starts[count] = p;
        lens[count] = len;
        if (len > 1) run_lens[run_count++] = len;
        count += 1;
        if (next == NULL) {
            exhausted = 1;
            break;
        }
        p = next + 1;
    }
    if (!exhausted && count < limit) {
        /* More parts than the scan buffer holds. Rare enough to be worth no second
         * implementation: push into a growable array, which is where the piece count stops
         * being a stack question. */
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) return status;
        array = kiln_c99_array_of(built);
        array->is_numeric = 0;
        p = bytes;
        for (index = 0; index < limit; ++index) {
            const char *next = kiln_c99_find_byte(p, end, separator);
            size_t len = next != NULL ? (size_t)(next - p) : (size_t)(end - p);
            kiln_val part;
            /* Room first, then the part: the part's own allocation can collect, and a slot the
             * count already covers would be traced before anything had been written into it. */
            status = kiln_c99_array_reserve(array, array->count + 1);
            if (status != KILN_C99_OK) return status;
            part = kiln_c99_val_from_ascii(p, len);
            if (part == KILN_VAL_UNDEFINED) return KILN_C99_OUT_OF_MEMORY;
            kiln_c99_heap_store_val(array, &array->elements[array->count], part);
            array->count += 1;
            if (next == NULL) break;
            p = next + 1;
        }
        *result = built;
        return KILN_C99_OK;
    }

    status = kiln_c99_new_array_with_capacity(&built, count, 0);
    if (status != KILN_C99_OK) return status;
    array = kiln_c99_array_of(built);
    if (run_count > 0) {
        placed = kiln_c99_gc_alloc_text_run(run_count, run_lens, places);
    }
    {
        kiln_val *slots = array->elements;
        for (index = 0; index < count; ++index) {
            kiln_val part;
            size_t len = lens[index];
            if (len > 1 && taken < placed) {
                /* The reservation's blocks are handed out in the order they were asked for, so
                 * the cursor and the parts that wanted one advance together. */
                char *into = places[taken++];
                kiln_c99_copy_short(into, starts[index], len);
                part = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, ((kiln_string *)into) - 1);
            } else if (len == 0) {
                part = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
            } else if (len == 1) {
                part = kiln_c99_val_single_char((unsigned char)starts[index][0]);
            } else {
                part = kiln_c99_val_from_ascii(starts[index], len);
                if (part == KILN_VAL_UNDEFINED) {
                    array->count = index;
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            kiln_c99_heap_store_val(array, &slots[index], part);
            /* The count follows the write, so a collection inside the fallback above never
             * traces a slot nothing has been put in. */
            array->count = index + 1;
        }
    }
    *result = built;
    return KILN_C99_OK;
}

/* The string methods.  Split out from the array ones because they share nothing but the dispatch
 * -- every one of these works on code units and every one of those works on elements. */
static int kiln_c99_invoke_string_method(kiln_host *host, kiln_value *result, uint64_t code,
                                         kiln_value receiver, const kiln_value *args, size_t argc,
                                         int depth, kiln_value *thrown) {
    const kiln_string *receiver_string = receiver.ptr == NULL ? &kiln_c99_empty_string : (const kiln_string *)receiver.ptr;
    /* These methods need one UTF-16 code unit, not a materialised string. Keep argument
     * coercion here so a numeric index is never first converted to text merely because the
     * bulk path prepares a possible needle. */
    if (code == KILN_C99_METHOD_CHAR_AT || code == KILN_C99_METHOD_CHAR_CODE_AT ||
        code == KILN_C99_METHOD_STRING_AT) {
        double index = 0.0;
        int index_status = argc > 0 ? kiln_c99_to_number_checked(host, args[0], &index, depth,
                                                                  thrown)
                                    : KILN_C99_OK;
        uint16_t unit;
        if (index_status != KILN_C99_OK) return index_status;
        if (isnan(index)) index = 0.0;
        index = trunc(index);
        if (code == KILN_C99_METHOD_STRING_AT && index < 0.0) {
            double length = (double)kiln_c99_string_length(receiver);
            index += length;
        }
        if (!isfinite(index) || index < 0.0 || index >= (double)SIZE_MAX ||
            !kiln_c99_utf16_unit_at(receiver, (size_t)index, &unit)) {
            *result = code == KILN_C99_METHOD_CHAR_AT      ? kiln_c99_from_units(NULL, 0)
                      : code == KILN_C99_METHOD_STRING_AT  ? kiln_c99_undefined()
                                                           : kiln_c99_number(0.0 / 0.0);
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_CHAR_CODE_AT) {
            *result = kiln_c99_number((double)unit);
        } else {
            *result = kiln_val_to_boundary(kiln_c99_val_from_unit(unit));
        }
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_STRING_TO_STRING ||
        code == KILN_C99_METHOD_STRING_VALUE_OF) {
        *result = receiver;
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_STRING_NORMALIZE) {
        if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED) {
            kiln_value shown = kiln_c99_text(args[0]);
            if (shown.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            const kiln_string *fstr = shown.ptr == NULL ? &kiln_c99_empty_string : (const kiln_string *)shown.ptr;
            static const struct { const char *name; size_t len; } forms[4] = {
                {"NFC", 3}, {"NFD", 3}, {"NFKC", 4}, {"NFKD", 4}
            };
            uint32_t which;
            for (which = 0; which < 4; ++which) {
                if (kiln_string_equals_ascii(fstr, forms[which].name, forms[which].len)) {
                    break;
                }
            }
            if (which == 4) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                             "The normalization form should be one of NFC, NFD, NFKC, NFKD.");
            }
        }
        if (receiver_string->unit_len == 0 || kiln_c99_string_is_ascii(receiver)) {
            *result = receiver;
            return KILN_C99_OK;
        }
    }

    if (code == KILN_C99_METHOD_STRING_LOCALE_COMPARE) {
        kiln_value locales = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_value options = argc > 2 ? args[2] : kiln_c99_undefined();
        if (locales.tag == KILN_VALUE_UNDEFINED && options.tag == KILN_VALUE_UNDEFINED) {
            kiln_value other = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            if (other.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            const kiln_string *other_string = other.ptr == NULL ? &kiln_c99_empty_string : (const kiln_string *)other.ptr;
            if (receiver.ptr == other.ptr || kiln_string_equal(receiver_string, other_string)) {
                *result = kiln_c99_number(0.0);
                return KILN_C99_OK;
            }
        }
    }

    if (code == KILN_C99_METHOD_STRING_CODE_POINT_AT) {
        double position = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        if (position != position) position = 0.0;
        position = trunc(position);
        if (position < 0.0 || position >= (double)receiver_string->unit_len) {
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        size_t pos = (size_t)position;
        uint16_t lead = kiln_string_unit_at(receiver_string, pos);
        if (lead >= 0xD800 && lead <= 0xDBFF && pos + 1 < receiver_string->unit_len) {
            uint16_t trail = kiln_string_unit_at(receiver_string, pos + 1);
            if (trail >= 0xDC00 && trail <= 0xDFFF) {
                *result = kiln_c99_number(
                    (double)(0x10000u + (((uint32_t)lead - 0xD800u) << 10) +
                             ((uint32_t)trail - 0xDC00u)));
                return KILN_C99_OK;
            }
        }
        *result = kiln_c99_number((double)lead);
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_STRING_SLICE || code == KILN_C99_METHOD_SUBSTRING) {
        int relative = (code == KILN_C99_METHOD_STRING_SLICE);
        size_t start = kiln_c99_unit_position(argc > 0 ? args[0] : kiln_c99_undefined(), receiver_string->unit_len, 0, relative);
        size_t end = kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), receiver_string->unit_len, receiver_string->unit_len, relative);
        if (!relative && start > end) {
            size_t swap = start;
            start = end;
            end = swap;
        } else if (relative && start > end) {
            /* `slice` clamps a reversed range to the start; only `substring` swaps it. */
            end = start;
        }
        kiln_val v = kiln_c99_val_slice_string(receiver_string, start, end > start ? end - start : 0);
        if (v == KILN_VAL_UNDEFINED) return KILN_C99_OUT_OF_MEMORY;
        *result = kiln_val_to_boundary(v);
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_STRING_SUBSTR) {
        double relative = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        size_t start;
        size_t taken;
        if (relative != relative) relative = 0.0;
        relative = trunc(relative);
        if (relative < 0.0) {
            double from = (double)receiver_string->unit_len + relative;
            start = from < 0.0 ? 0 : (size_t)from;
        } else {
            start = relative > (double)receiver_string->unit_len ? receiver_string->unit_len : (size_t)relative;
        }
        if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
            double wanted = kiln_c99_as_number(args[1]);
            if (isnan(wanted) || wanted < 0.0) {
                taken = 0;
            } else {
                taken = wanted > (double)(receiver_string->unit_len - start) ? receiver_string->unit_len - start : (size_t)wanted;
            }
        } else {
            taken = receiver_string->unit_len - start;
        }
        kiln_val v = kiln_c99_val_slice_string(receiver_string, start, taken);
        if (v == KILN_VAL_UNDEFINED) return KILN_C99_OUT_OF_MEMORY;
        *result = kiln_val_to_boundary(v);
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_TRIM || code == KILN_C99_METHOD_TRIM_START || code == KILN_C99_METHOD_TRIM_END) {
        size_t start = 0;
        size_t end = receiver_string->unit_len;
        if (code != KILN_C99_METHOD_TRIM_END) {
            while (start < end && kiln_c99_is_trimmed(kiln_string_unit_at(receiver_string, start))) {
                start += 1;
            }
        }
        if (code != KILN_C99_METHOD_TRIM_START) {
            while (end > start && kiln_c99_is_trimmed(kiln_string_unit_at(receiver_string, end - 1))) {
                end -= 1;
            }
        }
        kiln_val v = kiln_c99_val_slice_string(receiver_string, start, end > start ? end - start : 0);
        if (v == KILN_VAL_UNDEFINED) return KILN_C99_OUT_OF_MEMORY;
        *result = kiln_val_to_boundary(v);
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_STARTS_WITH || code == KILN_C99_METHOD_ENDS_WITH ||
        code == KILN_C99_METHOD_STRING_INDEX_OF || code == KILN_C99_METHOD_STRING_INCLUDES ||
        code == KILN_C99_METHOD_LAST_INDEX_OF) {
        kiln_value needle_arg = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value shown = kiln_c99_text(needle_arg);
        if (shown.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        const kiln_string *shown_string = shown.ptr == NULL ? &kiln_c99_empty_string : (const kiln_string *)shown.ptr;
        size_t h_len = receiver_string->unit_len;
        size_t n_len = shown_string->unit_len;
        int h_u16 = kiln_string_is_utf16(receiver_string);
        int n_u16 = kiln_string_is_utf16(shown_string);

        if (code == KILN_C99_METHOD_STARTS_WITH) {
            size_t start = kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), h_len, 0, 0);
            if (n_len > h_len - start) {
                *result = kiln_c99_boolean(0);
                return KILN_C99_OK;
            }
            if (!h_u16 && !n_u16) {
                *result = kiln_c99_boolean(memcmp((const char *)kiln_string_latin1(receiver_string) + start, (const char *)kiln_string_latin1(shown_string), n_len) == 0);
                return KILN_C99_OK;
            }
            if (h_u16 && n_u16) {
                *result = kiln_c99_boolean(memcmp((const uint16_t *)receiver_string->data + start, (const uint16_t *)shown_string->data, n_len * sizeof(uint16_t)) == 0);
                return KILN_C99_OK;
            }
            int match = 1;
            for (size_t i = 0; i < n_len; ++i) {
                if (kiln_string_unit_at(receiver_string, start + i) != kiln_string_unit_at(shown_string, i)) {
                    match = 0;
                    break;
                }
            }
            *result = kiln_c99_boolean(match);
            return KILN_C99_OK;
        }

        if (code == KILN_C99_METHOD_ENDS_WITH) {
            size_t end = kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), h_len, h_len, 0);
            if (n_len > end) {
                *result = kiln_c99_boolean(0);
                return KILN_C99_OK;
            }
            size_t start = end - n_len;
            if (!h_u16 && !n_u16) {
                *result = kiln_c99_boolean(memcmp((const char *)kiln_string_latin1(receiver_string) + start, (const char *)kiln_string_latin1(shown_string), n_len) == 0);
                return KILN_C99_OK;
            }
            if (h_u16 && n_u16) {
                *result = kiln_c99_boolean(memcmp((const uint16_t *)receiver_string->data + start, (const uint16_t *)shown_string->data, n_len * sizeof(uint16_t)) == 0);
                return KILN_C99_OK;
            }
            int match = 1;
            for (size_t i = 0; i < n_len; ++i) {
                if (kiln_string_unit_at(receiver_string, start + i) != kiln_string_unit_at(shown_string, i)) {
                    match = 0;
                    break;
                }
            }
            *result = kiln_c99_boolean(match);
            return KILN_C99_OK;
        }

        if (code == KILN_C99_METHOD_STRING_INDEX_OF || code == KILN_C99_METHOD_STRING_INCLUDES) {
            size_t from = kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), h_len, 0, 0);
            if (n_len == 0) {
                double at = from > h_len ? (double)h_len : (double)from;
                *result = (code == KILN_C99_METHOD_STRING_INDEX_OF) ? kiln_c99_number(at) : kiln_c99_boolean(1);
                return KILN_C99_OK;
            }
            if (from >= h_len || n_len > h_len - from) {
                *result = (code == KILN_C99_METHOD_STRING_INDEX_OF) ? kiln_c99_number(-1.0) : kiln_c99_boolean(0);
                return KILN_C99_OK;
            }
            double at = -1.0;
            if (!h_u16 && !n_u16) {
                const char *found = kiln_c99_memmem((const char *)kiln_string_latin1(receiver_string) + from, h_len - from, (const char *)kiln_string_latin1(shown_string), n_len);
                at = found != NULL ? (double)(found - (const char *)kiln_string_latin1(receiver_string)) : -1.0;
            } else if (h_u16 && n_u16) {
                at = kiln_c99_units_find((const uint16_t *)receiver_string->data, h_len, (const uint16_t *)shown_string->data, n_len, from);
            } else {
                uint16_t first = kiln_string_unit_at(shown_string, 0);
                size_t max_start = h_len - n_len;
                for (size_t i = from; i <= max_start; ++i) {
                    if (kiln_string_unit_at(receiver_string, i) == first) {
                        size_t j = 1;
                        while (j < n_len && kiln_string_unit_at(receiver_string, i + j) == kiln_string_unit_at(shown_string, j)) ++j;
                        if (j == n_len) { at = (double)i; break; }
                    }
                }
            }
            if (code == KILN_C99_METHOD_STRING_INDEX_OF) {
                *result = kiln_c99_number(at);
            } else {
                *result = kiln_c99_boolean(at >= 0.0);
            }
            return KILN_C99_OK;
        }

        if (code == KILN_C99_METHOD_LAST_INDEX_OF) {
            double requested = argc > 1 ? kiln_c99_as_number(args[1]) : 0.0 / 0.0;
            size_t limit;
            if (argc <= 1 || isnan(requested)) {
                limit = h_len;
            } else if (requested < 0.0) {
                limit = 0;
            } else {
                limit = requested > (double)h_len ? h_len : (size_t)requested;
            }
            if (n_len == 0) {
                *result = kiln_c99_number((double)limit);
                return KILN_C99_OK;
            }
            if (n_len > h_len) {
                *result = kiln_c99_number(-1.0);
                return KILN_C99_OK;
            }
            size_t max_start = h_len - n_len;
            if (limit > max_start) limit = max_start;
            double at = -1.0;
            for (size_t i = limit + 1; i > 0; --i) {
                size_t pos = i - 1;
                int match = 1;
                for (size_t j = 0; j < n_len; ++j) {
                    if (kiln_string_unit_at(receiver_string, pos + j) != kiln_string_unit_at(shown_string, j)) {
                        match = 0;
                        break;
                    }
                }
                if (match) {
                    at = (double)pos;
                    break;
                }
            }
            *result = kiln_c99_number(at);
            return KILN_C99_OK;
        }
    }

    if (receiver_string != NULL && !kiln_string_is_utf16(receiver_string)) {
        if (code == KILN_C99_METHOD_SPLIT) {
            kiln_value sep = argc > 0 ? args[0] : kiln_c99_undefined();
            /* A limit of zero answers the empty array before anything else is asked. `ToUint32`
             * is why NaN reads as zero and a negative limit reads as unlimited. */
            size_t lim = (size_t)0xFFFFFFFFu;
            if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
                lim = (size_t)kiln_c99_to_uint32(kiln_c99_as_number(args[1]));
            }
            if (lim == 0) {
                kiln_value built;
                int st = kiln_c99_new_array_with_capacity(&built, 0, 0);
                if (st != KILN_C99_OK) return st;
                *result = built;
                return KILN_C99_OK;
            }
            if (sep.tag == KILN_VALUE_UNDEFINED) {
                kiln_value built;
                int st = kiln_c99_new_array_with_capacity(&built, 1, 0);
                if (st != KILN_C99_OK) return st;
                kiln_array *arr = kiln_c99_array_of(built);
                kiln_c99_heap_store_val(arr, &arr->elements[arr->count++], kiln_val_from_boundary(receiver));
                *result = built;
                return KILN_C99_OK;
            }
            kiln_value shown = kiln_c99_text(sep);
            const kiln_string *shown_string = shown.ptr == NULL ? NULL : (const kiln_string *)shown.ptr;
            if (shown.tag == KILN_VALUE_STRING && shown_string != NULL && !kiln_string_is_utf16(shown_string)) {
                kiln_value built;
                if (shown.len == 0) {
                    size_t taken = receiver_string->unit_len < lim ? receiver_string->unit_len : lim;
                    int st = kiln_c99_new_array_with_capacity(&built, taken, 0);
                    if (st != KILN_C99_OK) return st;
                    kiln_array *arr = kiln_c99_array_of(built);
                    for (size_t i = 0; i < taken; ++i) {
                        kiln_c99_heap_store_val(arr, &arr->elements[arr->count++], kiln_c99_val_from_unit(kiln_string_unit_at(receiver_string, i)));
                    }
                    *result = built;
                    return KILN_C99_OK;
                }
                if (shown.len == 1) {
                    return kiln_c99_split_string_ascii_byte(result, receiver_string,
                                                           (char)kiln_string_unit_at(shown_string, 0), lim);
                }
                /* Counted first and written once, directly, never through the growable
                 * `kiln_c99_array_push`: that reallocs the array's element pointer, and an
                 * inline-capacity array's element pointer is not a heap block to realloc. */
                {
                    size_t part_count = 1;
                    const char *scan = (const char *)kiln_string_latin1(receiver_string);
                    const char *scan_end = scan + receiver_string->unit_len;
                    while (scan <= scan_end && part_count < lim) {
                        const char *next =
                            kiln_c99_memmem(scan, (size_t)(scan_end - scan), (const char *)kiln_string_latin1(shown_string), shown_string->unit_len);
                        if (next == NULL) {
                            break;
                        }
                        part_count += 1;
                        scan = next + shown_string->unit_len;
                    }
                    int st = kiln_c99_new_array_with_capacity(&built, part_count, 0);
                    if (st != KILN_C99_OK) return st;
                    {
                        /* A truncating limit drops the tail rather than merging it into the
                         * last kept piece, so the last piece is searched for like every other.
                         *
                         * Sliced strings point directly into receiver_string without copying. */
                        kiln_array *arr = kiln_c99_array_of(built);
                        const char *p = (const char *)kiln_string_latin1(receiver_string);
                        const char *end = p + receiver_string->unit_len;
                        while (arr->count < part_count) {
                            const char *next =
                                kiln_c99_memmem(p, (size_t)(end - p), (const char *)kiln_string_latin1(shown_string), shown_string->unit_len);
                            size_t len = next != NULL ? (size_t)(next - p) : (size_t)(end - p);
                            kiln_val part = kiln_c99_val_slice_string(receiver_string, (size_t)(p - (const char *)kiln_string_latin1(receiver_string)), len);
                            if (part == KILN_VAL_UNDEFINED) return KILN_C99_OUT_OF_MEMORY;
                            kiln_c99_heap_store_val(arr, &arr->elements[arr->count], part);
                            arr->count += 1;
                            if (next == NULL) break;
                            p = next + shown_string->unit_len;
                        }
                    }
                    *result = built;
                    return KILN_C99_OK;
                }
            }
        }
    }

    uint16_t *units = NULL;
    uint16_t *needle = NULL;
    size_t count = 0;
    size_t needle_count = 0;
    int status = kiln_c99_to_units(receiver, &units, &count);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* Most of these take a string argument, and converting it once here keeps each case short.
     * A case that does not want one simply ignores it. */
    if (argc > 0) {
        kiln_value shown = kiln_c99_text(args[0]);
        if (shown.tag == KILN_VALUE_STRING) {
            status = kiln_c99_to_units(shown, &needle, &needle_count);
            if (status != KILN_C99_OK) {
                free(units);
                return status;
            }
        }
    }
    status = KILN_C99_OK;
    switch (code) {
    case KILN_C99_METHOD_STRING_SLICE:
    case KILN_C99_METHOD_SUBSTRING: {
        int relative = code == KILN_C99_METHOD_STRING_SLICE;
        size_t start = kiln_c99_unit_position(argc > 0 ? args[0] : kiln_c99_undefined(), count, 0,
                                              relative);
        size_t end = kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), count,
                                            count, relative);
        /* `substring` swaps its ends when they are the wrong way round; `slice` yields nothing.
         * The two look alike and agree on almost no edge case. */
        if (!relative && start > end) {
            size_t swap = start;
            start = end;
            end = swap;
        }
        *result = start >= end ? kiln_c99_from_units(NULL, 0)
                               : kiln_c99_from_units(units + start, end - start);
        break;
    }
    /* All five of these take a *position* as their second argument, and all five used to
     * ignore it -- so `"abcabc".indexOf("c", 3)` answered 2, which is not merely the wrong
     * number but a number from before the search was asked to start. */
    case KILN_C99_METHOD_STRING_INDEX_OF:
        *result = kiln_c99_number(kiln_c99_units_find(
            units, count, needle, needle_count,
            kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), count, 0, 0)));
        break;
    case KILN_C99_METHOD_LAST_INDEX_OF: {
        double found = -1.0;
        double at;
        size_t from = 0;
        /* The one position argument in the language where NaN means the *end* of the string
         * rather than the start: `lastIndexOf` is specified as `+Infinity` for a NaN, which is
         * also what a missing argument gives, so both search the whole string. */
        double requested = argc > 1 ? kiln_c99_as_number(args[1]) : 0.0 / 0.0;
        size_t limit;
        if (argc <= 1 || isnan(requested)) {
            limit = count;
        } else if (requested < 0.0) {
            limit = 0;
        } else {
            limit = requested > (double)count ? count : (size_t)requested;
        }
        while ((at = kiln_c99_units_find(units, count, needle, needle_count, from)) >= 0.0) {
            if (at > (double)limit) {
                break;
            }
            found = at;
            from = (size_t)at + 1;
        }
        *result = kiln_c99_number(found);
        break;
    }
    case KILN_C99_METHOD_STRING_INCLUDES:
        *result = kiln_c99_boolean(
            kiln_c99_units_find(
                units, count, needle, needle_count,
                kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), count, 0, 0)) >=
            0.0);
        break;
    case KILN_C99_METHOD_STARTS_WITH: {
        size_t start = kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), count, 0,
                                              0);
        *result = kiln_c99_boolean(needle_count <= count - start &&
                                   memcmp(units + start, needle,
                                          needle_count * sizeof *units) == 0);
        break;
    }
    case KILN_C99_METHOD_ENDS_WITH: {
        /* `endsWith`'s position is where the string is treated as *ending*, so it defaults to
         * the whole length rather than to zero. */
        size_t end = kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), count,
                                            count, 0);
        *result = kiln_c99_boolean(needle_count <= end &&
                                   memcmp(units + end - needle_count, needle,
                                          needle_count * sizeof *units) == 0);
        break;
    }
    case KILN_C99_METHOD_STRING_TO_STRING:
    case KILN_C99_METHOD_STRING_VALUE_OF:
        *result = receiver;
        break;
    case KILN_C99_METHOD_STRING_NORMALIZE: {
        /* The tables are ICU4X's, through the provider (`kiln_string_normalize`), the same ones
         * the interpreter answers from. This was the identity until 2026-09-19, which was
         * honest for a bundler's output and wrong for `ﬁ` and every combining sequence. */
        uint32_t form = 0;
        if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED) {
            static const char *const forms[] = {"NFC", "NFD", "NFKC", "NFKD"};
            uint32_t which;
            for (which = 0; which < 4; ++which) {
                size_t len = strlen(forms[which]);
                size_t at;
                if (needle == NULL || needle_count != len) continue;
                for (at = 0; at < len && needle[at] == (uint16_t)forms[which][at]; ++at) {}
                if (at == len) break;
            }
            if (which == 4) {
                status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                              "The normalization form should be one of NFC, NFD, NFKC, NFKD.");
                break;
            }
            form = which;
        }
        if (count == 0 || kiln_c99_string_is_ascii(receiver)) {
            *result = receiver;
            break;
        }
        {
            uint64_t need = 0;
            uint64_t cap = (uint64_t)count * 3 + 8;
            uint16_t *mapped = (uint16_t *)malloc((size_t)cap * sizeof *mapped);
            int rc;
            if (mapped == NULL) {
                status = KILN_C99_OUT_OF_MEMORY;
                break;
            }
            rc = kiln_string_normalize(units, (uint64_t)count, form, mapped, cap, &need);
            if (rc == KILN_C99_INVALID_ARGUMENT && need > cap) {
                uint16_t *bigger = (uint16_t *)realloc(mapped, (size_t)need * sizeof *mapped);
                if (bigger == NULL) {
                    free(mapped);
                    status = KILN_C99_OUT_OF_MEMORY;
                    break;
                }
                mapped = bigger;
                cap = need;
                rc = kiln_string_normalize(units, (uint64_t)count, form, mapped, cap, &need);
            }
            if (rc != KILN_C99_OK) {
                free(mapped);
                free(units);
                free(needle);
                KILN_C99_REFUSE("the provider would not normalize this string");
            }
            *result = kiln_c99_from_units(mapped, (size_t)need);
            free(mapped);
        }
        break;
    }
    case KILN_C99_METHOD_STRING_LOCALE_COMPARE: {
        /* Collation is ICU4X's through the provider (`kiln_intl_collate`), the collator the
         * interpreter's `Intl.Collator` is built on. The spec is the seven fields the provider
         * documents, U+001F between them, and doubles as its cache key; until 2026-09-19 this
         * compared code units, which put `C` before `a` in a sort node did not. */
        uint8_t spec[1024];
        size_t spec_len = 0;
        int spec_ok = 1;
        int32_t order = 0;
        int rc;
        kiln_value locales = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_value options = argc > 2 ? args[2] : kiln_c99_undefined();
        if (locales.tag == KILN_VALUE_UNDEFINED && options.tag == KILN_VALUE_UNDEFINED &&
            count == needle_count && (count == 0 || (units != NULL && needle != NULL && memcmp(units, needle, count * sizeof *units) == 0))) {
            *result = kiln_c99_number(0.0);
            break;
        }
        static const struct { const char *name; size_t len; int boolean; } option_names[] = {
            {"usage", 5, 0},      {"sensitivity", 11, 0},       {"numeric", 7, 1},
            {"caseFirst", 9, 0},  {"ignorePunctuation", 17, 1}, {"collation", 9, 0},
        };
        size_t which;
        if (locales.tag == KILN_VALUE_ARRAY) {
            const kiln_array *list = (const kiln_array *)locales.ptr;
            locales = list != NULL && list->count > 0 ? kiln_val_to_boundary(list->elements[0])
                                                        : kiln_c99_undefined();
        }
        if (locales.tag != KILN_VALUE_UNDEFINED) {
            uint8_t *tag = NULL;
            size_t tag_len = 0;
            kiln_value shown = kiln_c99_text(locales);
            if (shown.tag != KILN_VALUE_STRING ||
                kiln_c99_string_to_utf8_lossy(shown, &tag, &tag_len) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
                break;
            }
            if (tag_len < sizeof spec) {
                memcpy(spec, tag, tag_len);
                spec_len = tag_len;
            } else {
                spec_ok = 0;
            }
            free(tag);
        }
        for (which = 0; which < 6 && spec_ok; ++which) {
            kiln_value option = kiln_c99_undefined();
            spec[spec_len++] = 0x1F;
            if (options.tag != KILN_VALUE_OBJECT) continue;
            status = kiln_c99_get(host, options, option_names[which].name, option_names[which].len,
                                  &option, depth, thrown);
            if (status != KILN_C99_OK) break;
            if (option.tag == KILN_VALUE_UNDEFINED) continue;
            if (option_names[which].boolean) {
                const char *word = kiln_c99_truthy(option) ? "true" : "false";
                size_t len = strlen(word);
                if (spec_len + len >= sizeof spec) { spec_ok = 0; break; }
                memcpy(spec + spec_len, word, len);
                spec_len += len;
            } else {
                uint8_t *text = NULL;
                size_t text_len = 0;
                kiln_value shown = kiln_c99_text(option);
                if (shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_string_to_utf8_lossy(shown, &text, &text_len) != KILN_C99_OK) {
                    status = KILN_C99_OUT_OF_MEMORY;
                    break;
                }
                if (spec_len + text_len >= sizeof spec) {
                    spec_ok = 0;
                } else {
                    memcpy(spec + spec_len, text, text_len);
                    spec_len += text_len;
                }
                free(text);
            }
        }
        if (status != KILN_C99_OK) break;
        rc = spec_ok ? kiln_intl_collate(units, (uint64_t)count, needle, (uint64_t)needle_count,
                                         spec, (uint64_t)spec_len, &order)
                     : -1;
        if (rc == -1) {
            status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                          "Incorrect locale information provided");
            break;
        }
        if (rc != KILN_C99_OK) {
            free(units);
            free(needle);
            KILN_C99_REFUSE("the provider would not collate these strings");
        }
        *result = kiln_c99_number((double)order);
        break;
    }
    case KILN_C99_METHOD_STRING_CODE_POINT_AT: {
        /* `ToIntegerOrInfinity`, which maps NaN to zero. Without it every comparison below is
         * false for a NaN -- that is what NaN does -- and the position fell through to a cast
         * that has no value to produce, which on this target aborts the process rather than
         * answering anything. `"abc".codePointAt(NaN)` is `"abc".codePointAt(0)`. */
        double position = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        if (position != position) position = 0.0;
        position = trunc(position);
        if (position < 0.0 || position >= (double)count) {
            *result = kiln_c99_undefined();
            break;
        }
        {
            /* A surrogate pair reads as the whole code point, which is the entire difference
             * from `charCodeAt`. */
            uint16_t lead = units[(size_t)position];
            if (lead >= 0xD800 && lead <= 0xDBFF && (size_t)position + 1 < count &&
                units[(size_t)position + 1] >= 0xDC00 && units[(size_t)position + 1] <= 0xDFFF) {
                *result = kiln_c99_number(
                    (double)(0x10000u + (((uint32_t)lead - 0xD800u) << 10) +
                             ((uint32_t)units[(size_t)position + 1] - 0xDC00u)));
                break;
            }
            *result = kiln_c99_number((double)lead);
        }
        break;
    }
    case KILN_C99_METHOD_STRING_SUBSTR: {
        /* `ToIntegerOrInfinity` for the same reason `codePointAt` needs one: a NaN start
         * satisfies neither comparison below and reached a cast that cannot represent it. */
        double relative = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        size_t start;
        size_t taken;
        if (relative != relative) relative = 0.0;
        relative = trunc(relative);
        /* A negative start counts from the end, which is the one thing `substr` does that
         * `substring` does not. */
        if (relative < 0.0) {
            double from = (double)count + relative;
            start = from < 0.0 ? 0 : (size_t)from;
        } else {
            start = relative > (double)count ? count : (size_t)relative;
        }
        if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
            double wanted = kiln_c99_as_number(args[1]);
            if (isnan(wanted) || wanted < 0.0) {
                taken = 0;
            } else {
                taken = wanted > (double)(count - start) ? count - start : (size_t)wanted;
            }
        } else {
            taken = count - start;
        }
        *result = kiln_c99_from_units(units + start, taken);
        break;
    }
    case KILN_C99_METHOD_TRIM:
    case KILN_C99_METHOD_TRIM_START:
    case KILN_C99_METHOD_TRIM_END: {
        size_t start = 0;
        size_t end = count;
        if (code != KILN_C99_METHOD_TRIM_END) {
            while (start < end && kiln_c99_is_trimmed(units[start])) {
                start += 1;
            }
        }
        if (code != KILN_C99_METHOD_TRIM_START) {
            while (end > start && kiln_c99_is_trimmed(units[end - 1])) {
                end -= 1;
            }
        }
        *result = kiln_c99_from_units(units + start, end - start);
        break;
    }
    case KILN_C99_METHOD_SPLIT: {
        kiln_value built;
        status = kiln_c99_new_array(&built);
        if (status == KILN_C99_OK) {
            size_t from = 0;
            if (argc == 0 || args[0].tag == KILN_VALUE_UNDEFINED) {
                /* No separator at all yields the whole string as one part. */
                status = kiln_c99_array_push(built, kiln_c99_from_units(units, count));
            } else if (needle_count == 0) {
                /* An empty separator splits into code units, not into nothing. */
                size_t index;
                for (index = 0; index < count && status == KILN_C99_OK; ++index) {
                    status = kiln_c99_array_push(built, kiln_c99_from_units(units + index, 1));
                }
            } else {
                double at;
                while (status == KILN_C99_OK &&
                       (at = kiln_c99_units_find(units, count, needle, needle_count, from)) >=
                           0.0) {
                    status = kiln_c99_array_push(
                        built, kiln_c99_from_units(units + from, (size_t)at - from));
                    from = (size_t)at + needle_count;
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(built,
                                                 kiln_c99_from_units(units + from, count - from));
                }
            }
            kiln_c99_split_limit(built, args, argc);
            *result = built;
        }
        break;
    }
    case KILN_C99_METHOD_REPEAT: {
        double times = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        kiln_text repeated = {NULL, 0, 0};
        if (!(times >= 0.0) || !isfinite(times)) {
            status = kiln_c99_refuse("String.prototype.repeat was given a count that is "
                                     "negative or not finite");
        } else {
            size_t remaining = (size_t)times;
            while (remaining-- > 0 && status == KILN_C99_OK) {
                status = kiln_c99_text_append_string(&repeated, receiver);
            }
            *result = kiln_c99_text_value(&repeated);
        }
        break;
    }
    case KILN_C99_METHOD_PAD_START:
    case KILN_C99_METHOD_PAD_END: {
        double target = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        static const uint16_t space[] = {0x20};
        const uint16_t *filler = space;
        size_t filler_count = 1;
        if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
            kiln_value shown = kiln_c99_text(args[1]);
            free(needle);
            needle = NULL;
            needle_count = 0;
            if (shown.tag != KILN_VALUE_STRING ||
                kiln_c99_to_units(shown, &needle, &needle_count) != KILN_C99_OK) {
                status = kiln_c99_refuse("string padding was given a filler with no text form");
                break;
            }
            filler = needle;
            filler_count = needle_count;
        }
        if (!(target > (double)count) || filler_count == 0) {
            *result = kiln_c99_from_units(units, count);
        } else {
            size_t wanted = (size_t)target;
            uint16_t *padded = (uint16_t *)malloc(wanted * sizeof *padded);
            size_t fill = wanted - count;
            size_t index;
            if (padded == NULL) {
                status = KILN_C99_OUT_OF_MEMORY;
                break;
            }
            for (index = 0; index < fill; ++index) {
                padded[code == KILN_C99_METHOD_PAD_START ? index : count + index] =
                    filler[index % filler_count];
            }
            for (index = 0; index < count; ++index) {
                padded[code == KILN_C99_METHOD_PAD_START ? fill + index : index] = units[index];
            }
            *result = kiln_c99_from_units(padded, wanted);
            free(padded);
        }
        break;
    }
    case KILN_C99_METHOD_TO_UPPER_CASE:
    case KILN_C99_METHOD_TO_LOWER_CASE: {
        size_t index;
        uint16_t *cased = (uint16_t *)malloc((count == 0 ? 1 : count) * sizeof *cased);
        if (cased == NULL) {
            status = KILN_C99_OUT_OF_MEMORY;
            break;
        }
        for (index = 0; index < count; ++index) {
            uint16_t unit = units[index];
            if (unit >= 0x80) {
                /* Beyond ASCII the mapping is Unicode's, which is Rust's own and comes through
                 * the provider (ADR 0025): one call for the whole string, sized by asking, and
                 * `ß` really does become `SS`. This refused until 2026-09-19. */
                uint64_t need = 0;
                uint64_t cap = (uint64_t)count * 3 + 8;
                uint16_t *mapped = (uint16_t *)malloc((size_t)cap * sizeof *mapped);
                int rc;
                free(cased);
                if (mapped == NULL) {
                    status = KILN_C99_OUT_OF_MEMORY;
                    goto case_done;
                }
                rc = kiln_string_case(units, (uint64_t)count, code == KILN_C99_METHOD_TO_UPPER_CASE, mapped, cap, &need);
                if (rc == KILN_C99_INVALID_ARGUMENT && need > cap) {
                    uint16_t *bigger = (uint16_t *)realloc(mapped, (size_t)need * sizeof *mapped);
                    if (bigger == NULL) {
                        free(mapped);
                        status = KILN_C99_OUT_OF_MEMORY;
                        goto case_done;
                    }
                    mapped = bigger;
                    cap = need;
                    rc = kiln_string_case(units, (uint64_t)count, code == KILN_C99_METHOD_TO_UPPER_CASE, mapped, cap, &need);
                }
                if (rc != KILN_C99_OK) {
                    free(mapped);
                    free(units);
                    free(needle);
                    KILN_C99_REFUSE("the provider would not case-map this string");
                }
                *result = kiln_c99_from_units(mapped, (size_t)need);
                free(mapped);
                goto case_done;
            }
            cased[index] = code == KILN_C99_METHOD_TO_UPPER_CASE
                               ? (uint16_t)toupper((int)unit)
                               : (uint16_t)tolower((int)unit);
        }
        *result = kiln_c99_from_units(cased, count);
        free(cased);
        break;
    case_done:
        break;
    }
    case KILN_C99_METHOD_REPLACE:
    case KILN_C99_METHOD_REPLACE_ALL: {
        kiln_value with = argc > 1 ? kiln_c99_text(args[1]) : kiln_c99_undefined();
        uint16_t *replacement = NULL;
        size_t replacement_count = 0;
        kiln_text built = {NULL, 0, 0};
        size_t from = 0;
        double at;
        if (with.tag == KILN_VALUE_STRING &&
            kiln_c99_to_units(with, &replacement, &replacement_count) != KILN_C99_OK) {
            status = KILN_C99_OUT_OF_MEMORY;
            break;
        }
        while ((at = kiln_c99_units_find(units, count, needle, needle_count, from)) >= 0.0) {
            kiln_value head = kiln_c99_from_units(units + from, (size_t)at - from);
            /* The occurrence is the only group, which is what lets the template expander below be
             * the same one the regexp path uses -- and a replacement *is* a template even when
             * the pattern was a plain string: `"a".replace("a", "$&$&")` is `"aa"`. */
            int64_t ranges[2];
            ranges[0] = (int64_t)at;
            ranges[1] = (int64_t)at + (int64_t)needle_count;
            if (kiln_c99_text_append_string(&built, head) != KILN_C99_OK ||
                kiln_c99_expand_replacement(&built, replacement, replacement_count, units, count,
                                            ranges, 1, NULL) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
                break;
            }
            from = (size_t)at + needle_count;
            /* `replace` takes the first occurrence; `replaceAll` takes every one. An empty needle
             * matches between every pair of characters, so the cursor is stepped by hand -- and
             * the character it steps over has to be carried across, or it would be dropped. */
            if (code == KILN_C99_METHOD_REPLACE) {
                break;
            }
            if (needle_count == 0) {
                kiln_value carried;
                if (from >= count) {
                    break;
                }
                carried = kiln_c99_from_units(units + from, 1);
                if (kiln_c99_text_append_string(&built, carried) != KILN_C99_OK) {
                    status = KILN_C99_OUT_OF_MEMORY;
                    break;
                }
                from += 1;
            }
        }
        if (status == KILN_C99_OK) {
            kiln_value rest = kiln_c99_from_units(units + from, count - from);
            if (kiln_c99_text_append_string(&built, rest) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
            } else {
                *result = kiln_c99_text_value(&built);
            }
        }
        free(replacement);
        break;
    }
    case KILN_C99_METHOD_STRING_CONCAT: {
        kiln_text built = {NULL, 0, 0};
        size_t index;
        if (kiln_c99_text_append_string(&built, receiver) != KILN_C99_OK) {
            status = KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < argc && status == KILN_C99_OK; ++index) {
            kiln_value shown = kiln_c99_text(args[index]);
            if (shown.tag != KILN_VALUE_STRING ||
                kiln_c99_text_append_string(&built, shown) != KILN_C99_OK) {
                status = kiln_c99_refuse("String.prototype.concat was given a value with no "
                                         "text form");
            }
        }
        if (status == KILN_C99_OK) {
            *result = kiln_c99_text_value(&built);
        }
        break;
    }
    default:
        status = kiln_c99_refuse("a String.prototype method this backend has not implemented");
        break;
    }
    free(units);
    free(needle);
    return status;
}

/* One `exec`-shaped array: the match, then each group, with `index`, `input` and `groups`. */
static int kiln_c99_match_result(kiln_host *host, kiln_value *out, const uint16_t *units,
                                 size_t count, const int64_t *ranges, size_t groups, int depth,
                                 kiln_value *thrown, kiln_regexp *regexp) {
    kiln_value built;
    size_t group;
    int status = kiln_c99_new_array(&built);
    for (group = 0; group < groups && status == KILN_C99_OK; ++group) {
        int64_t from = ranges[group * 2];
        int64_t to = ranges[group * 2 + 1];
        status = kiln_c99_array_push(built, from < 0 ? kiln_c99_undefined()
                                                     : kiln_c99_from_units(units + (size_t)from,
                                                                           (size_t)(to - from)));
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, built, "index", 5, kiln_c99_number((double)ranges[0]), depth,
                              thrown);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, built, "input", 5, kiln_c99_from_units(units, count), depth,
                              thrown);
    }
    if (status == KILN_C99_OK && regexp != NULL) {
        status = kiln_c99_match_groups(regexp, kiln_c99_array_of(built));
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, built, "groups", 6, kiln_c99_array_of(built)->match_groups, depth, thrown);
    }
    *out = built;
    return status;
}

/* The string methods that take a pattern.
 *
 * Intercepted before the string block because they need the interpreter: a replacement can be a
 * *function*, and the string block is a pure transformation with nothing to call. `replace` with
 * a plain string and a plain needle stays down there, where it always was.
 *
 * An empty match advances the cursor by one, or a pattern that can match nothing would never
 * finish. */
static int kiln_c99_string_pattern(kiln_host *host, kiln_value *result, uint64_t code,
                                   kiln_value receiver, const kiln_value *args, size_t argc,
                                   int depth, kiln_value *thrown);
static int kiln_c99_string_pattern_split(kiln_host *host, kiln_value *result, kiln_value receiver,
                                         const kiln_value *args, size_t argc, int depth,
                                         kiln_value *thrown) {
    int status = kiln_c99_string_pattern(host, result, KILN_C99_METHOD_SPLIT, receiver, args,
                                         argc > 1 ? 1 : argc, depth, thrown);
    if (status == KILN_C99_OK) kiln_c99_split_limit(*result, args, argc);
    return status;
}

static int kiln_c99_string_pattern(kiln_host *host, kiln_value *result, uint64_t code,
                                   kiln_value receiver, const kiln_value *args, size_t argc,
                                   int depth, kiln_value *thrown) {
    kiln_value pattern = argc > 0 ? args[0] : kiln_c99_undefined();
    kiln_value with = argc > 1 ? args[1] : kiln_c99_undefined();
    kiln_regexp *regexp = pattern.tag == KILN_VALUE_REGEXP ? kiln_c99_regexp_of(pattern) : NULL;
    uint16_t *units = NULL;
    uint16_t *replacement = NULL;
    size_t count = 0;
    size_t replacement_count = 0;
    size_t from = 0;
    size_t at = 0;
    int64_t stack_ranges[64];
    int64_t *ranges = stack_ranges;
    size_t ranges_capacity = sizeof(stack_ranges) / sizeof(stack_ranges[0]);
    kiln_text built = {NULL, 0, 0};
    kiln_value collected = kiln_c99_undefined();
    int status = kiln_c99_to_units(receiver, &units, &count);
    int every;
    if (status != KILN_C99_OK) {
        return status;
    }
    if (regexp == NULL) {
        /* `"x".match("a")` compiles its argument, which is what makes a string pattern work
         * everywhere a regexp does. */
        kiln_value source = kiln_c99_text(pattern);
        free(units);
        if (source.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        {
            kiln_value made;
            kiln_value forwarded[2];
            status = kiln_c99_new_regexp(&made, (const kiln_string *)source.ptr,
                                         &kiln_c99_empty_string, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            forwarded[0] = made;
            forwarded[1] = with;
            return kiln_c99_string_pattern(host, result, code, receiver, forwarded,
                                           argc > 1 ? 2 : 1, depth, thrown);
        }
    }
    every = regexp->global || code == KILN_C99_METHOD_REPLACE_ALL ||
            code == KILN_C99_METHOD_STRING_MATCH_ALL || code == KILN_C99_METHOD_SPLIT;
    if (code == KILN_C99_METHOD_STRING_MATCH || code == KILN_C99_METHOD_STRING_MATCH_ALL ||
        code == KILN_C99_METHOD_SPLIT) {
        status = kiln_c99_new_array(&collected);
        if (status != KILN_C99_OK) {
            free(units);
            return status;
        }
    }
    if (code == KILN_C99_METHOD_SPLIT && count == 0) {
        /* An empty subject is the one case with no trailing piece: a separator that matches it
         * splits it into nothing, and one that does not leaves it whole. */
        size_t groups = 0;
        int matched = 0;
        status = kiln_c99_regexp_find(regexp, units, count, 0, ranges,
                                      ranges_capacity, &groups, &matched);
        if (status == KILN_C99_OK && !matched) {
            status = kiln_c99_array_push(collected, kiln_c99_from_units(units, 0));
        }
        if (ranges != stack_ranges) {
            free(ranges);
        }
        free(units);
        *result = collected;
        return status;
    }
    if ((code == KILN_C99_METHOD_REPLACE || code == KILN_C99_METHOD_REPLACE_ALL) &&
        !kiln_c99_is_callable(with)) {
        kiln_value text = kiln_c99_text(with);
        if (text.tag != KILN_VALUE_STRING ||
            kiln_c99_to_units(text, &replacement, &replacement_count) != KILN_C99_OK) {
            free(units);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    for (;;) {
        size_t groups = 0;
        int matched = 0;
        status = kiln_c99_regexp_find(regexp, units, count, at, ranges,
                                      ranges_capacity, &groups, &matched);
        if (status != KILN_C99_OK || !matched) {
            break;
        }
        if (groups > ranges_capacity / 2) {
            int64_t *new_ranges = (int64_t *)malloc(groups * 2 * sizeof(int64_t));
            if (new_ranges == NULL) {
                status = KILN_C99_OUT_OF_MEMORY;
                break;
            }
            if (ranges != stack_ranges) {
                free(ranges);
            }
            ranges = new_ranges;
            ranges_capacity = groups * 2;
            status = kiln_c99_regexp_find(regexp, units, count, at, ranges,
                                          ranges_capacity, &groups, &matched);
            if (status != KILN_C99_OK || !matched) {
                break;
            }
        }
        if (code == KILN_C99_METHOD_STRING_SEARCH) {
            *result = kiln_c99_number((double)ranges[0]);
            if (ranges != stack_ranges) {
                free(ranges);
            }
            free(units);
            free(replacement);
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_STRING_MATCH && !regexp->global) {
            status = kiln_c99_match_result(host, result, units, count, ranges, groups, depth,
                                           thrown, regexp);
            if (ranges != stack_ranges) {
                free(ranges);
            }
            free(units);
            free(replacement);
            return status;
        }
        if (code == KILN_C99_METHOD_STRING_MATCH) {
            /* A global `match` collects the matched text and nothing else, which is the whole
             * difference from `matchAll`. */
            status = kiln_c99_array_push(collected,
                                         kiln_c99_from_units(units + (size_t)ranges[0],
                                                             (size_t)(ranges[1] - ranges[0])));
        } else if (code == KILN_C99_METHOD_STRING_MATCH_ALL) {
            kiln_value entry;
            status = kiln_c99_match_result(host, &entry, units, count, ranges, groups, depth,
                                           thrown, regexp);
            if (status == KILN_C99_OK) {
                status = kiln_c99_array_push(collected, entry);
            }
        } else if (code == KILN_C99_METHOD_SPLIT) {
            size_t group;
            /* An empty match at the position the last piece started is not a separator, and
             * neither is a match *at* the end: the walk stops before the last position and the
             * piece after the final separator is pushed below. Without both, splitting "abc" on
             * a pattern that can match nothing grows an empty part at each end. */
            if (((size_t)ranges[0] == (size_t)ranges[1] && (size_t)ranges[0] == from) ||
                (size_t)ranges[0] == count) {
                at = ranges[1] == ranges[0] ? (size_t)ranges[1] + 1 : (size_t)ranges[1];
                if (at > count) {
                    break;
                }
                continue;
            }
            status = kiln_c99_array_push(collected,
                                         kiln_c99_from_units(units + from,
                                                             (size_t)ranges[0] - from));
            /* A captured group becomes an element of its own, which is what makes
             * `"a1b".split(/(\d)/)` three parts rather than two. */
            for (group = 1; group < groups && status == KILN_C99_OK; ++group) {
                status = kiln_c99_array_push(
                    collected, ranges[group * 2] < 0
                                   ? kiln_c99_undefined()
                                   : kiln_c99_from_units(units + (size_t)ranges[group * 2],
                                                         (size_t)(ranges[group * 2 + 1] -
                                                                  ranges[group * 2])));
            }
            from = (size_t)ranges[1];
        } else {
            kiln_value head = kiln_c99_from_units(units + from, (size_t)ranges[0] - from);
            if (kiln_c99_text_append_string(&built, head) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
            } else if (kiln_c99_is_callable(with)) {
                /* The callback is handed the match, then each group, then the index and the whole
                 * subject -- which is why it cannot live in the string block. */
                kiln_value produced;
                kiln_value handed[64];
                size_t group;
                size_t handed_count = groups + 2 > 64 ? 64 : groups + 2;
                for (group = 0; group + 2 < handed_count; ++group) {
                    handed[group] = ranges[group * 2] < 0
                                        ? kiln_c99_undefined()
                                        : kiln_c99_from_units(units + (size_t)ranges[group * 2],
                                                              (size_t)(ranges[group * 2 + 1] -
                                                                       ranges[group * 2]));
                }
                handed[handed_count - 2] = kiln_c99_number((double)ranges[0]);
                handed[handed_count - 1] = kiln_c99_from_units(units, count);
                status = kiln_c99_call(host, &produced, with, kiln_c99_undefined(), handed,
                                       handed_count, depth, thrown);
                if (status == KILN_C99_OK) {
                    kiln_value text = kiln_c99_text(produced);
                    status = text.tag == KILN_VALUE_STRING &&
                                     kiln_c99_text_append_string(&built, text) ==
                                         KILN_C99_OK
                                 ? KILN_C99_OK
                                 : KILN_C99_OUT_OF_MEMORY;
                }
            } else {
                status = kiln_c99_expand_replacement(&built, replacement, replacement_count, units,
                                                     count, ranges, groups, regexp);
            }
            from = (size_t)ranges[1];
        }
        if (status != KILN_C99_OK) {
            break;
        }
        at = ranges[1] == ranges[0] ? (size_t)ranges[1] + 1 : (size_t)ranges[1];
        if (!every) {
            break;
        }
    }
    if (regexp->global || regexp->sticky) {
        /* Every one of these leaves the cursor where it found it, which `exec` relies on and
         * `match` deliberately resets. */
        regexp->last_index = 0.0;
    }
    if (status == KILN_C99_OK) {
        switch (code) {
        case KILN_C99_METHOD_STRING_SEARCH:
            *result = kiln_c99_number(-1.0);
            break;
        case KILN_C99_METHOD_STRING_MATCH:
            /* No match at all is null rather than an empty array: the two are different answers
             * and a program tests for the first. */
            *result = kiln_c99_array_of(collected)->count == 0 ? kiln_c99_null() : collected;
            break;
        case KILN_C99_METHOD_STRING_MATCH_ALL:
            status = kiln_c99_new_iterator(result, collected);
            break;
        case KILN_C99_METHOD_SPLIT:
            status = kiln_c99_array_push(collected,
                                         kiln_c99_from_units(units + from, count - from));
            *result = collected;
            break;
        default: {
            kiln_value rest = kiln_c99_from_units(units + from, count - from);
            if (kiln_c99_text_append_string(&built, rest) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
            } else {
                *result = kiln_c99_text_value(&built);
            }
            break;
        }
        }
    }
    if (ranges != stack_ranges) {
        free(ranges);
    }
    free(units);
    free(replacement);
    return status;
}
