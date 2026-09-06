
/* Every position-taking string method counts UTF-16 code units (ADR 0022), so each of them
 * converts once, works on units, and re-encodes.  One conversion pair rather than a dozen
 * ad-hoc walks is what keeps them agreeing with each other and with the interpreter.
 *
 * The caller owns the returned array and frees it. */
static int kiln_c99_to_units(kiln_value value, uint16_t **out, size_t *count) {
    kiln_c99_utf16_iter iterator;
    uint16_t *units;
    size_t capacity;
    size_t length;
    uint16_t unit;
    /* ASCII text has one code unit per byte and no surrogate pairing to do, so the whole
     * decode state machine (kiln_c99_utf8_next by way of kiln_c99_utf16_next) is replaced with
     * a straight widening copy -- still O(len), but branch-free per byte instead of walking a
     * UTF-8 length table and a surrogate split for every unit. */
    if (kiln_c99_string_is_ascii(value)) {
        size_t index;
        if (value.len == 0) {
            *out = NULL;
            *count = 0;
            return KILN_C99_OK;
        }
        units = (uint16_t *)malloc(value.len * sizeof *units);
        if (units == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < value.len; ++index) {
            units[index] = (uint16_t)(unsigned char)value.ptr[index];
        }
        *out = units;
        *count = value.len;
        return KILN_C99_OK;
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
    kiln_text text = {NULL, 0, 0};
    size_t index;
    if (kiln_c99_units_ascii(units, count)) {
        char *copy;
        kiln_gc_heap *heap;
        kiln_gc_block *block;
        if (count == 0) {
            return kiln_c99_value(KILN_VALUE_STRING, 0, NULL, 0);
        }
        copy = kiln_c99_gc_alloc_text(count);
        if (copy == NULL) {
            return kiln_c99_undefined();
        }
        for (index = 0; index < count; ++index) {
            copy[index] = (char)units[index];
        }
        heap = kiln_c99_gc_heap();
        block = heap != NULL ? kiln_c99_gc_lookup(heap, copy) : NULL;
        if (block != NULL) {
            block->kind = (block->kind & KILN_GC_KIND_MASK) | KILN_GC_FLAG_ASCII_KNOWN |
                          KILN_GC_FLAG_ASCII;
        }
        return kiln_c99_value(KILN_VALUE_STRING, 0, copy, count);
    }
    for (index = 0; index < count; ++index) {
        uint32_t point = units[index];
        char bytes[4];
        size_t width;
        if (point >= 0xD800 && point <= 0xDBFF && index + 1 < count && units[index + 1] >= 0xDC00 &&
            units[index + 1] <= 0xDFFF) {
            point = 0x10000 + ((point - 0xD800) << 10) + (units[index + 1] - 0xDC00);
            index += 1;
        } else if (point >= 0xD800 && point <= 0xDFFF) {
            point = 0xFFFD;
        }
        if (point < 0x80) {
            bytes[0] = (char)point;
            width = 1;
        } else if (point < 0x800) {
            bytes[0] = (char)(0xC0 | (point >> 6));
            bytes[1] = (char)(0x80 | (point & 0x3F));
            width = 2;
        } else if (point < 0x10000) {
            bytes[0] = (char)(0xE0 | (point >> 12));
            bytes[1] = (char)(0x80 | ((point >> 6) & 0x3F));
            bytes[2] = (char)(0x80 | (point & 0x3F));
            width = 3;
        } else {
            bytes[0] = (char)(0xF0 | (point >> 18));
            bytes[1] = (char)(0x80 | ((point >> 12) & 0x3F));
            bytes[2] = (char)(0x80 | ((point >> 6) & 0x3F));
            bytes[3] = (char)(0x80 | (point & 0x3F));
            width = 4;
        }
        if (kiln_c99_text_append(&text, bytes, width) != KILN_C99_OK) {
            free(text.bytes);
            return kiln_c99_undefined();
        }
    }
    return kiln_c99_text_value(&text);
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

/* The string methods.  Split out from the array ones because they share nothing but the dispatch
 * -- every one of these works on code units and every one of those works on elements. */
static int kiln_c99_invoke_string_method(kiln_value *result, uint64_t code, kiln_value receiver,
                                         const kiln_value *args, size_t argc) {
    if (kiln_c99_string_is_ascii(receiver)) {
        if (code == KILN_C99_METHOD_STRING_SLICE || code == KILN_C99_METHOD_SUBSTRING) {
            int relative = (code == KILN_C99_METHOD_STRING_SLICE);
            size_t start = kiln_c99_unit_position(argc > 0 ? args[0] : kiln_c99_undefined(), receiver.len, 0, relative);
            size_t end = kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), receiver.len, receiver.len, relative);
            if (!relative && start > end) {
                size_t swap = start;
                start = end;
                end = swap;
            }
            if (start >= end) {
                *result = kiln_c99_static_text("", 0);
            } else {
                *result = kiln_c99_copy_text(receiver.ptr + start, end - start);
            }
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_STRING_INDEX_OF || code == KILN_C99_METHOD_STRING_INCLUDES) {
            kiln_value needle_arg = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value shown = kiln_c99_text(needle_arg);
            if (shown.tag == KILN_VALUE_STRING && kiln_c99_string_is_ascii(shown)) {
                size_t from = kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), receiver.len, 0, 0);
                if (shown.len == 0) {
                    double at = from > receiver.len ? (double)receiver.len : (double)from;
                    if (code == KILN_C99_METHOD_STRING_INDEX_OF) *result = kiln_c99_number(at);
                    else *result = kiln_c99_boolean(1);
                    return KILN_C99_OK;
                }
                if (from >= receiver.len) {
                    if (code == KILN_C99_METHOD_STRING_INDEX_OF) *result = kiln_c99_number(-1.0);
                    else *result = kiln_c99_boolean(0);
                    return KILN_C99_OK;
                }
                const char *found = kiln_c99_memmem(receiver.ptr + from, receiver.len - from, shown.ptr, shown.len);
                if (code == KILN_C99_METHOD_STRING_INDEX_OF) {
                    *result = kiln_c99_number(found != NULL ? (double)(found - receiver.ptr) : -1.0);
                } else {
                    *result = kiln_c99_boolean(found != NULL);
                }
                return KILN_C99_OK;
            }
        }
        if (code == KILN_C99_METHOD_SPLIT) {
            kiln_value sep = argc > 0 ? args[0] : kiln_c99_undefined();
            if (sep.tag == KILN_VALUE_UNDEFINED) {
                kiln_value built;
                int st = kiln_c99_new_array_with_capacity(&built, 1, 0);
                if (st != KILN_C99_OK) return st;
                kiln_array *arr = kiln_c99_array_of(built);
                arr->elements[arr->count++] = kiln_val_from_boundary(receiver);
                *result = built;
                return KILN_C99_OK;
            }
            kiln_value shown = kiln_c99_text(sep);
            if (shown.tag == KILN_VALUE_STRING && kiln_c99_string_is_ascii(shown)) {
                kiln_value built;
                if (shown.len == 0) {
                    int st = kiln_c99_new_array_with_capacity(&built, receiver.len, 0);
                    if (st != KILN_C99_OK) return st;
                    kiln_array *arr = kiln_c99_array_of(built);
                    for (size_t i = 0; i < receiver.len; ++i) {
                        arr->elements[arr->count++] = kiln_c99_val_single_char((unsigned char)receiver.ptr[i]);
                    }
                    *result = built;
                    return KILN_C99_OK;
                }
                int st = kiln_c99_new_array_with_capacity(&built, 8, 0);
                if (st != KILN_C99_OK) return st;
                const char *p = receiver.ptr;
                const char *end = receiver.ptr + receiver.len;
                while (p <= end) {
                    const char *next = NULL;
                    if (shown.len == 1) {
                        next = (const char *)memchr(p, shown.ptr[0], (size_t)(end - p));
                    } else {
                        next = kiln_c99_memmem(p, (size_t)(end - p), shown.ptr, shown.len);
                    }
                    if (next != NULL) {
                        kiln_value piece = kiln_c99_copy_text(p, (size_t)(next - p));
                        st = kiln_c99_array_push(built, piece);
                        if (st != KILN_C99_OK) return st;
                        p = next + shown.len;
                    } else {
                        kiln_value piece = kiln_c99_copy_text(p, (size_t)(end - p));
                        st = kiln_c99_array_push(built, piece);
                        if (st != KILN_C99_OK) return st;
                        break;
                    }
                }
                *result = built;
                return KILN_C99_OK;
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
    case KILN_C99_METHOD_CHAR_AT:
    case KILN_C99_METHOD_CHAR_CODE_AT:
    case KILN_C99_METHOD_STRING_AT: {
        double index = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        if (isnan(index)) {
            index = 0.0;
        }
        /* `at` accepts a negative index, where `charAt` clamps and `charCodeAt` answers NaN --
         * three spellings of "the character there" that differ only at the edges. */
        if (code == KILN_C99_METHOD_STRING_AT && index < 0.0) {
            index += (double)count;
        }
        if (index < 0.0 || index >= (double)count) {
            *result = code == KILN_C99_METHOD_CHAR_AT      ? kiln_c99_from_units(NULL, 0)
                      : code == KILN_C99_METHOD_STRING_AT  ? kiln_c99_undefined()
                                                           : kiln_c99_number(0.0 / 0.0);
        } else if (code == KILN_C99_METHOD_CHAR_CODE_AT) {
            *result = kiln_c99_number((double)units[(size_t)index]);
        } else {
            *result = kiln_c99_from_units(units + (size_t)index, 1);
        }
        break;
    }
    case KILN_C99_METHOD_STRING_TO_STRING:
    case KILN_C99_METHOD_STRING_NORMALIZE:
    case KILN_C99_METHOD_STRING_VALUE_OF:
        /* `normalize` has no normalisation tables behind it on either side, so it is the
         * identity: honest for text that is already normalised, which is what a bundler emits,
         * and wrong for text that is not -- which is why it is said here rather than left to be
         * discovered. */
        *result = receiver;
        break;
    case KILN_C99_METHOD_STRING_LOCALE_COMPARE: {
        /* No collation tables either, so this compares code units -- which is a valid locale
         * ordering and is the one both engines use. */
        kiln_value other = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_from_units(NULL, 0);
        if (other.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        *result = kiln_c99_number((double)kiln_c99_string_compare(receiver, other));
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
                status = kiln_c99_text_append(&repeated, receiver.ptr, receiver.len);
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
            /* Case mapping outside ASCII needs Unicode tables this unit does not carry, and
             * guessing would disagree with the interpreter silently. */
            if (unit >= 0x80) {
                free(cased);
                free(units);
                free(needle);
                KILN_C99_REFUSE("case mapping outside ASCII needs Unicode tables this unit does "
                                "not carry");
            }
            cased[index] = code == KILN_C99_METHOD_TO_UPPER_CASE
                               ? (uint16_t)toupper((int)unit)
                               : (uint16_t)tolower((int)unit);
        }
        *result = kiln_c99_from_units(cased, count);
        free(cased);
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
            if (kiln_c99_text_append(&built, head.ptr, head.len) != KILN_C99_OK ||
                kiln_c99_expand_replacement(&built, replacement, replacement_count, units, count,
                                            ranges, 1) != KILN_C99_OK) {
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
                if (kiln_c99_text_append(&built, carried.ptr, carried.len) != KILN_C99_OK) {
                    status = KILN_C99_OUT_OF_MEMORY;
                    break;
                }
                from += 1;
            }
        }
        if (status == KILN_C99_OK) {
            kiln_value rest = kiln_c99_from_units(units + from, count - from);
            if (kiln_c99_text_append(&built, rest.ptr, rest.len) != KILN_C99_OK) {
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
        if (kiln_c99_text_append(&built, receiver.ptr, receiver.len) != KILN_C99_OK) {
            status = KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < argc && status == KILN_C99_OK; ++index) {
            kiln_value shown = kiln_c99_text(args[index]);
            if (shown.tag != KILN_VALUE_STRING ||
                kiln_c99_text_append(&built, shown.ptr, shown.len) != KILN_C99_OK) {
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
                                 kiln_value *thrown) {
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
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, built, "groups", 6, kiln_c99_undefined(), depth, thrown);
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
    int64_t ranges[64];
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
        static const char none[] = "";
        free(units);
        if (source.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        {
            kiln_value made;
            kiln_value forwarded[2];
            status = kiln_c99_new_regexp(&made, source.ptr, source.len, none, 0, thrown);
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
                                      sizeof ranges / sizeof *ranges, &groups, &matched);
        if (status == KILN_C99_OK && !matched) {
            status = kiln_c99_array_push(collected, kiln_c99_from_units(units, 0));
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
                                      sizeof ranges / sizeof *ranges, &groups, &matched);
        if (status != KILN_C99_OK || !matched) {
            break;
        }
        if (code == KILN_C99_METHOD_STRING_SEARCH) {
            *result = kiln_c99_number((double)ranges[0]);
            free(units);
            free(replacement);
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_STRING_MATCH && !regexp->global) {
            status = kiln_c99_match_result(host, result, units, count, ranges, groups, depth,
                                           thrown);
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
                                           thrown);
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
            if (kiln_c99_text_append(&built, head.ptr, head.len) != KILN_C99_OK) {
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
                                     kiln_c99_text_append(&built, text.ptr, text.len) ==
                                         KILN_C99_OK
                                 ? KILN_C99_OK
                                 : KILN_C99_OUT_OF_MEMORY;
                }
            } else {
                status = kiln_c99_expand_replacement(&built, replacement, replacement_count, units,
                                                     count, ranges, groups);
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
            if (kiln_c99_text_append(&built, rest.ptr, rest.len) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
            } else {
                *result = kiln_c99_text_value(&built);
            }
            break;
        }
        }
    }
    free(units);
    free(replacement);
    return status;
}
