/* Does any byte in this word need `\"`, `\\` or `\u00XX` escaping?
 *
 * The standard word tricks: `(w - 0x01..01 * n) & ~w & 0x80..80` has a high bit set in every byte
 * lane below `n`, and xor-ing against a word of one byte turns "is any lane equal to it" into the
 * same question about zero. Three word tests decide eight bytes, and they are exactly as valid
 * for a multi-byte UTF-8 string as for ASCII, since every continuation and lead byte is >= 0x80
 * and so never equals a quote, a backslash or a C0 control. */
#define KILN_JSON_ONES UINT64_C(0x0101010101010101)
#define KILN_JSON_HIGH UINT64_C(0x8080808080808080)

static inline int kiln_c99_json_word_escapes(uint64_t word) {
    uint64_t quotes = word ^ (KILN_JSON_ONES * (uint64_t)'"');
    uint64_t slashes = word ^ (KILN_JSON_ONES * (uint64_t)'\\');
    return ((word - KILN_JSON_ONES * 0x20u) & ~word & KILN_JSON_HIGH) != 0 ||
           ((quotes - KILN_JSON_ONES) & ~quotes & KILN_JSON_HIGH) != 0 ||
           ((slashes - KILN_JSON_ONES) & ~slashes & KILN_JSON_HIGH) != 0;
}

/* Whether any byte in this span needs escaping; the predicate the slow path below asks once it
 * knows the span does not fit in the builder as it stands. */
static int kiln_c99_json_needs_escape(const char *bytes, size_t len) {
    size_t index = 0;
    while (index + 8 <= len) {
        uint64_t word;
        memcpy(&word, bytes + index, sizeof word);
        if (kiln_c99_json_word_escapes(word)) return 1;
        index += 8;
    }
    for (; index < len; ++index) {
        unsigned char byte = (unsigned char)bytes[index];
        if (byte == '"' || byte == '\\' || byte < 0x20) return 1;
    }
    return 0;
}

#undef KILN_JSON_ONES
#undef KILN_JSON_HIGH

/* One byte per row: 1 where JSON has to escape the byte, 0 where it stands for itself. A table
 * read, rather than the three comparisons it replaces, is what the copy loop below wants -- the
 * strings a document is made of are keys and short values of a handful of bytes each, so the
 * per-byte test is the loop, and clang compiled the three comparisons into eight instructions a
 * byte (it duplicated the equality tests to carry the exit out of the loop). One load and one
 * branch is four. */
static const unsigned char kiln_c99_json_escape_class[256] = {
    /* 0x00 */ 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    /* 0x10 */ 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    /* 0x20 */ 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0x30 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0x40 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0x50 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0,
    /* 0x60 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0x70 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0x80 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0x90 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0xA0 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0xB0 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0xC0 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0xD0 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0xE0 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    /* 0xF0 */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
};

/* Quotes and copies the span into the builder in one pass, or answers 0 having changed nothing.
 *
 * The bytes are examined and copied together, and the write is committed only by the `text->len`
 * at the end -- so a span that turns out to need escaping, or one the builder has no room for,
 * simply leaves the general path below to do the work.
 *
 * What stood here was a separate predicate pass over the span followed by three
 * `kiln_c99_text_append` calls, each repeating the same capacity test for between one byte and
 * the whole string. A document is mostly keys and short string values, so that was two passes and
 * four capacity tests per member to copy a dozen bytes. */
static int kiln_c99_json_quote_plain(kiln_text *text, const char *bytes, size_t len) {
    char *at;
    size_t copied;
    if (KILN_UNLIKELY(text->len + len + 2 > text->capacity)) {
        return 0;
    }
    at = text->bytes + text->len + 1;
    for (copied = 0; copied < len; ++copied) {
        unsigned char byte = (unsigned char)bytes[copied];
        if (KILN_UNLIKELY(kiln_c99_json_escape_class[byte] != 0)) {
            return 0;
        }
        at[copied] = (char)byte;
    }
    at[-1] = '"';
    at[len] = '"';
    text->len += len + 2;
    return 1;
}

