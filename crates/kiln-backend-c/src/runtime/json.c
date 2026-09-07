/* Whether any byte in this span needs `\"`, `\\` or `\u00XX` escaping. A single predicate pass
 * over plain comparisons, with no per-byte call into the text builder, is what lets the common
 * case below become one bulk append instead of one append per byte -- and it is exactly as valid
 * for a multi-byte UTF-8 string as for ASCII, since every continuation and lead byte is >= 0x80
 * and so never equals a quote, a backslash or a C0 control. */
static int kiln_c99_json_needs_escape(const char *bytes, size_t len) {
    size_t index;
    for (index = 0; index < len; ++index) {
        unsigned char byte = (unsigned char)bytes[index];
        if (byte == '"' || byte == '\\' || byte < 0x20) return 1;
    }
    return 0;
}

static int kiln_c99_json_quote(kiln_text *text, const char *bytes, size_t len) {
    static const char digits[] = "0123456789abcdef";
    size_t index;
    if (kiln_c99_text_append(text, "\"", 1) != KILN_C99_OK) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    if (!kiln_c99_json_needs_escape(bytes, len)) {
        if (kiln_c99_text_append(text, bytes, len) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append(text, "\"", 1);
    }
    for (index = 0; index < len; ++index) {
        char byte = bytes[index];
        const char *escape = NULL;
        char pair[2];
        char hex[6];
        switch (byte) {
        case '"': escape = "\\\""; break;
        case '\\': escape = "\\\\"; break;
        case '\n': escape = "\\n"; break;
        case '\r': escape = "\\r"; break;
        case '\t': escape = "\\t"; break;
        case '\b': escape = "\\b"; break;
        case '\f': escape = "\\f"; break;
        default: break;
        }
        if (escape != NULL) {
            if (kiln_c99_text_append_c(text, escape) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            continue;
        }
        /* Only the C0 controls are escaped as \u; everything else is emitted as its own UTF-8,
         * which is what JSON permits and what node produces. */
        if ((unsigned char)byte < 0x20) {
            hex[0] = '\\';
            hex[1] = 'u';
            hex[2] = '0';
            hex[3] = '0';
            hex[4] = digits[((unsigned char)byte >> 4) & 0xF];
            hex[5] = digits[(unsigned char)byte & 0xF];
            if (kiln_c99_text_append(text, hex, 6) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            continue;
        }
        pair[0] = byte;
        if (kiln_c99_text_append(text, pair, 1) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    return kiln_c99_text_append(text, "\"", 1);
}

/* True when no object in `value`'s own store or its prototype chain owns a property named
 * "toJSON" -- the common case for a plain object literal, whose chain is just Object.prototype.
 * `JSON.stringify`'s real question is `IsCallable(GetV(value, "toJSON"))`, and `GetV` walks every
 * level looking only for a property with that name (accessor or not, enumerable or not); this
 * answers the same walk without the call, the boundary round trips, or the prototype-chain code
 * shared with every other named read, because the answer here only ever needs "is the name
 * there", never the value.
 *
 * A shape's keys are always the interned canonical pointer for their spelling (every writer that
 * builds one -- `kiln_c99_define_named_fast`, `kiln_c99_json_define` below, the generic
 * `kiln_c99_put` -- interns before it transitions), so a shaped level compares by pointer against
 * one interned lookup of "toJSON" and never touches the bytes. A dictionary-mode level has no such
 * guarantee (a computed key can be heap bytes nobody interned) and is scanned by content instead,
 * which is why `kiln_c99_same_key` -- not a pointer compare -- is what a dictionary level uses.
 *
 * Anything other than a plain object at the end of the chain -- a proxy, a function, a class
 * standing in as someone's prototype -- answers conservatively (0, "cannot promise it lacks one")
 * rather than guess, and the caller falls back to the real `kiln_c99_get` walk exactly as before. */
static int kiln_c99_json_lacks_tojson(kiln_value value) {
    const char *tojson_ikey = kiln_c99_interned("toJSON", 6);
    int hops = 0;
    for (;;) {
        kiln_object *object;
        size_t index;
        if (value.tag == KILN_VALUE_NULL || value.tag == KILN_VALUE_UNDEFINED) {
            return 1;
        }
        if (value.tag != KILN_VALUE_OBJECT) {
            return 0;
        }
        if (++hops > KILN_C99_MAX_PROTOTYPE_DEPTH) {
            return 0;
        }
        object = kiln_c99_object_of(value);
        if (object->shape != NULL) {
            if (tojson_ikey != NULL) {
                for (index = 0; index < object->shape->property_count; ++index) {
                    if (object->shape->keys[index] == tojson_ikey) return 0;
                }
            }
        } else {
            for (index = 0; index < object->count; ++index) {
                if (kiln_c99_same_key(&object->properties[index], "toJSON", 6)) return 0;
            }
        }
        value = object->prototype;
    }
}

/* Appends the decimal spelling of `magnitude` (< 2^64) with no leading zero, for the integer fast
 * path below: digits come out least-significant first into a stack buffer sized for the largest
 * 64-bit value, then go to the builder in the order they are actually spelled, in the one append
 * every other literal in this file already uses. */
static int kiln_c99_json_append_uint(kiln_text *text, uint64_t magnitude) {
    char digits[20];
    size_t count = 0;
    if (magnitude == 0) {
        return kiln_c99_text_append(text, "0", 1);
    }
    while (magnitude != 0) {
        digits[count++] = (char)('0' + (magnitude % 10));
        magnitude /= 10;
    }
    {
        size_t low = 0;
        size_t high = count - 1;
        while (low < high) {
            char tmp = digits[low];
            digits[low] = digits[high];
            digits[high] = tmp;
            ++low;
            --high;
        }
    }
    return kiln_c99_text_append(text, digits, count);
}

/* `serialised` is 0 when the value has no JSON spelling at all -- undefined, a function -- which
 * drops it from an object and turns it into null in an array. */
static int kiln_c99_json_value(kiln_host *host, kiln_text *text, kiln_value value,
                               const char *indent, size_t indent_len, const char *current,
                               size_t current_len, int *serialised, int depth,
                               const kiln_json_frame *parents, kiln_value *thrown) {
    kiln_json_frame frame;
    const kiln_json_frame *ancestor;
    kiln_text nested = {NULL, 0, 0};
    kiln_value replaced;
    size_t index;
    int status;
    *serialised = 1;

    /* The `toJSON` step, before anything else is decided about the value.
     *
     * Only an object is asked. An array's named properties are not stored here at all, so there
     * is nowhere for one to have been put -- and asking would reach the array method table,
     * which reports an unknown name as an error rather than as absence. A Date is asked because
     * carrying one is the whole reason the step exists. */
    if (value.tag == KILN_VALUE_DATE || value.tag == KILN_VALUE_TEMPORAL ||
        (value.tag == KILN_VALUE_PROXY &&
         kiln_c99_proxy_inner(value).tag == KILN_VALUE_OBJECT) ||
        (value.tag == KILN_VALUE_OBJECT && !kiln_c99_json_lacks_tojson(value))) {
        kiln_value method;
        status = kiln_c99_get(host, value, "toJSON", 6, &method, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (kiln_c99_is_callable(method)) {
            status = kiln_c99_call(host, &replaced, method, value, NULL, 0, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            value = replaced;
        }
    }

    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
    case KILN_VALUE_HOLE:
    case KILN_VALUE_FUNCTION:
    case KILN_VALUE_BUILTIN:
    case KILN_VALUE_METHOD:
        *serialised = 0;
        return KILN_C99_OK;
    case KILN_VALUE_NULL:
        return kiln_c99_text_append_c(text, "null");
    case KILN_VALUE_BOOLEAN:
        return kiln_c99_text_append_c(text, value.bits ? "true" : "false");
    case KILN_VALUE_NUMBER: {
        /* NaN and both infinities have no JSON spelling, so they serialise as null. */
        double number = kiln_c99_as_number(value);
        kiln_value shown;
        if (!isfinite(number)) {
            return kiln_c99_text_append_c(text, "null");
        }
        /* An integer under 2^53 spells exactly in decimal from its bit pattern with no rounding
         * question to ask, so it is formatted here with a plain C conversion instead of a call
         * across the FFI into the Rust number formatter -- the common case for a counter, an id,
         * an index, or (as `-0.0 < 0.0` is false) `-0`, which this prints as "0" for the same
         * reason `kiln_c99_text`'s FFI path already does. Anything wider, fractional, or
         * non-finite keeps going through that formatter, which is the one place that knows the
         * full `Number::toString` grammar (exponential notation, the shortest round-tripping
         * decimal, and so on). */
        if (floor(number) == number && fabs(number) < 9007199254740992.0 /* 2^53 */) {
            if (number < 0.0) {
                return kiln_c99_text_append(text, "-", 1) != KILN_C99_OK
                           ? KILN_C99_OUT_OF_MEMORY
                           : kiln_c99_json_append_uint(text, (uint64_t)(-number));
            }
            return kiln_c99_json_append_uint(text, (uint64_t)number);
        }
        shown = kiln_c99_text(value);
        if (shown.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a number with no text form to serialise");
        }
        return kiln_c99_text_append(text, shown.ptr, shown.len);
    }
    case KILN_VALUE_STRING:
        return kiln_c99_json_quote(text, value.ptr, value.len);
    case KILN_VALUE_BIGINT:
        /* BigInt has no JSON representation.  This is a TypeError, rather than an omitted
         * value, and must travel through the ordinary thrown-value path so callers can catch it. */
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Do not know how to serialize a BigInt");
    default:
        break;
    }
    /* A callable and a symbol are the two things `JSON.stringify` omits: a property holding one
     * disappears from the object, and one passed directly answers `undefined`. */
    if (kiln_c99_is_callable(value) || value.tag == KILN_VALUE_SYMBOL) {
        *serialised = 0;
        return KILN_C99_OK;
    }
    /* Every other remaining reference is an ordinary object to `JSON.stringify`, and one with no
     * own enumerable string-keyed property serialises as `{}` -- which is what node answers for a
     * Map, a Set, a promise, a generator, a regexp and a buffer alike, and what the interpreter
     * already answered. Only the four below have a store worth walking.
     *
     * Listing the four and dropping the rest is the allowlist mistake this file has now made
     * four times over: a `Map` came out as `undefined` at the top level, as a *missing key*
     * inside an object, and as `null` inside an array -- three different wrong answers from one
     * missing arm, none of which stopped the program. */
    if (value.tag != KILN_VALUE_OBJECT && value.tag != KILN_VALUE_ARRAY &&
        value.tag != KILN_VALUE_TYPED_ARRAY && value.tag != KILN_VALUE_PROXY) {
        return kiln_c99_text_append_c(text, "{}");
    }

    /* With no indent every separator is empty, which collapses the pretty form to the compact
     * one and lets both share one code path. */
    if (indent_len != 0 &&
        (kiln_c99_text_append(&nested, current, current_len) != KILN_C99_OK ||
         kiln_c99_text_append(&nested, indent, indent_len) != KILN_C99_OK)) {
        free(nested.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }

#define KILN_JSON_OPEN(bracket)                                                                   \
    do {                                                                                          \
        if (kiln_c99_text_append(text, bracket, 1) != KILN_C99_OK) {                               \
            free(nested.bytes);                                                                    \
            return KILN_C99_OUT_OF_MEMORY;                                                          \
        }                                                                                          \
    } while (0)
#define KILN_JSON_BREAK(first)                                                                    \
    do {                                                                                          \
        if (!(first) && kiln_c99_text_append(text, ",", 1) != KILN_C99_OK) {                       \
            free(nested.bytes);                                                                    \
            return KILN_C99_OUT_OF_MEMORY;                                                          \
        }                                                                                          \
        if (indent_len != 0 &&                                                                     \
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||                                  \
             kiln_c99_text_append(text, nested.bytes, nested.len) != KILN_C99_OK)) {                \
            free(nested.bytes);                                                                    \
            return KILN_C99_OUT_OF_MEMORY;                                                          \
        }                                                                                          \
    } while (0)

    if (value.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(value);
        /* A container that is already being written is a cycle, and the language stops rather
         * than producing a document that cannot exist. Without this the recursion below runs
         * until the C stack is gone, which ends the process with nothing printed and nothing
         * said -- the worst answer of the three available. */
        for (ancestor = parents; ancestor != NULL; ancestor = ancestor->parent) {
            if (ancestor->container == (const void *)array) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Converting circular structure to JSON");
            }
        }
        frame.parent = parents;
        frame.container = (const void *)array;
        KILN_JSON_OPEN("[");
        for (index = 0; index < array->count; ++index) {
            int wrote;
            KILN_JSON_BREAK(index == 0);
            status = kiln_c99_json_value(host, text, kiln_val_to_boundary(array->elements[index]), indent, indent_len,
                                         nested.bytes, nested.len, &wrote, depth, &frame, thrown);
            if (status != KILN_C99_OK) {
                free(nested.bytes);
                return status;
            }
            /* An element with no JSON spelling -- undefined, a function, a hole -- becomes null
             * rather than disappearing: an array's positions are part of its meaning. */
            if (!wrote && kiln_c99_text_append_c(text, "null") != KILN_C99_OK) {
                free(nested.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        if (indent_len != 0 && array->count != 0 &&
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||
             kiln_c99_text_append(text, current, current_len) != KILN_C99_OK)) {
            free(nested.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        free(nested.bytes);
        return kiln_c99_text_append_c(text, "]");
    }
    if (value.tag == KILN_VALUE_TYPED_ARRAY) {
        /* An *object* keyed by index, not an array: `JSON.stringify(new Uint8Array([1, 2]))` is
         * `{"0":1,"1":2}`. A view is not an Array -- `Array.isArray` says so -- and JSON's array
         * form is reserved for things that are, so the generic object walk over own enumerable
         * properties is what applies, and a view's own enumerable properties are its indices. */
        kiln_typed_array *view = kiln_c99_typed_array_of(value);
        KILN_JSON_OPEN("{");
        for (index = 0; index < view->length; ++index) {
            char digits[24];
            int written;
            int wrote;
            written = kiln_number_to_js_string((double)index, digits, sizeof digits);
            if (written < 0) {
                free(nested.bytes);
                KILN_C99_REFUSE("a typed-array index with no string spelling");
            }
            KILN_JSON_BREAK(index == 0);
            if (kiln_c99_json_quote(text, digits, (size_t)written) != KILN_C99_OK ||
                (indent_len != 0 ? kiln_c99_text_append(text, ": ", 2) : kiln_c99_text_append(text, ":", 1)) != KILN_C99_OK) {
                free(nested.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            /* Through the same recursion an object member takes, so a `Float64Array` holding
             * NaN spells it `null` here for the same reason a plain number does. */
            status = kiln_c99_json_value(host, text, kiln_c99_number(kiln_c99_typed_get(view, index)),
                                         indent, indent_len, nested.bytes, nested.len, &wrote,
                                         depth, parents, thrown);
            if (status != KILN_C99_OK) {
                free(nested.bytes);
                return status;
            }
        }
        if (indent_len != 0 && view->length != 0 &&
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||
             kiln_c99_text_append(text, current, current_len) != KILN_C99_OK)) {
            free(nested.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        free(nested.bytes);
        return kiln_c99_text_append_c(text, "}");
    }
    if (value.tag == KILN_VALUE_PROXY) {
        /* Read entirely through the handler: an `ownKeys` for the shape and a `get` per member,
         * which is what the language specifies and what makes a virtual object serialise as the
         * thing it presents rather than as the target hiding behind it.
         *
         * Whether it is written as an array or as an object follows the target, exactly as
         * `Array.isArray` does -- the two must agree or a proxy of an array serialises with
         * quoted indices. */
        int is_array = kiln_c99_proxy_inner(value).tag == KILN_VALUE_ARRAY;
        kiln_value keys;
        size_t count = 0;
        size_t written = 0;
        for (ancestor = parents; ancestor != NULL; ancestor = ancestor->parent) {
            if (ancestor->container == (const void *)kiln_c99_proxy_of(value)) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Converting circular structure to JSON");
            }
        }
        frame.parent = parents;
        frame.container = (const void *)kiln_c99_proxy_of(value);
        if (is_array) {
            kiln_value length_value;
            double length;
            status = kiln_c99_get(host, value, "length", 6, &length_value, depth, thrown);
            if (status != KILN_C99_OK) {
                free(nested.bytes);
                return status;
            }
            length = kiln_c99_as_number(length_value);
            if (!(length >= 0.0)) {
                length = 0.0;
            }
            count = (size_t)length;
            keys = kiln_c99_undefined();
        } else {
            status = kiln_c99_own_keys(host, value, &keys, depth, thrown);
            if (status != KILN_C99_OK) {
                free(nested.bytes);
                return status;
            }
            count = kiln_c99_array_of(keys)->count;
        }
        KILN_JSON_OPEN(is_array ? "[" : "{");
        for (index = 0; index < count; ++index) {
            kiln_value key = is_array ? kiln_c99_text(kiln_c99_number((double)index))
                                      : kiln_val_to_boundary(kiln_c99_array_of(keys)->elements[index]);
            kiln_value member;
            /* Written straight into the parent builder now, not a per-member buffer: the
             * separator and the key are speculative, and a member with no JSON spelling rolls
             * all three back to `mark` in one assignment instead of freeing a scratch
             * allocation this loop no longer makes. An array position never rolls back -- its
             * `null` replaces the value in place of the roll back, since a position is kept
             * either way. */
            size_t mark = text->len;
            int wrote;
            status = kiln_c99_get_computed(host, value, key, &member, depth, thrown);
            if (status != KILN_C99_OK) {
                free(nested.bytes);
                return status;
            }
            KILN_JSON_BREAK(written == 0);
            if (!is_array && (kiln_c99_json_quote(text, key.ptr, key.len) != KILN_C99_OK ||
                              (indent_len != 0 ? kiln_c99_text_append(text, ": ", 2) : kiln_c99_text_append(text, ":", 1)) !=
                                  KILN_C99_OK)) {
                free(nested.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            status = kiln_c99_json_value(host, text, member, indent, indent_len, nested.bytes,
                                         nested.len, &wrote, depth, &frame, thrown);
            if (status != KILN_C99_OK) {
                free(nested.bytes);
                return status;
            }
            if (!wrote) {
                if (!is_array) {
                    text->len = mark;
                    continue;
                }
                if (kiln_c99_text_append_c(text, "null") != KILN_C99_OK) {
                    free(nested.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            written += 1;
        }
        if (indent_len != 0 && written != 0 &&
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||
             kiln_c99_text_append(text, current, current_len) != KILN_C99_OK)) {
            free(nested.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        free(nested.bytes);
        return kiln_c99_text_append_c(text, is_array ? "]" : "}");
    }
    {
        kiln_object *object = kiln_c99_object_of(value);
        size_t written = 0;
        size_t length;
        /* The same cycle check the array branch makes, and for the same reason: `c.self = c` is
         * the shortest way in. */
        for (ancestor = parents; ancestor != NULL; ancestor = ancestor->parent) {
            if (ancestor->container == (const void *)object) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Converting circular structure to JSON");
            }
        }
        frame.parent = parents;
        frame.container = (const void *)object;
        KILN_JSON_OPEN("{");
        if (object->properties == NULL && object->shape != NULL) {
            /* The shaped fast path: every member is read straight out of `elements`/`slots`
             * with no `kiln_c99_own_keys` array of copied key strings and no `get_computed` by
             * boundary key, and every member is written straight into the parent builder with
             * no buffer of its own -- the separator and the (already-known-length) key are
             * written speculatively at `mark`, and a member with no JSON spelling rolls all
             * three back in one assignment rather than freeing a scratch allocation that no
             * longer exists. */
            if (object->elements != NULL) {
                for (index = 0; index < object->element_count; ++index) {
                    char num_buf[32];
                    int nlen;
                    kiln_value member;
                    size_t mark;
                    int wrote;
                    if (object->elements[index] == KILN_VAL_HOLE) continue;
                    nlen = kiln_number_to_js_string((double)index, num_buf, sizeof num_buf);
                    member = kiln_val_to_boundary(object->elements[index]);
                    mark = text->len;
                    KILN_JSON_BREAK(written == 0);
                    if (kiln_c99_json_quote(text, num_buf, (size_t)nlen) != KILN_C99_OK ||
                        (indent_len != 0 ? kiln_c99_text_append(text, ": ", 2) : kiln_c99_text_append(text, ":", 1)) != KILN_C99_OK) {
                        free(nested.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    status = kiln_c99_json_value(host, text, member, indent, indent_len,
                                                 nested.bytes, nested.len, &wrote, depth, &frame, thrown);
                    if (status != KILN_C99_OK) {
                        free(nested.bytes);
                        return status;
                    }
                    if (!wrote) {
                        text->len = mark;
                        continue;
                    }
                    written += 1;
                }
            }
            length = object->shape->property_count;
            for (index = 0; index < length && object->shape != NULL && index < object->shape->property_count; ++index) {
                const char *skey = object->shape->keys[index];
                size_t key_len = strlen(skey);
                kiln_value member;
                size_t mark;
                int wrote;
                if (key_len >= 2 && skey[0] == '@' && skey[1] == '@') {
                    continue;
                }
                member = (object->slots != NULL && index < object->slot_capacity)
                             ? kiln_val_to_boundary(object->slots[index])
                             : kiln_c99_undefined();
                mark = text->len;
                KILN_JSON_BREAK(written == 0);
                if (kiln_c99_json_quote(text, skey, key_len) != KILN_C99_OK ||
                    (indent_len != 0 ? kiln_c99_text_append(text, ": ", 2) : kiln_c99_text_append(text, ":", 1)) != KILN_C99_OK) {
                    free(nested.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                status = kiln_c99_json_value(host, text, member, indent, indent_len,
                                             nested.bytes, nested.len, &wrote, depth, &frame, thrown);
                if (status != KILN_C99_OK) {
                    free(nested.bytes);
                    return status;
                }
                if (!wrote) {
                    text->len = mark;
                    continue;
                }
                written += 1;
            }
        } else {
            if (object->shape != NULL) {
                kiln_c99_ensure_dictionary(object);
            }
            length = object->count;
            for (index = 0; index < length && index < object->count; ++index) {
            kiln_value member;
            size_t mark;
            int wrote;
            if (!object->properties[index].enumerable) {
                continue;
            }
            if (object->properties[index].accessor) {
                if (object->properties[index].getter.tag == KILN_VALUE_UNDEFINED) {
                    continue;
                }
                status = kiln_c99_call(host, &member, object->properties[index].getter, value,
                                       NULL, 0, depth, thrown);
                if (status != KILN_C99_OK) {
                    free(nested.bytes);
                    return status;
                }
            } else {
                member = object->properties[index].value;
            }
            /* Written straight into `text` at `mark`: a member with no JSON spelling is
             * *dropped*, so on that answer the separator and key just written are rolled back
             * along with it rather than never written in the first place. */
            mark = text->len;
            KILN_JSON_BREAK(written == 0);
            if (kiln_c99_json_quote(text, object->properties[index].key,
                                    object->properties[index].key_len) != KILN_C99_OK ||
                (indent_len != 0 ? kiln_c99_text_append(text, ": ", 2) : kiln_c99_text_append(text, ":", 1)) != KILN_C99_OK) {
                free(nested.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            status = kiln_c99_json_value(host, text, member, indent, indent_len, nested.bytes,
                                         nested.len, &wrote, depth, &frame, thrown);
            if (status != KILN_C99_OK) {
                free(nested.bytes);
                return status;
            }
            if (!wrote) {
                text->len = mark;
                continue;
            }
            written += 1;
        }
    }
    if (indent_len != 0 && written != 0 &&
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||
             kiln_c99_text_append(text, current, current_len) != KILN_C99_OK)) {
            free(nested.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        free(nested.bytes);
        return kiln_c99_text_append_c(text, "}");
    }
#undef KILN_JSON_OPEN
#undef KILN_JSON_BREAK
}

/* A recursive-descent JSON parser over the source bytes.
 *
 * `at` is the cursor, moved forward as text is consumed. A malformed document is a control error
 * rather than a partial value: JSON.parse either produces the whole thing or nothing. */

static int kiln_c99_json_read(kiln_json_reader *reader, kiln_value *out);

static void kiln_c99_json_space(kiln_json_reader *reader) {
    while (reader->at < reader->len) {
        char byte = reader->bytes[reader->at];
        if (byte != ' ' && byte != '\t' && byte != '\n' && byte != '\r') {
            return;
        }
        reader->at += 1;
    }
}

static int kiln_c99_json_literal(kiln_json_reader *reader, const char *word) {
    size_t len = strlen(word);
    if (reader->len - reader->at < len || memcmp(reader->bytes + reader->at, word, len) != 0) {
        return 0;
    }
    reader->at += len;
    return 1;
}

static int kiln_c99_json_string(kiln_json_reader *reader, kiln_value *out) {
    kiln_text text = {NULL, 0, 0};
    uint16_t pending_high = 0;
    reader->at += 1;
    /* The common case by far -- a string with no escape and no embedded control byte -- copies in
     * one allocation, at `kiln_c99_copy_text`, rather than growing a builder one
     * `kiln_c99_text_append` call per byte and copying it again at the end. Same shape as
     * `kiln_c99_json_needs_escape` on the stringify side: one pass of plain comparisons either
     * finds the closing quote, or finds the reason (an escape, a control byte, unterminated
     * input) this string needs the byte-by-byte path below, which is untouched and still owns
     * every one of those cases exactly as before. */
    {
        size_t start = reader->at;
        size_t scan = start;
        while (scan < reader->len) {
            unsigned char byte = (unsigned char)reader->bytes[scan];
            if (byte == '"') {
                *out = kiln_c99_copy_text(reader->bytes + start, scan - start);
                reader->at = scan + 1;
                return KILN_C99_OK;
            }
            if (byte == '\\' || byte < 0x20) {
                break;
            }
            ++scan;
        }
    }
    while (reader->at < reader->len) {
        char byte = reader->bytes[reader->at];
        if (byte == '"') {
            reader->at += 1;
            /* A high surrogate left unpaired at the end is written as the replacement
             * character, for the same reason as everywhere else: this string cannot hold one. */
            if (pending_high != 0 &&
                kiln_c99_text_append_c(&text, "\xEF\xBF\xBD") != KILN_C99_OK) {
                free(text.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            *out = kiln_c99_text_value(&text);
            return KILN_C99_OK;
        }
        if (byte != '\\') {
            /* JSON strings may contain ordinary Unicode text, but the sixteen C0 control
             * characters must be escaped.  Letting one through would make JSON.parse accept a
             * JavaScript source string that the JSON grammar rejects. */
            if ((unsigned char)byte < 0x20) {
                free(text.bytes);
                KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
            }
            if (pending_high != 0) {
                if (kiln_c99_text_append_c(&text, "\xEF\xBF\xBD") != KILN_C99_OK) {
                    free(text.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                pending_high = 0;
            }
            if (kiln_c99_text_append(&text, &byte, 1) != KILN_C99_OK) {
                free(text.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            reader->at += 1;
            continue;
        }
        reader->at += 1;
        if (reader->at >= reader->len) {
            break;
        }
        {
            char escaped = reader->bytes[reader->at];
            const char *literal = NULL;
            reader->at += 1;
            switch (escaped) {
            case '"': literal = "\""; break;
            case '\\': literal = "\\"; break;
            case '/': literal = "/"; break;
            case 'n': literal = "\n"; break;
            case 't': literal = "\t"; break;
            case 'r': literal = "\r"; break;
            case 'b': literal = "\b"; break;
            case 'f': literal = "\f"; break;
            case 'u': {
                uint16_t unit = 0;
                uint16_t pair[2];
                size_t digit;
                if (reader->len - reader->at < 4) {
                    free(text.bytes);
                    KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a "
                                             "SyntaxError");
                }
                for (digit = 0; digit < 4; ++digit) {
                    char hex = reader->bytes[reader->at + digit];
                    unit = (uint16_t)(unit << 4);
                    if (hex >= '0' && hex <= '9') {
                        unit = (uint16_t)(unit | (uint16_t)(hex - '0'));
                    } else if (hex >= 'a' && hex <= 'f') {
                        unit = (uint16_t)(unit | (uint16_t)(hex - 'a' + 10));
                    } else if (hex >= 'A' && hex <= 'F') {
                        unit = (uint16_t)(unit | (uint16_t)(hex - 'A' + 10));
                    } else {
                        free(text.bytes);
                        KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a "
                                                 "SyntaxError");
                    }
                }
                reader->at += 4;
                /* A surrogate pair is two escapes and one character, so the high half is held
                 * until the low half arrives. */
                if (unit >= 0xD800 && unit <= 0xDBFF) {
                    pair[0] = unit;
                    if (pending_high != 0) {
                        kiln_value lone = kiln_c99_from_units(&pending_high, 1);
                        if (kiln_c99_text_append(&text, lone.ptr, lone.len) != KILN_C99_OK) {
                            free(text.bytes);
                            return KILN_C99_OUT_OF_MEMORY;
                        }
                    }
                    pending_high = pair[0];
                    continue;
                }
                if (pending_high != 0) {
                    pair[0] = pending_high;
                    pair[1] = unit;
                    pending_high = 0;
                    {
                        kiln_value combined = kiln_c99_from_units(pair, 2);
                        if (kiln_c99_text_append(&text, combined.ptr, combined.len) !=
                            KILN_C99_OK) {
                            free(text.bytes);
                            return KILN_C99_OUT_OF_MEMORY;
                        }
                    }
                    continue;
                }
                {
                    kiln_value single = kiln_c99_from_units(&unit, 1);
                    if (kiln_c99_text_append(&text, single.ptr, single.len) != KILN_C99_OK) {
                        free(text.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
                continue;
            }
            default:
                free(text.bytes);
                KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
            }
            if (pending_high != 0) {
                if (kiln_c99_text_append_c(&text, "\xEF\xBF\xBD") != KILN_C99_OK) {
                    free(text.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                pending_high = 0;
            }
            if (kiln_c99_text_append_c(&text, literal) != KILN_C99_OK) {
                free(text.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
    }
    free(text.bytes);
    KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
}

/* A property key for the object case below, returned as an already-interned C string rather
 * than a materialized value: a parsed key never survives past the `kiln_c99_json_define` call
 * that follows it, so building a GC string only to have the object store intern its bytes a
 * second time was two copies paid for a value nothing keeps. The scan is
 * `kiln_c99_json_string`'s own fast path, repeated here rather than shared, because an object key
 * is exactly as likely to be a plain run of bytes with no escape as any other JSON string; a key
 * that needs the escape machinery rewinds to the opening quote and falls back to that function
 * unchanged, interning what it produces instead of a second allocation's worth of bytes. */
static int kiln_c99_json_object_key(kiln_json_reader *reader, const char **out_ikey) {
    size_t open = reader->at;
    size_t start;
    size_t scan;
    reader->at += 1;
    start = reader->at;
    scan = start;
    while (scan < reader->len) {
        unsigned char byte = (unsigned char)reader->bytes[scan];
        if (byte == '"') {
            const char *ikey = kiln_c99_intern(reader->bytes + start, scan - start);
            if (ikey == NULL) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            *out_ikey = ikey;
            reader->at = scan + 1;
            return KILN_C99_OK;
        }
        if (byte == '\\' || byte < 0x20) {
            break;
        }
        ++scan;
    }
    {
        kiln_value materialized;
        int status;
        reader->at = open;
        status = kiln_c99_json_string(reader, &materialized);
        if (status != KILN_C99_OK) {
            return status;
        }
        *out_ikey = kiln_c99_intern(materialized.ptr, materialized.len);
        return *out_ikey == NULL ? KILN_C99_OUT_OF_MEMORY : KILN_C99_OK;
    }
}

/* Defines `member` on `built` under the already-interned `ikey`, taking the shape transition
 * directly (the same pattern `kiln_c99_define_named_fast` uses for an object literal) rather than
 * going through `kiln_c99_put`'s general assignment semantics. That distinction is not just
 * speed: `kiln_c99_put_mode` treats a key spelled `__proto__` as a request to relink the
 * prototype, which is the rule an object literal and a plain assignment both follow -- but
 * `JSON.parse`'s own step here is CreateDataProperty, so `{"__proto__":{}}` must parse to an
 * object with an *own property* named `__proto__`, not a relinked one. Going straight to the
 * shape sidesteps that rule entirely, which is what makes this correct and not only fast.
 *
 * `built` is always the reader's own freshly created object -- shape-based, extensible, no
 * accessors, exactly `kiln_c99_new_object`'s starting state -- so the fast-property paths below
 * are the only ones a freshly parsed object can need; a document with more distinct keys on one
 * object than the shape tree allows falls back to `kiln_c99_put`, which switches it to
 * dictionary mode exactly as any other object growing past the same limit would. A repeated key
 * is `find_slot` overwriting the earlier value in place, which is JSON.parse's last-one-wins rule
 * for free. */
static int kiln_c99_json_define(kiln_value built, const char *ikey, kiln_value member) {
    kiln_object *object = kiln_c99_object_of(built);
    uint32_t slot;
    kiln_shape *next;
    if (object->shape != NULL) {
        if (kiln_c99_shape_find_slot(object->shape, ikey, &slot) && slot < object->slot_capacity) {
            object->slots[slot] = kiln_val_from_boundary(member);
            return KILN_C99_OK;
        }
        if (kiln_c99_shape_transition(object->shape, ikey, &next)) {
            if ((size_t)next->slot_index >= object->slot_capacity) {
                int grown = kiln_c99_grow_slots(object, (size_t)next->slot_index + 1);
                if (grown != KILN_C99_OK) {
                    return grown;
                }
            }
            object->slots[next->slot_index] = kiln_val_from_boundary(member);
            object->shape = next;
            return KILN_C99_OK;
        }
    }
    return kiln_c99_put(NULL, built, ikey, strlen(ikey), member, 0, NULL);
}

/* Clinger's fast path for the common decimal grammar: when the mantissa fits exactly in a double
 * (at most fifteen significant digits) and the decimal exponent is one a power of ten also
 * represents exactly (|k| <= 22 -- 5^22 needs under 53 bits, so 10^22 = 5^22 * 2^22 is exact, and
 * every smaller power of ten more so), `mantissa * 10^k` or `mantissa / 10^k` is a single
 * correctly-rounded IEEE operation on two exact operands, which is the same double the
 * arbitrary-precision conversion below would produce. Outside that range -- more digits, a wider
 * exponent -- this says so truthfully by returning 0, and the caller keeps using the FFI parser,
 * which is what every number able to defeat this shortcut still needs.
 *
 * Sign is applied last, as a negation rather than a conditional skip, which is what keeps "-0"
 * `-0`: negating a zero magnitude produces negative zero exactly, the same way `-0.0` is spelled
 * in C. */
static const double kiln_c99_json_pow10[23] = {
    1e0,  1e1,  1e2,  1e3,  1e4,  1e5,  1e6,  1e7,  1e8,  1e9,  1e10, 1e11,
    1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
};

static int kiln_c99_json_fast_number(const char *bytes, size_t len, double *out) {
    size_t i = 0;
    int negative = 0;
    uint64_t mantissa = 0;
    int mantissa_digits = 0;
    int seen_dot = 0;
    int fraction_digits = 0;
    int exponent = 0;
    int exponent_negative = 0;
    int decimal_exponent;
    double value;

    if (i < len && bytes[i] == '-') {
        negative = 1;
        ++i;
    }
    for (; i < len; ++i) {
        char c = bytes[i];
        if (c == '.') {
            if (seen_dot) return 0;
            seen_dot = 1;
            continue;
        }
        if (c == 'e' || c == 'E') break;
        if (mantissa_digits >= 15) {
            /* More significant digits than a double can hold exactly: outside the range this
             * shortcut can answer for, so it defers rather than round the wrong digit away. */
            return 0;
        }
        mantissa = mantissa * 10 + (uint64_t)(c - '0');
        ++mantissa_digits;
        if (seen_dot) ++fraction_digits;
    }
    if (i < len && (bytes[i] == 'e' || bytes[i] == 'E')) {
        ++i;
        if (i < len && (bytes[i] == '+' || bytes[i] == '-')) {
            exponent_negative = bytes[i] == '-';
            ++i;
        }
        for (; i < len; ++i) {
            if (exponent > 1000) return 0; /* absurd; let the FFI's own bound decide */
            exponent = exponent * 10 + (bytes[i] - '0');
        }
    }
    decimal_exponent = (exponent_negative ? -exponent : exponent) - fraction_digits;
    if (decimal_exponent < -22 || decimal_exponent > 22) return 0;
    value = (double)mantissa;
    if (decimal_exponent >= 0) {
        value *= kiln_c99_json_pow10[decimal_exponent];
    } else {
        value /= kiln_c99_json_pow10[-decimal_exponent];
    }
    if (negative) value = -value;
    *out = value;
    return 1;
}

static int kiln_c99_json_read(kiln_json_reader *reader, kiln_value *out) {
    int status;
    if (reader->depth >= KILN_C99_MAX_CALL_DEPTH) {
        return KILN_C99_CALL_DEPTH_ERROR;
    }
    kiln_c99_json_space(reader);
    if (reader->at >= reader->len) {
        KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
    }
    switch (reader->bytes[reader->at]) {
    case 'n':
        if (!kiln_c99_json_literal(reader, "null")) {
            KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
        }
        *out = kiln_c99_null();
        return KILN_C99_OK;
    case 't':
        if (!kiln_c99_json_literal(reader, "true")) {
            KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
        }
        *out = kiln_c99_boolean(1);
        return KILN_C99_OK;
    case 'f':
        if (!kiln_c99_json_literal(reader, "false")) {
            KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
        }
        *out = kiln_c99_boolean(0);
        return KILN_C99_OK;
    case '"':
        return kiln_c99_json_string(reader, out);
    case '[': {
        kiln_value built;
        reader->at += 1;
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        kiln_c99_json_space(reader);
        if (reader->at < reader->len && reader->bytes[reader->at] == ']') {
            reader->at += 1;
            *out = built;
            return KILN_C99_OK;
        }
        for (;;) {
            kiln_value element;
            reader->depth += 1;
            status = kiln_c99_json_read(reader, &element);
            reader->depth -= 1;
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_array_push(built, element);
            if (status != KILN_C99_OK) {
                return status;
            }
            kiln_c99_json_space(reader);
            if (reader->at >= reader->len) {
                KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
            }
            if (reader->bytes[reader->at] == ',') {
                reader->at += 1;
                continue;
            }
            if (reader->bytes[reader->at] == ']') {
                reader->at += 1;
                *out = built;
                return KILN_C99_OK;
            }
            KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
        }
    }
    case '{': {
        kiln_value built;
        reader->at += 1;
        status = kiln_c99_new_object(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        kiln_c99_json_space(reader);
        if (reader->at < reader->len && reader->bytes[reader->at] == '}') {
            reader->at += 1;
            *out = built;
            return KILN_C99_OK;
        }
        for (;;) {
            const char *ikey;
            kiln_value member;
            kiln_c99_json_space(reader);
            if (reader->at >= reader->len || reader->bytes[reader->at] != '"') {
                KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
            }
            status = kiln_c99_json_object_key(reader, &ikey);
            if (status != KILN_C99_OK) {
                return status;
            }
            kiln_c99_json_space(reader);
            if (reader->at >= reader->len || reader->bytes[reader->at] != ':') {
                KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
            }
            reader->at += 1;
            reader->depth += 1;
            status = kiln_c99_json_read(reader, &member);
            reader->depth -= 1;
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_json_define(built, ikey, member);
            if (status != KILN_C99_OK) {
                return status;
            }
            kiln_c99_json_space(reader);
            if (reader->at >= reader->len) {
                KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
            }
            if (reader->bytes[reader->at] == ',') {
                reader->at += 1;
                continue;
            }
            if (reader->bytes[reader->at] == '}') {
                reader->at += 1;
                *out = built;
                return KILN_C99_OK;
            }
            KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
        }
    }
    default: {
        /* A number. JSON's grammar is narrower than the language's, so the extent is measured
         * here and the digits themselves are handed to the shared parser -- one place that knows
         * how a decimal becomes a double. */
        size_t start = reader->at;
        size_t digits;
        if (reader->bytes[reader->at] == '-') {
            reader->at += 1;
        }
        if (reader->at >= reader->len) {
            KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
        }
        if (reader->bytes[reader->at] == '0') {
            reader->at += 1;
            /* JSON disallows leading zeroes; the following digit is not a new token. */
            if (reader->at < reader->len && reader->bytes[reader->at] >= '0' &&
                reader->bytes[reader->at] <= '9') {
                KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
            }
        } else if (reader->bytes[reader->at] >= '1' && reader->bytes[reader->at] <= '9') {
            do {
                reader->at += 1;
            } while (reader->at < reader->len && reader->bytes[reader->at] >= '0' &&
                     reader->bytes[reader->at] <= '9');
        } else {
            KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
        }
        if (reader->at < reader->len && reader->bytes[reader->at] == '.') {
            reader->at += 1;
            digits = reader->at;
            while (reader->at < reader->len && reader->bytes[reader->at] >= '0' &&
                   reader->bytes[reader->at] <= '9') {
                reader->at += 1;
            }
            if (reader->at == digits) {
                KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
            }
        }
        if (reader->at < reader->len &&
            (reader->bytes[reader->at] == 'e' || reader->bytes[reader->at] == 'E')) {
            reader->at += 1;
            if (reader->at < reader->len &&
                (reader->bytes[reader->at] == '+' || reader->bytes[reader->at] == '-')) {
                reader->at += 1;
            }
            digits = reader->at;
            while (reader->at < reader->len && reader->bytes[reader->at] >= '0' &&
                   reader->bytes[reader->at] <= '9') {
                reader->at += 1;
            }
            if (reader->at == digits) {
                KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
            }
        }
        {
            double number;
            if (!kiln_c99_json_fast_number(reader->bytes + start, reader->at - start, &number)) {
                number = kiln_string_to_number(reader->bytes + start, reader->at - start);
                if (isnan(number)) {
                    KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
                }
            }
            *out = kiln_c99_number(number);
            return KILN_C99_OK;
        }
    }
    }
}

/* Parse one complete captured JSON module document. This is the same reader `JSON.parse` uses,
 * but data-module materialization has no JavaScript call frame or reviver and needs a direct
 * private entry point. */
static int kiln_c99_json_document(const char *bytes, size_t len, kiln_value *out) {
    kiln_json_reader reader;
    int status;
    reader.bytes = bytes;
    reader.len = len;
    reader.at = 0;
    reader.depth = 0;
    status = kiln_c99_json_read(&reader, out);
    if (status != KILN_C99_OK) return status;
    kiln_c99_json_space(&reader);
    if (reader.at != reader.len) {
        KILN_C99_REFUSE_INTERNAL("JSON.parse converts a reader refusal into a SyntaxError");
    }
    return KILN_C99_OK;
}