static int kiln_c99_json_quote(kiln_text *text, const char *bytes, size_t len) {
    static const char digits[] = "0123456789abcdef";
    size_t index;
    if (kiln_c99_json_quote_plain(text, bytes, len)) {
        return KILN_C99_OK;
    }
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

/* The four scalars a document is mostly made of, written straight from the heap's own
 * representation.
 *
 * Every member of an array or a shaped object is offered here before the general walk is entered,
 * and each of these is decided by one test on the tagged word. `kiln_c99_json_value` would first
 * convert that word to a boundary `kiln_value`, ask whether the value might carry a `toJSON`, and
 * initialise a nested builder for an indent the compact form does not have -- none of which a
 * number, a string, a boolean or null has any use for, and all of which was being paid once per
 * member of every document.
 *
 * Answers 0 for anything else, including `undefined` and a hole, which the caller hands to
 * `kiln_c99_json_value` unchanged -- so the omission rules stay in exactly one place. */
static int kiln_c99_json_scalar(kiln_text *text, kiln_val value, int *status) {
    if (KILN_VAL_IS_NUMBER(value)) {
        double number;
        char spelled[32];
        size_t written;
        memcpy(&number, &value, sizeof number);
        /* NaN and both infinities have no JSON spelling and serialise as null, which is the same
         * rule -- and the same formatter for everything else -- the general walk applies. */
        if (KILN_UNLIKELY(!isfinite(number))) {
            *status = kiln_c99_text_append(text, "null", 4);
            return 1;
        }
        written = kiln_c99_double_to_js(number, spelled);
        *status = kiln_c99_text_append(text, spelled, written);
        return 1;
    }
    if (KILN_VAL_IS_STRING(value)) {
        const kiln_string *string = (const kiln_string *)KILN_VAL_PTR(value);
        *status = string == NULL ? kiln_c99_json_quote(text, "", 0)
                                 : (!kiln_string_is_utf16(string)
                                        ? kiln_c99_json_quote(text, (const char *)kiln_string_latin1(string), string->unit_len)
                                        : kiln_c99_refuse("JSON.stringify on UTF-16 string"));
        return 1;
    }
    if (value == KILN_VAL_NULL) {
        *status = kiln_c99_text_append(text, "null", 4);
        return 1;
    }
    if (value == KILN_VAL_TRUE) {
        *status = kiln_c99_text_append(text, "true", 4);
        return 1;
    }
    if (value == KILN_VAL_FALSE) {
        *status = kiln_c99_text_append(text, "false", 5);
        return 1;
    }
    return 0;
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

/* The nested-indent builder is only ever allocated for the pretty form; the compact form leaves
 * it null from top to bottom, and every one of the walk's exits was calling `free` on that null
 * through the dynamic linker's stub -- once per container per document, for nothing. */
static void kiln_c99_json_drop_nested(kiln_text *nested) {
    if (nested->bytes != NULL) free(nested->bytes);
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
        /* NaN and both infinities have no JSON spelling, so they serialise as null; every other
         * number is JSON's number grammar and `Number::toString`'s at once, so the one formatter
         * answers both.
         *
         * It carries the integer fast path this case used to hold separately -- a counter, an id,
         * an index, or (as `String(-0)` is "0") a negative zero -- and measured on
         * `bench/non-io/json.mjs`, whose documents are four integers and three fractions each,
         * the two spell at the same speed: 40.4 ms against 40.5 ms, interleaved, on 2026-09-07.
         * One formatter for the whole grammar is the version worth keeping at that price. */
        double number = kiln_c99_as_number(value);
        char spelled[32];
        size_t written;
        if (!isfinite(number)) {
            return kiln_c99_text_append_c(text, "null");
        }
        written = kiln_c99_double_to_js(number, spelled);
        return kiln_c99_text_append(text, spelled, written);
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
        kiln_c99_json_drop_nested(&nested);
        return KILN_C99_OUT_OF_MEMORY;
    }

#define KILN_JSON_OPEN(bracket)                                                                   \
    do {                                                                                          \
        if (kiln_c99_text_append(text, bracket, 1) != KILN_C99_OK) {                               \
            kiln_c99_json_drop_nested(&nested);                                                   \
            return KILN_C99_OUT_OF_MEMORY;                                                          \
        }                                                                                          \
    } while (0)
#define KILN_JSON_BREAK(first)                                                                    \
    do {                                                                                          \
        if (!(first) && kiln_c99_text_append(text, ",", 1) != KILN_C99_OK) {                       \
            kiln_c99_json_drop_nested(&nested);                                                   \
            return KILN_C99_OUT_OF_MEMORY;                                                          \
        }                                                                                          \
        if (indent_len != 0 &&                                                                     \
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||                                  \
             kiln_c99_text_append(text, nested.bytes, nested.len) != KILN_C99_OK)) {                \
            kiln_c99_json_drop_nested(&nested);                                                   \
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
                kiln_c99_json_drop_nested(&nested);
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
            if (kiln_c99_json_scalar(text, array->elements[index], &status)) {
                if (status != KILN_C99_OK) {
                    kiln_c99_json_drop_nested(&nested);
                    return status;
                }
                continue;
            }
            status = kiln_c99_json_value(host, text, kiln_val_to_boundary(array->elements[index]), indent, indent_len,
                                         nested.bytes, nested.len, &wrote, depth, &frame, thrown);
            if (status != KILN_C99_OK) {
                kiln_c99_json_drop_nested(&nested);
                return status;
            }
            /* An element with no JSON spelling -- undefined, a function, a hole -- becomes null
             * rather than disappearing: an array's positions are part of its meaning. */
            if (!wrote && kiln_c99_text_append_c(text, "null") != KILN_C99_OK) {
                kiln_c99_json_drop_nested(&nested);
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        if (indent_len != 0 && array->count != 0 &&
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||
             kiln_c99_text_append(text, current, current_len) != KILN_C99_OK)) {
            kiln_c99_json_drop_nested(&nested);
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_json_drop_nested(&nested);
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
            char digits[32];
            size_t written;
            int wrote;
            written = kiln_c99_double_to_js((double)index, digits);
            KILN_JSON_BREAK(index == 0);
            if (kiln_c99_json_quote(text, digits, written) != KILN_C99_OK ||
                (indent_len != 0 ? kiln_c99_text_append(text, ": ", 2) : kiln_c99_text_append(text, ":", 1)) != KILN_C99_OK) {
                kiln_c99_json_drop_nested(&nested);
                return KILN_C99_OUT_OF_MEMORY;
            }
            /* Through the same recursion an object member takes, so a `Float64Array` holding
             * NaN spells it `null` here for the same reason a plain number does. */
            status = kiln_c99_json_value(host, text, kiln_c99_number(kiln_c99_typed_get(view, index)),
                                         indent, indent_len, nested.bytes, nested.len, &wrote,
                                         depth, parents, thrown);
            if (status != KILN_C99_OK) {
                kiln_c99_json_drop_nested(&nested);
                return status;
            }
        }
        if (indent_len != 0 && view->length != 0 &&
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||
             kiln_c99_text_append(text, current, current_len) != KILN_C99_OK)) {
            kiln_c99_json_drop_nested(&nested);
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_json_drop_nested(&nested);
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
                kiln_c99_json_drop_nested(&nested);
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
                kiln_c99_json_drop_nested(&nested);
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
                kiln_c99_json_drop_nested(&nested);
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
                kiln_c99_json_drop_nested(&nested);
                return status;
            }
            KILN_JSON_BREAK(written == 0);
            if (!is_array && (kiln_c99_json_quote(text, key.ptr, key.len) != KILN_C99_OK ||
                              (indent_len != 0 ? kiln_c99_text_append(text, ": ", 2) : kiln_c99_text_append(text, ":", 1)) !=
                                  KILN_C99_OK)) {
                kiln_c99_json_drop_nested(&nested);
                return KILN_C99_OUT_OF_MEMORY;
            }
            status = kiln_c99_json_value(host, text, member, indent, indent_len, nested.bytes,
                                         nested.len, &wrote, depth, &frame, thrown);
            if (status != KILN_C99_OK) {
                kiln_c99_json_drop_nested(&nested);
                return status;
            }
            if (!wrote) {
                if (!is_array) {
                    text->len = mark;
                    continue;
                }
                if (kiln_c99_text_append_c(text, "null") != KILN_C99_OK) {
                    kiln_c99_json_drop_nested(&nested);
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            written += 1;
        }
        if (indent_len != 0 && written != 0 &&
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||
             kiln_c99_text_append(text, current, current_len) != KILN_C99_OK)) {
            kiln_c99_json_drop_nested(&nested);
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_json_drop_nested(&nested);
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
                kiln_c99_json_drop_nested(&nested);
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
                    size_t nlen;
                    kiln_value member;
                    size_t mark;
                    int wrote;
                    if (object->elements[index] == KILN_VAL_HOLE) continue;
                    nlen = kiln_c99_double_to_js((double)index, num_buf);
                    mark = text->len;
                    KILN_JSON_BREAK(written == 0);
                    if (kiln_c99_json_quote(text, num_buf, nlen) != KILN_C99_OK ||
                        (indent_len != 0 ? kiln_c99_text_append(text, ": ", 2) : kiln_c99_text_append(text, ":", 1)) != KILN_C99_OK) {
                        kiln_c99_json_drop_nested(&nested);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    if (kiln_c99_json_scalar(text, object->elements[index], &status)) {
                        if (status != KILN_C99_OK) {
                            kiln_c99_json_drop_nested(&nested);
                            return status;
                        }
                        written += 1;
                        continue;
                    }
                    member = kiln_val_to_boundary(object->elements[index]);
                    status = kiln_c99_json_value(host, text, member, indent, indent_len,
                                                 nested.bytes, nested.len, &wrote, depth, &frame, thrown);
                    if (status != KILN_C99_OK) {
                        kiln_c99_json_drop_nested(&nested);
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
                size_t key_len = kiln_c99_key_len(skey);
                kiln_val held = (object->slots != NULL && index < object->slot_capacity)
                                    ? object->slots[index]
                                    : KILN_VAL_UNDEFINED;
                kiln_value member;
                size_t mark;
                int wrote;
                if (key_len >= 2 && skey[0] == '@' && skey[1] == '@') {
                    continue;
                }
                mark = text->len;
                KILN_JSON_BREAK(written == 0);
                if (kiln_c99_json_quote(text, skey, key_len) != KILN_C99_OK ||
                    (indent_len != 0 ? kiln_c99_text_append(text, ": ", 2) : kiln_c99_text_append(text, ":", 1)) != KILN_C99_OK) {
                    kiln_c99_json_drop_nested(&nested);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                if (kiln_c99_json_scalar(text, held, &status)) {
                    if (status != KILN_C99_OK) {
                        kiln_c99_json_drop_nested(&nested);
                        return status;
                    }
                    written += 1;
                    continue;
                }
                member = kiln_val_to_boundary(held);
                status = kiln_c99_json_value(host, text, member, indent, indent_len,
                                             nested.bytes, nested.len, &wrote, depth, &frame, thrown);
                if (status != KILN_C99_OK) {
                    kiln_c99_json_drop_nested(&nested);
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
                    kiln_c99_json_drop_nested(&nested);
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
                kiln_c99_json_drop_nested(&nested);
                return KILN_C99_OUT_OF_MEMORY;
            }
            status = kiln_c99_json_value(host, text, member, indent, indent_len, nested.bytes,
                                         nested.len, &wrote, depth, &frame, thrown);
            if (status != KILN_C99_OK) {
                kiln_c99_json_drop_nested(&nested);
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
            kiln_c99_json_drop_nested(&nested);
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_json_drop_nested(&nested);
        return kiln_c99_text_append_c(text, "}");
    }
#undef KILN_JSON_OPEN
#undef KILN_JSON_BREAK
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
static int kiln_c99_json_define(kiln_value built, const char *ikey, kiln_val member) {
    kiln_object *object = kiln_c99_object_of(built);
    uint32_t slot;
    kiln_shape *next;
    if (object->shape != NULL) {
        if (kiln_c99_shape_find_slot(object->shape, ikey, &slot) && slot < object->slot_capacity) {
            object->slots[slot] = member;
            return KILN_C99_OK;
        }
        int is_double = KILN_VAL_IS_NUMBER(member);
        if (kiln_c99_shape_transition(object->shape, ikey, is_double, &next)) {
            if ((size_t)next->slot_index >= object->slot_capacity) {
                int grown = kiln_c99_grow_slots(object, (size_t)next->slot_index + 1);
                if (grown != KILN_C99_OK) {
                    return grown;
                }
            }
            object->slots[next->slot_index] = member;
            object->shape = next;
            return KILN_C99_OK;
        }
    }
    return kiln_c99_put(NULL, built, ikey, kiln_c99_key_len(ikey), kiln_val_to_boundary(member), 0,
                        NULL);
}

/* Building `JSON.parse`'s answer from the tape the linked reader produced.
 *
 * The grammar is not here. It is in `crates/kiln-json`, linked from the provider archive and
 * walked by the interpreter too, so a document parses the same on both engines by construction
 * rather than by testing (ADR 0025). What the reader hands back is a tape: the document's nodes
 * in source order, a container carrying its child count, a string an offset and length into the
 * decoded text, a number its double. Every escape, surrogate half and overflowing number has
 * already been decided by then. This walk does only what has to touch this heap -- make the
 * string, the array or the object, and define the property -- over an explicit stack, so a
 * document's nesting is bounded by its length rather than by a call depth.
 *
 * An object's key never becomes a string of this heap: it is interned straight from the tape's
 * text, and `kiln_c99_json_define` above takes the interned key. That is the same economy the
 * reader this replaced had found, and the same rule -- a key spelled `__proto__` is data here,
 * where `kiln_c99_put` would relink the prototype. */

typedef struct kiln_json_build_frame {
    kiln_value container;
    /* The key waiting for its value, in an object: interned while the frame is on the machine
     * stack, where a pointer keeps the interned bytes alive through the collector's conservative
     * scan, and a string of this heap once the frames have spilled, where only a traced hold
     * can. Exactly one of the two is set while `has_key`.
     *
     * The spilled key is a `kiln_val` and not a boundary `kiln_value` because only the former
     * names a block. A boundary string is bytes and a length with no heap identity, so mirroring
     * one meant `kiln_val_from_boundary` allocating a *second* block and holding that: the block
     * this frame actually points into stayed unrooted, was swept, and came back as a later key's
     * bytes. */
    const char *ikey;
    kiln_val key;
    /* Elements, or key/value pairs, still to arrive. */
    size_t remaining;
    int is_object;
    int has_key;
} kiln_json_build_frame;

/* How many open containers the walk keeps on the machine stack before spilling to the heap. */
#define KILN_JSON_BUILD_LOCAL_FRAMES 32

/* The collector may run on any allocation this walk makes, and it finds its roots by scanning
 * the machine stack. So the frames live in a local array while the document is ordinarily deep,
 * which is nearly always, and what they hold is traced for free. Past that the frames move to the
 * heap, which the collector never looks at, and from then on every container and pending key is
 * mirrored into `holds`, an array of this heap that a local variable keeps alive. */
typedef struct kiln_json_build_stack {
    kiln_json_build_frame local[KILN_JSON_BUILD_LOCAL_FRAMES];
    kiln_json_build_frame *frames;
    size_t depth;
    size_t capacity;
    kiln_value holds;
    int spilled;
} kiln_json_build_stack;

/* Root `value` for as long as the frames are off the machine stack. The value arrives as a
 * `kiln_val`, and is stored as it arrived: converting a boundary string here would allocate a
 * fresh block and root that instead of the one the frame points into, which is exactly the
 * use-after-free this walk had. */
static int kiln_c99_json_build_mirror(kiln_json_build_stack *stack, size_t index, kiln_val value,
                                      int is_key) {
    kiln_array *held;
    size_t slot = 2 * index + (is_key ? 1 : 0);
    if (!stack->spilled) {
        return KILN_C99_OK;
    }
    held = kiln_c99_array_of(stack->holds);
    while (held->count <= slot) {
        int status = kiln_c99_array_push(stack->holds, kiln_c99_undefined());
        if (status != KILN_C99_OK) {
            return status;
        }
        /* Each push may replace the elements block, so the array is read again rather than
         * through a pointer taken before it grew. */
        held = kiln_c99_array_of(stack->holds);
    }
    held->elements[slot] = value;
    return KILN_C99_OK;
}

/* A pending key that was interned must become a string of this heap before its frame leaves the
 * machine stack: the interned bytes are collector-owned and unmarked until a shape holds them. */
static int kiln_c99_json_build_materialize_key(kiln_json_build_stack *stack, size_t index) {
    kiln_json_build_frame *frame = &stack->frames[index];
    if (!frame->has_key || frame->ikey == NULL) {
        return KILN_C99_OK;
    }
    {
        size_t len = kiln_c99_key_len(frame->ikey);
        kiln_val key = kiln_c99_val_from_text(frame->ikey, len);
        if (len != 0 && key == KILN_VAL_UNDEFINED) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        frame->key = key;
    }
    frame->ikey = NULL;
    return kiln_c99_json_build_mirror(stack, index, frame->key, 1);
}

static int kiln_c99_json_build_open(kiln_json_build_stack *stack, kiln_value container,
                                    size_t remaining, int is_object) {
    kiln_json_build_frame *frame;
    if (stack->depth == stack->capacity) {
        size_t grown = stack->capacity * 2;
        kiln_json_build_frame *bigger;
        size_t index;
        if (!stack->spilled) {
            /* Leaving the machine stack: from here on the collector cannot see the frames, so
             * everything already open is mirrored into an array it can. */
            int status = kiln_c99_new_array_with_capacity(&stack->holds, 2 * grown, 0);
            if (status != KILN_C99_OK) {
                return status;
            }
            stack->spilled = 1;
            for (index = 0; index < stack->depth; ++index) {
                status = kiln_c99_json_build_mirror(stack, index,
                                                    kiln_val_from_boundary(stack->frames[index].container), 0);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_json_build_materialize_key(stack, index);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            bigger = (kiln_json_build_frame *)malloc(grown * sizeof *bigger);
            if (bigger != NULL) {
                memcpy(bigger, stack->frames, stack->depth * sizeof *bigger);
            }
        } else {
            bigger = (kiln_json_build_frame *)realloc(stack->frames, grown * sizeof *bigger);
        }
        if (bigger == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        stack->frames = bigger;
        stack->capacity = grown;
    }
    if (stack->spilled) {
        int status =
            kiln_c99_json_build_mirror(stack, stack->depth, kiln_val_from_boundary(container), 0);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    frame = &stack->frames[stack->depth];
    frame->container = container;
    frame->ikey = NULL;
    frame->key = KILN_VAL_UNDEFINED;
    frame->remaining = remaining;
    frame->is_object = is_object;
    frame->has_key = 0;
    stack->depth += 1;
    return KILN_C99_OK;
}

static void kiln_c99_json_build_release(kiln_json_build_stack *stack) {
    if (stack->spilled) {
        free(stack->frames);
        kiln_c99_array_of(stack->holds)->count = 0;
    }
}

/* A parsed value travels this walk as a `kiln_val` -- the heap's own representation -- and not
 * as a boundary `kiln_value`.
 *
 * The two are not the same amount of work for a string. A boundary string is a bare
 * pointer-and-length with no heap identity, so `kiln_val_from_boundary` has no choice but to
 * allocate a block and copy the bytes into it -- which meant a parsed string was allocated and
 * copied *twice*, once by `kiln_c99_copy_text` here and once again by the `define` or `push` that
 * took it, each with its own ASCII scan over the bytes. Building the `kiln_val` once and handing
 * it to `kiln_c99_json_define` and `kiln_c99_array_push_val` leaves one allocation per string.
 *
 * The collector sees a `kiln_val` on the machine stack exactly as it saw the `kiln_value` it
 * replaces: `kiln_c99_gc_mark_word` tests a word both as a raw address and as a boxed pointer, so
 * a tagged string, array or object in a local is a root either way. */
static int kiln_c99_json_build(const kiln_json_node *nodes, size_t count,
                               const unsigned char *text, size_t text_len, kiln_val *out) {
    kiln_json_build_stack stack;
    kiln_val root = KILN_VAL_UNDEFINED;
    int have_root = 0;
    size_t index;
    stack.frames = stack.local;
    stack.depth = 0;
    stack.capacity = KILN_JSON_BUILD_LOCAL_FRAMES;
    stack.holds = kiln_c99_undefined();
    stack.spilled = 0;
    for (index = 0; index < count; ++index) {
        const kiln_json_node *node = &nodes[index];
        kiln_json_build_frame *top = stack.depth == 0 ? NULL : &stack.frames[stack.depth - 1];
        kiln_val value = KILN_VAL_UNDEFINED;
        kiln_value container = kiln_c99_undefined();
        int status = KILN_C99_OK;
        int opened = 0;
        if (have_root) {
            /* The root closed and the tape goes on: not a tape the reader produces. */
            kiln_c99_json_build_release(&stack);
            KILN_C99_REFUSE_INTERNAL("JSON.parse, which turns it into a SyntaxError the program can catch");
        }
        if (node->tag == KILN_JSON_STRING &&
            (node->value > (uint64_t)text_len ||
             (size_t)node->len > text_len - (size_t)node->value)) {
            /* The offsets crossed an ABI; a lie there is a refusal, never a read past the text. */
            kiln_c99_json_build_release(&stack);
            KILN_C99_REFUSE_INTERNAL("JSON.parse, which turns it into a SyntaxError the program can catch");
        }
        if (top != NULL && top->is_object && !top->has_key) {
            /* An object's key, taken from the tape's text without a string of this heap. */
            if (node->tag != KILN_JSON_STRING) {
                kiln_c99_json_build_release(&stack);
                KILN_C99_REFUSE_INTERNAL("JSON.parse, which turns it into a SyntaxError the program can catch");
            }
            if (stack.spilled) {
                top->key = kiln_c99_val_from_text((const char *)text + (size_t)node->value,
                                                  (size_t)node->len);
                if (node->len != 0 && top->key == KILN_VAL_UNDEFINED) {
                    status = KILN_C99_OUT_OF_MEMORY;
                } else {
                    status = kiln_c99_json_build_mirror(&stack, stack.depth - 1, top->key, 1);
                }
            } else {
                top->ikey = kiln_c99_intern((const char *)text + (size_t)node->value,
                                            (size_t)node->len);
                if (top->ikey == NULL) {
                    status = KILN_C99_OUT_OF_MEMORY;
                }
            }
            if (status != KILN_C99_OK) {
                kiln_c99_json_build_release(&stack);
                return status;
            }
            top->has_key = 1;
            continue;
        }
        switch (node->tag) {
        case KILN_JSON_NULL:
            value = KILN_VAL_NULL;
            break;
        case KILN_JSON_FALSE:
            value = KILN_VAL_FALSE;
            break;
        case KILN_JSON_TRUE:
            value = KILN_VAL_TRUE;
            break;
        case KILN_JSON_NUMBER: {
            double number;
            memcpy(&number, &node->value, sizeof number);
            value = kiln_c99_val_from_double(number);
            break;
        }
        case KILN_JSON_STRING:
            value = kiln_c99_val_from_text((const char *)text + (size_t)node->value,
                                           (size_t)node->len);
            break;
        case KILN_JSON_ARRAY:
            /* The count is known up front, so the array is allocated once at its final size. */
            status = kiln_c99_new_array_with_capacity(&container, (size_t)node->len, 1);
            value = kiln_val_from_boundary(container);
            opened = node->len != 0;
            break;
        case KILN_JSON_OBJECT:
            status = kiln_c99_new_object(&container);
            value = kiln_val_from_boundary(container);
            opened = node->len != 0;
            break;
        default:
            kiln_c99_json_build_release(&stack);
            KILN_C99_REFUSE_INTERNAL("JSON.parse, which turns it into a SyntaxError the program can catch");
        }
        if (status == KILN_C99_OK && opened) {
            status = kiln_c99_json_build_open(&stack, container, (size_t)node->len,
                                              node->tag == KILN_JSON_OBJECT);
        }
        if (status != KILN_C99_OK) {
            kiln_c99_json_build_release(&stack);
            return status;
        }
        if (opened) {
            continue;
        }
        /* Deliver the value to the container it belongs to, and every container that completes
         * with it to the one above. */
        for (;;) {
            kiln_json_build_frame *frame;
            if (stack.depth == 0) {
                root = value;
                have_root = 1;
                break;
            }
            frame = &stack.frames[stack.depth - 1];
            if (frame->is_object) {
                /* A repeated key overwrites in place, which keeps the first position and takes the
                 * last value; `__proto__` is an own property; both are `kiln_c99_json_define`'s
                 * doing. A spilled frame's key is a string, interned here at the last moment. */
                const char *ikey = frame->ikey;
                if (ikey == NULL) {
                    /* Free for a string: the bytes and length are read out of the block. */
                    kiln_value spelling = kiln_val_to_boundary(frame->key);
                    ikey = kiln_c99_intern(spelling.ptr, spelling.len);
                    if (ikey == NULL) {
                        kiln_c99_json_build_release(&stack);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
                status = kiln_c99_json_define(frame->container, ikey, value);
                frame->ikey = NULL;
                frame->key = KILN_VAL_UNDEFINED;
                frame->has_key = 0;
            } else {
                status = kiln_c99_array_push_val(frame->container, value);
            }
            if (status != KILN_C99_OK) {
                kiln_c99_json_build_release(&stack);
                return status;
            }
            frame->remaining -= 1;
            if (frame->remaining != 0) {
                break;
            }
            value = kiln_val_from_boundary(frame->container);
            stack.depth -= 1;
        }
    }
    kiln_c99_json_build_release(&stack);
    if (!have_root || stack.depth != 0) {
        KILN_C99_REFUSE_INTERNAL("JSON.parse, which turns it into a SyntaxError the program can catch");
    }
    *out = root;
    return KILN_C99_OK;
}

/* Parse one complete JSON text. `JSON.parse` and a data module's materialization both come here;
 * the reader's refusal becomes the SyntaxError, and the reader has already rejected trailing
 * text, so `JSON.parse("1 2")` has no answer here either. The tape is lent from the reader's
 * per-thread scratch and is valid until the next parse on this thread, which is after the walk:
 * the walk allocates on this heap and never parses. */
static int kiln_c99_json_document(const char *bytes, size_t len, kiln_val *out) {
    const kiln_json_node *nodes = NULL;
    size_t count = 0;
    const unsigned char *text = NULL;
    size_t text_len = 0;
    int32_t parsed =
        kiln_json_parse((const unsigned char *)bytes, len, &nodes, &count, &text, &text_len);
    if (parsed == -1) {
        KILN_C99_REFUSE_INTERNAL("JSON.parse, which turns it into a SyntaxError the program can catch");
    }
    if (parsed != 0) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    return kiln_c99_json_build(nodes, count, text, text_len, out);
}
