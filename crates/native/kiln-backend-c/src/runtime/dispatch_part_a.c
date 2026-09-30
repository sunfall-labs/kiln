
/* Argument coercion metadata is generated from the Rust method registry in prelude.rs.
 * Dispatch reads groups and both coercion masks together so this hot path has one descriptor
 * lookup and unknown codes retain the all-zero refusal behavior. */

/* Whether a numeric argument still has to go through `ToNumber` to become one.
 *
 * `undefined` is left alone: it is not a missing number but a *meaningful* one, and each built-in
 * says for itself what it means -- `slice(1, undefined)` runs to the end, where
 * `ToNumber(undefined)` is NaN. A symbol and a BigInt are converted rather than skipped, because
 * converting either is the `TypeError` the language requires and the pure `kiln_c99_as_number`
 * answered an ordinary number for both. */
static int kiln_c99_needs_number_conversion(kiln_value value) {
    if (value.tag == KILN_VALUE_UNDEFINED || value.tag == KILN_VALUE_NUMBER) {
        return 0;
    }
    if (value.tag == KILN_VALUE_SYMBOL || value.tag == KILN_VALUE_BIGINT) {
        return 1;
    }
    /* Every remaining primitive spells its own number; everything else is a call away from one. */
    return !(value.tag == KILN_VALUE_NULL || value.tag == KILN_VALUE_BOOLEAN ||
             value.tag == KILN_VALUE_STRING);
}

/* Whether a textual argument still has to go through `ToString`.
 *
 * A symbol, because `ToString` is the step that refuses one; and anything that is not a primitive,
 * because the conversion is a method the program may have written. */
static int kiln_c99_needs_text_conversion(kiln_value value) {
    if (value.tag == KILN_VALUE_SYMBOL) {
        return 1;
    }
    return !(value.tag == KILN_VALUE_UNDEFINED || value.tag == KILN_VALUE_NULL ||
             value.tag == KILN_VALUE_BOOLEAN || value.tag == KILN_VALUE_NUMBER ||
             value.tag == KILN_VALUE_STRING || value.tag == KILN_VALUE_BIGINT);
}

/* `ToNumber` of one argument, answered as a value rather than as a double so that the built-in
 * behind it goes on reading its arguments exactly the way it already does. */
static int kiln_c99_to_number_argument(kiln_host *host, kiln_value value, kiln_value *out,
                                       int depth, kiln_value *thrown) {
    kiln_value reduced = value;
    if (kiln_c99_is_reference(value)) {
        int status = kiln_c99_to_primitive(host, value, 0, &reduced, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    if (reduced.tag == KILN_VALUE_SYMBOL) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot convert a Symbol value to a number");
    }
    if (reduced.tag == KILN_VALUE_BIGINT) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot convert a BigInt value to a number");
    }
    *out = kiln_c99_number(kiln_c99_as_number(reduced));
    return KILN_C99_OK;
}

/* `ToString` of one argument, on the same terms. */
static int kiln_c99_to_text_argument(kiln_host *host, kiln_value value, kiln_value *out,
                                     int depth, kiln_value *thrown) {
    kiln_value reduced = value;
    if (kiln_c99_is_reference(value)) {
        int status = kiln_c99_to_primitive(host, value, 1, &reduced, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    if (reduced.tag == KILN_VALUE_SYMBOL) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot convert a Symbol value to a string");
    }
    *out = kiln_c99_text(reduced);
    return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
}

/* JSON.stringify's string `space` argument is capped at ten UTF-16 code units before it crosses
 * the byte-oriented writer boundary. A cap in encoded bytes is wrong for non-ASCII text, and a
 * lossy codec would silently turn a truncated surrogate pair into a different indentation. Keep
 * the bounded bridge explicit: reject an ill-formed ten-unit prefix until the writer gains a
 * native UTF-16 output buffer. */
static int kiln_c99_json_indent_string(kiln_value value, kiln_text *out) {
    const kiln_string *source;
    kiln_string view;
    kiln_value view_value;
    uint8_t *encoded = NULL;
    size_t encoded_len = 0;
    size_t units;
    size_t index;
    int status;
    if (value.tag != KILN_VALUE_STRING || value.ptr == NULL || out == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    source = (const kiln_string *)value.ptr;
    if (((source->flags & ~(KILN_STRING_LATIN1 | KILN_STRING_UTF16 | KILN_STRING_SLICED)) != 0) ||
        (source->unit_len != 0 && source->data == NULL) ||
        (kiln_string_is_utf16(source) && source->data != NULL &&
         ((uintptr_t)source->data & (sizeof(uint16_t) - 1)) != 0)) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    units = source->unit_len > 10 ? 10 : source->unit_len;
    view = *source;
    view.unit_len = (uint32_t)units;
    if (units != 0 && source->data != NULL) {
        view.data = source->data;
    }
    if (kiln_string_is_utf16(&view)) {
        for (index = 0; index < units; ++index) {
            uint32_t unit = kiln_string_unit_at(&view, index);
            if (unit >= 0xD800 && unit <= 0xDBFF) {
                if (index + 1 >= units) return KILN_C99_INVALID_ARGUMENT;
                unit = kiln_string_unit_at(&view, ++index);
                if (unit < 0xDC00 || unit > 0xDFFF) return KILN_C99_INVALID_ARGUMENT;
            } else if (unit >= 0xDC00 && unit <= 0xDFFF) {
                return KILN_C99_INVALID_ARGUMENT;
            }
        }
    }
    view_value = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)&view, units);
    status = kiln_c99_string_to_utf8_lossy(view_value, &encoded, &encoded_len);
    if (status == KILN_C99_OK) status = kiln_c99_text_append(out, (const char *)encoded, encoded_len);
    free(encoded);
    return status;
}

static int kiln_c99_data_view_write_bits(kiln_host *host, uint64_t code, kiln_value value,
                                         uint64_t *out, int depth, kiln_value *thrown);

/* `DataView.prototype.getInt16(offset[, littleEndian])` and its nineteen siblings.
 *
 * One handler rather than twenty, because the twenty differ only in a width, how the bytes are
 * read back as a value, and which direction the transfer goes. The three questions are asked
 * separately below and each has exactly one answer per code.
 *
 * THE BYTE-ORDER ARGUMENT DEFAULTS TO BIG-ENDIAN -- the opposite of every typed array and of
 * every platform this backend runs on. That is the specification's choice and the whole reason
 * the type exists: a DataView reads bytes that came from somewhere else, and somewhere else
 * usually means network order. It is also the detail most likely to be got backwards, because a
 * program that writes and reads through the same view never notices.
 *
 * An access that does not fit entirely inside the window is a RangeError, not a clamp and not a
 * NaN. A typed array's out-of-range *index* reads `undefined` because an index is a property; a
 * DataView's offset is an argument, and the language throws on it. */
static int kiln_c99_data_view_method(kiln_host *host, kiln_value *result, uint64_t code,
                                     kiln_value receiver, const kiln_value *args, size_t argc,
                                     int depth, kiln_value *thrown) {
    kiln_data_view *view;
    kiln_value value;
    size_t width;
    size_t offset = 0;
    size_t flag;
    int little_endian;
    int is_set = code >= KILN_C99_METHOD_DATA_VIEW_SET_INT8;
    uint8_t *bytes = NULL;
    uint64_t bits = 0;
    int status;
    if (receiver.tag != KILN_VALUE_DATA_VIEW) {
        /* The interpreter's wording for `NotADataView`, character for character: `error.message`
         * is program-visible, so two spellings of one fault are a divergence like any other. */
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Method called on an incompatible receiver");
    }
    view = kiln_c99_data_view_of(receiver);
    switch (code) {
    case KILN_C99_METHOD_DATA_VIEW_GET_INT8:
    case KILN_C99_METHOD_DATA_VIEW_GET_UINT8:
    case KILN_C99_METHOD_DATA_VIEW_SET_INT8:
    case KILN_C99_METHOD_DATA_VIEW_SET_UINT8: width = 1; break;
    case KILN_C99_METHOD_DATA_VIEW_GET_INT16:
    case KILN_C99_METHOD_DATA_VIEW_GET_UINT16:
    case KILN_C99_METHOD_DATA_VIEW_SET_INT16:
    case KILN_C99_METHOD_DATA_VIEW_SET_UINT16: width = 2; break;
    case KILN_C99_METHOD_DATA_VIEW_GET_INT32:
    case KILN_C99_METHOD_DATA_VIEW_GET_UINT32:
    case KILN_C99_METHOD_DATA_VIEW_GET_FLOAT32:
    case KILN_C99_METHOD_DATA_VIEW_SET_INT32:
    case KILN_C99_METHOD_DATA_VIEW_SET_UINT32:
    case KILN_C99_METHOD_DATA_VIEW_SET_FLOAT32: width = 4; break;
    /* The two float64 accessors and the four BigInt ones, which are the whole of the rest. */
    default: width = 8; break;
    }
    /* `ToIndex`, which is where a negative offset stops: `view.getUint8(-1)` is a RangeError
     * about the argument rather than a read of some byte in front of the window. Shared with the
     * typed-array constructors so the two cannot come to word it differently. */
    status = kiln_c99_to_index(argc > 0 ? args[0] : kiln_c99_undefined(), &offset, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* The flag sits after the value on a `set` and immediately after the offset on a `get`, which
     * is the only place the two shapes differ. Absent is false, and false is big-endian. */
    flag = is_set ? 2 : 1;
    if (is_set) {
        /* The value is converted here: after the offset and *before* the bounds are checked,
         * which is where the specification puts it and where the order is observable. A write
         * that could never have landed still runs the conversion, so `view.setBigInt64(999, 1)`
         * is the TypeError about the number rather than the RangeError about the offset.
         *
         * The eight numeric setters arrive already converted -- the generated numeric mask
         * names argument one for them, and that pass is the one with a machine to run a
         * `valueOf` with. Only the BigInt pair is left to do here, because `ToNumber` is a
         * TypeError on the one type those two accept. */
        value = argc > 1 ? args[1] : kiln_c99_undefined();
        status = kiln_c99_data_view_write_bits(host, code, value, &bits, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    little_endian = argc > flag && kiln_c99_truthy(args[flag]);
    if (view->buffer->detached) {
        /* A TypeError, not a RangeError: no offset is in bounds of bytes that are gone. */
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot perform DataView access on a detached ArrayBuffer");
    }
    if (!kiln_c99_data_view_at(view, offset, width, &bytes)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Offset is outside the bounds of the DataView");
    }
    if (!is_set) {
        bits = kiln_c99_data_view_gather(bytes, width, little_endian);
        switch (code) {
        /* Each narrowing is spelled the way the neighbouring `kiln_c99_typed_get` spells it, so a
         * buffer written through a typed array and read through a view agrees with itself. */
        case KILN_C99_METHOD_DATA_VIEW_GET_INT8:
            *result = kiln_c99_number((double)(int8_t)(uint8_t)bits);
            return KILN_C99_OK;
        case KILN_C99_METHOD_DATA_VIEW_GET_UINT8:
            *result = kiln_c99_number((double)(uint8_t)bits);
            return KILN_C99_OK;
        case KILN_C99_METHOD_DATA_VIEW_GET_INT16:
            *result = kiln_c99_number((double)(int16_t)(uint16_t)bits);
            return KILN_C99_OK;
        case KILN_C99_METHOD_DATA_VIEW_GET_UINT16:
            *result = kiln_c99_number((double)(uint16_t)bits);
            return KILN_C99_OK;
        case KILN_C99_METHOD_DATA_VIEW_GET_INT32: {
            uint32_t raw = (uint32_t)bits;
            int32_t narrowed;
            memcpy(&narrowed, &raw, sizeof narrowed);
            *result = kiln_c99_number((double)narrowed);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_DATA_VIEW_GET_UINT32:
            *result = kiln_c99_number((double)(uint32_t)bits);
            return KILN_C99_OK;
        case KILN_C99_METHOD_DATA_VIEW_GET_FLOAT32: {
            uint32_t raw = (uint32_t)bits;
            float narrowed;
            memcpy(&narrowed, &raw, sizeof narrowed);
            *result = kiln_c99_number((double)narrowed);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_DATA_VIEW_GET_FLOAT64: {
            double wide;
            memcpy(&wide, &bits, sizeof wide);
            *result = kiln_c99_number(wide);
            return KILN_C99_OK;
        }
        default: {
            /* The BigInt pair. Rendered as decimal and parsed back, which is exact for all
             * sixty-four bits and uses only what the provider already exports:
             * `kiln_bigint_from_double` rounds everything above 2^53, and rounding is precisely
             * what these two accessors exist to avoid. */
            char digits[24];
            size_t len;
            void *handle = NULL;
            int negative = code == KILN_C99_METHOD_DATA_VIEW_GET_BIG_INT64 &&
                           (bits & UINT64_C(0x8000000000000000)) != 0;
            /* The magnitude without ever forming the negative: `-x` has no representation at the
             * minimum signed value, and this does. */
            uint64_t magnitude = negative ? (~bits + UINT64_C(1)) : bits;
            if (negative) {
                digits[0] = '-';
                len = 1 + kiln_c99_uint64_digits(magnitude, digits + 1);
            } else {
                len = kiln_c99_uint64_digits(magnitude, digits);
            }
            if (kiln_bigint_parse((const unsigned char *)digits, len, 10, &handle) != 0) {
                KILN_C99_REFUSE("a sixty-four bit integer the BigInt provider would not parse");
            }
            *result = kiln_c99_bigint_value(handle);
            return KILN_C99_OK;
        }
        }
    }
    kiln_c99_data_view_scatter(bytes, width, little_endian, bits);
    *result = kiln_c99_undefined();
    return KILN_C99_OK;
}

/* `ToBigInt` and then `BigInt.asUintN(64, _)`: the sixty-four bits a `setBigInt64`,
 * `setBigUint64` or a BigInt64Array/BigUint64Array element store writes. */
static int kiln_c99_to_bigint64_bits(kiln_host *host, kiln_value value, uint64_t *out, int depth,
                                     kiln_value *thrown) {
    int status;
    /* `ToBigInt`, which accepts far less than `ToNumber` does: a BigInt, a boolean and a
     * string that spells an integer, and a *number* not at all. `setBigInt64(0, 1)` is a
     * TypeError rather than a one, because an implicit crossing between the two numeric
     * towers is precisely what having two of them is meant to prevent.
     *
     * Reduced to a primitive here rather than by the coercion pass on the way in: that pass
     * runs `ToNumber`, which is a TypeError on the one type these two accessors want. */
    const void *mask = kiln_c99_bigint_uint64_mask();
    void *wrapped = NULL;
    void *converted = NULL;
    int64_t high = 0;
    uint64_t low = 0;
    if (kiln_c99_is_reference(value)) {
        status = kiln_c99_to_primitive(host, value, 0, &value, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    if (value.tag == KILN_VALUE_BOOLEAN) {
        /* `1n` and `0n`. Spelled through the parser because that is the only way this
         * backend makes a BigInt out of nothing, and `kiln_bigint_from_double` is refused
         * territory: it is the rounding door. */
        static const unsigned char zero_one[] = "01";
        if (kiln_bigint_parse(zero_one + (value.bits != 0 ? 1 : 0), 1, 10, &converted) != 0) {
            KILN_C99_REFUSE("a boolean the BigInt provider would not parse");
        }
        value = kiln_c99_bigint_value(converted);
    } else if (value.tag == KILN_VALUE_STRING) {
        /* A string that is not an integer literal is a SyntaxError rather than a NaN, which
         * is the answer `BigInt("x")` gives for the same reason: this type has no NaN to
         * become. `kiln_bigint_parse` trims, so " 12 " is twelve. */
        size_t value_len = 0;
        const char *value_bytes = kiln_c99_string_ascii_bytes(value, &value_len);
        if (value_bytes == NULL || value_len == 0 ||
            kiln_bigint_parse((const unsigned char *)value_bytes, value_len, 10,
                              &converted) != 0) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                        "Cannot convert this string to a BigInt");
        }
        value = kiln_c99_bigint_value(converted);
    }
    if (value.tag != KILN_VALUE_BIGINT) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot convert this value to a BigInt");
    }
    /* Operation 7 is the provider's bitwise AND. `& (2n ** 64n - 1n)` is `BigInt.asUintN(64)`
     * spelled with what the provider exports, and the wrap is the language's own: the two
     * setters store the low sixty-four bits of *any* BigInt rather than refusing a wide one.
     * `kiln_bigint_to_i128` alone would have called `2n ** 200n` out of range; after the mask
     * the value is below 2^64 and always fits. */
    if (mask == NULL ||
        kiln_bigint_binary(7, (const void *)(uintptr_t)value.bits, mask, &wrapped) != 0 ||
        kiln_bigint_to_i128(wrapped, &high, &low) != 0 || high != 0) {
        /* `high != 0` is the mask having failed to do what its name says. Checked rather than
         * cast away, because the alternative to noticing is storing the wrong eight bytes. */
        KILN_C99_REFUSE("a BigInt that would not mask into sixty-four bits");
    }
    *out = low;
    return KILN_C99_OK;
}

/* The bytes one `set` accessor is being asked to store, as an unsigned integer of its width.
 *
 * Its own function because the conversion is the half with an order to get right, and the only
 * half that can call back into the program. */
static int kiln_c99_data_view_write_bits(kiln_host *host, uint64_t code, kiln_value value,
                                         uint64_t *out, int depth, kiln_value *thrown) {
    switch (code) {
    case KILN_C99_METHOD_DATA_VIEW_SET_BIG_INT64:
    case KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64:
        return kiln_c99_to_bigint64_bits(host, value, out, depth, thrown);
    case KILN_C99_METHOD_DATA_VIEW_SET_FLOAT32: {
        float narrowed = (float)kiln_c99_as_number(value);
        uint32_t raw;
        memcpy(&raw, &narrowed, sizeof raw);
        *out = raw;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_DATA_VIEW_SET_FLOAT64: {
        double wide = kiln_c99_as_number(value);
        uint64_t raw;
        memcpy(&raw, &wide, sizeof raw);
        *out = raw;
        return KILN_C99_OK;
    }
    default:
        /* The six integer setters. `ToUint32` and then a truncation to the width, which is what
         * `NumberToRawBytes` does for every integer element type -- and it is the conversion
         * `kiln_c99_typed_set` already uses, so `view.setUint8(0, 300)` and `u8[0] = 300` cannot
         * come out different. The truncation itself is `kiln_c99_data_view_scatter`, which keeps
         * only `width` bytes.
         *
         * The pure `kiln_c99_as_number` is safe here only because the coercion pass has already
         * run: by this point argument one is a Number, or the call threw on the way in. */
        *out = (uint64_t)kiln_c99_to_uint32(kiln_c99_as_number(value));
        return KILN_C99_OK;
    }
}

/* How many arguments a call may carry before the coercion pass has to borrow memory for its
 * copy. Nothing in either table names a position above the third; the room is for the *tail*,
 * which a program is free to make as long as it likes. */
#define KILN_C99_COERCED_INLINE 8

/* `ToNumber` and `ToString` for every argument a built-in reads as one, run once on the way in.
 *
 * The same place and the same reason as the interpreter's pass: these conversions are the
 * program's own `Symbol.toPrimitive`, `valueOf` or `toString` -- calls, which can be counted and
 * can throw -- and the several dozen built-ins behind this function have no machine to run one
 * with. `kiln_c99_as_number` is pure, so every object was silently read as NaN and every
 * `valueOf` the program wrote was never called at all: `[1,2,3,4].slice({valueOf: () => 1})`
 * answered the whole array, and `"ab".repeat({valueOf() { throw x; }})` never threw.
 *
 * After the receiver, because that is the order a `valueOf` counting its own calls can observe,
 * and left to right, because two arguments that both throw must throw the leftmost first.
 *
 * The hand-on cannot loop: what it passes down is a Number or a String. */

/* ========================================================================= */
/* Timsort: Adaptive, stable sort with binary insertion sort & galloping   */
/* ========================================================================= */

typedef struct {
    kiln_host *host;
    kiln_value comparator;
    int depth;
    kiln_value *thrown;
} kiln_c99_sort_context;

static int kiln_c99_sort_compare_value(kiln_c99_sort_context *ctx, kiln_value a,
                                       kiln_value b, double *out_order) {
    if (kiln_c99_is_callable(ctx->comparator)) {
        kiln_value pair[2];
        kiln_value produced;
        pair[0] = a;
        pair[1] = b;
        int status = kiln_c99_call(ctx->host, &produced, ctx->comparator, kiln_c99_undefined(),
                                   pair, 2, ctx->depth, ctx->thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        double order = kiln_c99_as_number(produced);
        *out_order = isnan(order) ? 0.0 : order;
        return KILN_C99_OK;
    } else {
        kiln_value left_text = kiln_c99_text(a);
        kiln_value right_text = kiln_c99_text(b);
        if (left_text.tag != KILN_VALUE_STRING || right_text.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        *out_order = (double)kiln_c99_string_compare(left_text, right_text);
        return KILN_C99_OK;
    }
}

static int kiln_c99_sort_compare_val(kiln_c99_sort_context *ctx, kiln_val a,
                                     kiln_val b, double *out_order) {
    return kiln_c99_sort_compare_value(ctx, kiln_val_to_boundary(a), kiln_val_to_boundary(b), out_order);
}

typedef struct {
    kiln_host *host;
    kiln_value comparator;
    int depth;
    kiln_value *thrown;
} kiln_c99_typed_sort_context;

static int kiln_c99_sort_compare_typed(kiln_c99_typed_sort_context *ctx, double a,
                                       double b, double *out_order) {
    if (ctx->comparator.tag == KILN_VALUE_UNDEFINED) {
        *out_order = (double)kiln_c99_typed_order(a, b);
        return KILN_C99_OK;
    } else {
        kiln_value pair[2];
        kiln_value answer;
        pair[0] = kiln_c99_number(a);
        pair[1] = kiln_c99_number(b);
        int status = kiln_c99_call(ctx->host, &answer, ctx->comparator, kiln_c99_undefined(),
                                   pair, 2, ctx->depth, ctx->thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        double decided = kiln_c99_as_number(answer);
        *out_order = isnan(decided) ? 0.0 : (decided > 0.0 ? 1.0 : (decided < 0.0 ? -1.0 : 0.0));
        return KILN_C99_OK;
    }
}

typedef struct {
    size_t base;
    size_t len;
} kiln_c99_timsort_run;

static size_t kiln_c99_timsort_min_run_length(size_t n) {
    size_t r = 0;
    while (n >= 64) {
        r |= (n & 1);
        n >>= 1;
    }
    return n + r;
}

#define KILN_DEF_TIMSORT(SUFFIX, TYPE, CTX_TYPE, CMP_FUNC)                                         \
    typedef struct {                                                                               \
        CTX_TYPE *ctx;                                                                             \
        TYPE *a;                                                                                   \
        TYPE *scratch;                                                                             \
        size_t scratch_capacity;                                                                   \
        size_t min_gallop;                                                                         \
        kiln_c99_timsort_run stack[85];                                                            \
        size_t stack_len;                                                                          \
    } kiln_c99_timsort_state_##SUFFIX;                                                             \
                                                                                                   \
    static int kiln_c99_timsort_binary_sort_##SUFFIX(kiln_c99_timsort_state_##SUFFIX *ts,          \
                                                     size_t lo, size_t hi, size_t start) {         \
        if (start == lo)                                                                           \
            start++;                                                                               \
        for (; start < hi; start++) {                                                              \
            TYPE pivot = ts->a[start];                                                             \
            size_t left = lo;                                                                      \
            size_t right = start;                                                                  \
            while (left < right) {                                                                 \
                size_t mid = left + ((right - left) >> 1);                                         \
                double order;                                                                      \
                int status = CMP_FUNC(ts->ctx, pivot, ts->a[mid], &order);                         \
                if (status != KILN_C99_OK)                                                         \
                    return status;                                                                 \
                if (order < 0.0) {                                                                 \
                    right = mid;                                                                   \
                } else {                                                                           \
                    left = mid + 1;                                                                \
                }                                                                                  \
            }                                                                                      \
            size_t n_shift = start - left;                                                         \
            if (n_shift > 0) {                                                                     \
                memmove(&ts->a[left + 1], &ts->a[left], n_shift * sizeof(TYPE));                   \
            }                                                                                      \
            ts->a[left] = pivot;                                                                   \
        }                                                                                          \
        return KILN_C99_OK;                                                                        \
    }                                                                                              \
                                                                                                   \
    static int kiln_c99_timsort_count_run_##SUFFIX(kiln_c99_timsort_state_##SUFFIX *ts, size_t lo, \
                                                   size_t hi, size_t *out_len) {                   \
        if (lo + 1 >= hi) {                                                                        \
            *out_len = hi - lo;                                                                    \
            return KILN_C99_OK;                                                                    \
        }                                                                                          \
        size_t run_hi = lo + 2;                                                                    \
        double order;                                                                              \
        int status = CMP_FUNC(ts->ctx, ts->a[lo + 1], ts->a[lo], &order);                          \
        if (status != KILN_C99_OK)                                                                 \
            return status;                                                                         \
        if (order < 0.0) {                                                                         \
            while (run_hi < hi) {                                                                  \
                status = CMP_FUNC(ts->ctx, ts->a[run_hi], ts->a[run_hi - 1], &order);              \
                if (status != KILN_C99_OK)                                                         \
                    return status;                                                                 \
                if (order < 0.0)                                                                   \
                    run_hi++;                                                                      \
                else                                                                               \
                    break;                                                                         \
            }                                                                                      \
            size_t p = lo;                                                                         \
            size_t q = run_hi - 1;                                                                 \
            while (p < q) {                                                                        \
                TYPE tmp = ts->a[p];                                                               \
                ts->a[p] = ts->a[q];                                                               \
                ts->a[q] = tmp;                                                                    \
                p++;                                                                               \
                q--;                                                                               \
            }                                                                                      \
        } else {                                                                                   \
            while (run_hi < hi) {                                                                  \
                status = CMP_FUNC(ts->ctx, ts->a[run_hi], ts->a[run_hi - 1], &order);              \
                if (status != KILN_C99_OK)                                                         \
                    return status;                                                                 \
                if (order >= 0.0)                                                                  \
                    run_hi++;                                                                      \
                else                                                                               \
                    break;                                                                         \
            }                                                                                      \
        }                                                                                          \
        *out_len = run_hi - lo;                                                                    \
        return KILN_C99_OK;                                                                        \
    }                                                                                              \
                                                                                                   \
    static int kiln_c99_timsort_gallop_left_##SUFFIX(kiln_c99_timsort_state_##SUFFIX *ts,          \
                                                     TYPE key, TYPE *a, size_t base, size_t len,   \
                                                     size_t hint, size_t *out_pos) {               \
        size_t last_offset = 0;                                                                    \
        size_t offset = 1;                                                                         \
        double order;                                                                              \
        int status = CMP_FUNC(ts->ctx, a[base + hint], key, &order);                               \
        if (status != KILN_C99_OK)                                                                 \
            return status;                                                                         \
        size_t low;                                                                                \
        size_t high;                                                                               \
        if (order < 0.0) {                                                                         \
            size_t max_offset = len - hint;                                                        \
            while (offset < max_offset) {                                                          \
                status = CMP_FUNC(ts->ctx, a[base + hint + offset], key, &order);                  \
                if (status != KILN_C99_OK)                                                         \
                    return status;                                                                 \
                if (order < 0.0) {                                                                 \
                    last_offset = offset;                                                          \
                    offset = (offset << 1) + 1;                                                    \
                } else                                                                             \
                    break;                                                                         \
            }                                                                                      \
            if (offset > max_offset)                                                               \
                offset = max_offset;                                                               \
            low = hint + last_offset + 1;                                                          \
            high = hint + offset;                                                                  \
            if (high > len)                                                                        \
                high = len;                                                                        \
        } else {                                                                                   \
            size_t max_offset = hint + 1;                                                          \
            while (offset < max_offset) {                                                          \
                status = CMP_FUNC(ts->ctx, a[base + hint - offset], key, &order);                  \
                if (status != KILN_C99_OK)                                                         \
                    return status;                                                                 \
                if (order >= 0.0) {                                                                \
                    last_offset = offset;                                                          \
                    offset = (offset << 1) + 1;                                                    \
                } else                                                                             \
                    break;                                                                         \
            }                                                                                      \
            if (offset > max_offset)                                                               \
                offset = max_offset;                                                               \
            low = (offset > hint) ? 0 : hint - offset + 1;                                         \
            high = hint - last_offset;                                                             \
        }                                                                                          \
        while (low < high) {                                                                       \
            size_t mid = low + ((high - low) >> 1);                                                \
            status = CMP_FUNC(ts->ctx, a[base + mid], key, &order);                                \
            if (status != KILN_C99_OK)                                                             \
                return status;                                                                     \
            if (order < 0.0)                                                                       \
                low = mid + 1;                                                                     \
            else                                                                                   \
                high = mid;                                                                        \
        }                                                                                          \
        *out_pos = high;                                                                           \
        return KILN_C99_OK;                                                                        \
    }                                                                                              \
                                                                                                   \
    static int kiln_c99_timsort_gallop_right_##SUFFIX(kiln_c99_timsort_state_##SUFFIX *ts,         \
                                                      TYPE key, TYPE *a, size_t base, size_t len,  \
                                                      size_t hint, size_t *out_pos) {              \
        size_t last_offset = 0;                                                                    \
        size_t offset = 1;                                                                         \
        double order;                                                                              \
        int status = CMP_FUNC(ts->ctx, key, a[base + hint], &order);                               \
        if (status != KILN_C99_OK)                                                                 \
            return status;                                                                         \
        size_t low;                                                                                \
        size_t high;                                                                               \
        if (order < 0.0) {                                                                         \
            size_t max_offset = hint + 1;                                                          \
            while (offset < max_offset) {                                                          \
                status = CMP_FUNC(ts->ctx, key, a[base + hint - offset], &order);                  \
                if (status != KILN_C99_OK)                                                         \
                    return status;                                                                 \
                if (order < 0.0) {                                                                 \
                    last_offset = offset;                                                          \
                    offset = (offset << 1) + 1;                                                    \
                } else                                                                             \
                    break;                                                                         \
            }                                                                                      \
            if (offset > max_offset)                                                               \
                offset = max_offset;                                                               \
            low = (offset > hint) ? 0 : hint - offset + 1;                                         \
            high = hint - last_offset;                                                             \
        } else {                                                                                   \
            size_t max_offset = len - hint;                                                        \
            while (offset < max_offset) {                                                          \
                status = CMP_FUNC(ts->ctx, key, a[base + hint + offset], &order);                  \
                if (status != KILN_C99_OK)                                                         \
                    return status;                                                                 \
                if (order >= 0.0) {                                                                \
                    last_offset = offset;                                                          \
                    offset = (offset << 1) + 1;                                                    \
                } else                                                                             \
                    break;                                                                         \
            }                                                                                      \
            if (offset > max_offset)                                                               \
                offset = max_offset;                                                               \
            low = hint + last_offset + 1;                                                          \
            high = hint + offset;                                                                  \
            if (high > len)                                                                        \
                high = len;                                                                        \
        }                                                                                          \
        while (low < high) {                                                                       \
            size_t mid = low + ((high - low) >> 1);                                                \
            status = CMP_FUNC(ts->ctx, key, a[base + mid], &order);                                \
            if (status != KILN_C99_OK)                                                             \
                return status;                                                                     \
            if (order < 0.0)                                                                       \
                high = mid;                                                                        \
            else                                                                                   \
                low = mid + 1;                                                                     \
        }                                                                                          \
        *out_pos = high;                                                                           \
        return KILN_C99_OK;                                                                        \
    }                                                                                              \
                                                                                                   \
    static int kiln_c99_timsort_merge_lo_##SUFFIX(kiln_c99_timsort_state_##SUFFIX *ts,            \
                                                  size_t base1, size_t len1, size_t base2,         \
                                                  size_t len2) {                                   \
        if (len1 > ts->scratch_capacity) {                                                         \
            TYPE *new_scratch = (TYPE *)realloc(ts->scratch, len1 * sizeof(TYPE));                 \
            if (new_scratch == NULL)                                                               \
                return KILN_C99_OUT_OF_MEMORY;                                                     \
            ts->scratch = new_scratch;                                                             \
            ts->scratch_capacity = len1;                                                           \
        }                                                                                          \
        memcpy(ts->scratch, &ts->a[base1], len1 * sizeof(TYPE));                                   \
        size_t cursor1 = 0;                                                                        \
        size_t cursor2 = base2;                                                                    \
        size_t dest = base1;                                                                       \
        size_t min_gallop = ts->min_gallop;                                                        \
        while (1) {                                                                                \
            size_t count1 = 0;                                                                     \
            size_t count2 = 0;                                                                     \
            while (1) {                                                                            \
                double order;                                                                      \
                int status = CMP_FUNC(ts->ctx, ts->a[cursor2], ts->scratch[cursor1], &order);      \
                if (status != KILN_C99_OK) {                                                       \
                    if (len1 > 0)                                                                  \
                        memcpy(&ts->a[dest], &ts->scratch[cursor1], len1 * sizeof(TYPE));          \
                    return status;                                                                 \
                }                                                                                  \
                if (order < 0.0) {                                                                 \
                    ts->a[dest++] = ts->a[cursor2++];                                              \
                    count2++;                                                                      \
                    count1 = 0;                                                                    \
                    if (--len2 == 0)                                                               \
                        goto outer_done_lo_##SUFFIX;                                               \
                    if (count2 >= min_gallop)                                                      \
                        break;                                                                     \
                } else {                                                                           \
                    ts->a[dest++] = ts->scratch[cursor1++];                                        \
                    count1++;                                                                      \
                    count2 = 0;                                                                    \
                    if (--len1 == 0)                                                               \
                        goto outer_done_lo_##SUFFIX;                                               \
                    if (count1 >= min_gallop)                                                      \
                        break;                                                                     \
                }                                                                                  \
            }                                                                                      \
            while (1) {                                                                            \
                size_t k;                                                                          \
                int status = kiln_c99_timsort_gallop_right_##SUFFIX(ts, ts->a[cursor2],            \
                                                                    ts->scratch, cursor1, len1, 0, \
                                                                    &k);                           \
                if (status != KILN_C99_OK) {                                                       \
                    if (len1 > 0)                                                                  \
                        memcpy(&ts->a[dest], &ts->scratch[cursor1], len1 * sizeof(TYPE));          \
                    return status;                                                                 \
                }                                                                                  \
                if (k > 0) {                                                                       \
                    memcpy(&ts->a[dest], &ts->scratch[cursor1], k * sizeof(TYPE));                 \
                    dest += k;                                                                     \
                    cursor1 += k;                                                                  \
                    len1 -= k;                                                                     \
                    if (len1 == 0)                                                                 \
                        goto outer_done_lo_##SUFFIX;                                               \
                }                                                                                  \
                ts->a[dest++] = ts->a[cursor2++];                                                  \
                if (--len2 == 0)                                                                   \
                    goto outer_done_lo_##SUFFIX;                                                   \
                size_t m;                                                                          \
                status = kiln_c99_timsort_gallop_left_##SUFFIX(ts, ts->scratch[cursor1], ts->a,    \
                                                               cursor2, len2, 0, &m);              \
                if (status != KILN_C99_OK) {                                                       \
                    if (len1 > 0)                                                                  \
                        memcpy(&ts->a[dest], &ts->scratch[cursor1], len1 * sizeof(TYPE));          \
                    return status;                                                                 \
                }                                                                                  \
                if (m > 0) {                                                                       \
                    memmove(&ts->a[dest], &ts->a[cursor2], m * sizeof(TYPE));                      \
                    dest += m;                                                                     \
                    cursor2 += m;                                                                  \
                    len2 -= m;                                                                     \
                    if (len2 == 0)                                                                 \
                        goto outer_done_lo_##SUFFIX;                                               \
                }                                                                                  \
                ts->a[dest++] = ts->scratch[cursor1++];                                            \
                if (--len1 == 0)                                                                   \
                    goto outer_done_lo_##SUFFIX;                                                   \
                if (k < 7 && m < 7) {                                                              \
                    min_gallop++;                                                                  \
                    break;                                                                         \
                }                                                                                  \
                if (min_gallop > 1)                                                                \
                    min_gallop--;                                                                  \
            }                                                                                      \
        }                                                                                          \
    outer_done_lo_##SUFFIX:                                                                        \
        ts->min_gallop = min_gallop < 1 ? 1 : min_gallop;                                          \
        if (len1 > 0) {                                                                            \
            memcpy(&ts->a[dest], &ts->scratch[cursor1], len1 * sizeof(TYPE));                      \
        }                                                                                          \
        return KILN_C99_OK;                                                                        \
    }                                                                                              \
                                                                                                   \
    static int kiln_c99_timsort_merge_hi_##SUFFIX(kiln_c99_timsort_state_##SUFFIX *ts,            \
                                                  size_t base1, size_t len1, size_t base2,         \
                                                  size_t len2) {                                   \
        if (len2 > ts->scratch_capacity) {                                                         \
            TYPE *new_scratch = (TYPE *)realloc(ts->scratch, len2 * sizeof(TYPE));                 \
            if (new_scratch == NULL)                                                               \
                return KILN_C99_OUT_OF_MEMORY;                                                     \
            ts->scratch = new_scratch;                                                             \
            ts->scratch_capacity = len2;                                                           \
        }                                                                                          \
        memcpy(ts->scratch, &ts->a[base2], len2 * sizeof(TYPE));                                   \
        size_t cursor1 = base1 + len1 - 1;                                                         \
        size_t cursor2 = len2 - 1;                                                                 \
        size_t dest = base2 + len2 - 1;                                                            \
        size_t min_gallop = ts->min_gallop;                                                        \
        while (1) {                                                                                \
            size_t count1 = 0;                                                                     \
            size_t count2 = 0;                                                                     \
            while (1) {                                                                            \
                double order;                                                                      \
                int status = CMP_FUNC(ts->ctx, ts->scratch[cursor2], ts->a[cursor1], &order);      \
                if (status != KILN_C99_OK) {                                                       \
                    if (len2 > 0)                                                                  \
                        memcpy(&ts->a[dest - len2 + 1], ts->scratch, len2 * sizeof(TYPE));         \
                    return status;                                                                 \
                }                                                                                  \
                if (order < 0.0) {                                                                 \
                    ts->a[dest--] = ts->a[cursor1--];                                              \
                    count1++;                                                                      \
                    count2 = 0;                                                                    \
                    if (--len1 == 0)                                                               \
                        goto outer_done_hi_##SUFFIX;                                               \
                    if (count1 >= min_gallop)                                                      \
                        break;                                                                     \
                } else {                                                                           \
                    ts->a[dest--] = ts->scratch[cursor2--];                                        \
                    count2++;                                                                      \
                    count1 = 0;                                                                    \
                    if (--len2 == 0)                                                               \
                        goto outer_done_hi_##SUFFIX;                                               \
                    if (count2 >= min_gallop)                                                      \
                        break;                                                                     \
                }                                                                                  \
            }                                                                                      \
            while (1) {                                                                            \
                size_t k;                                                                          \
                int status = kiln_c99_timsort_gallop_right_##SUFFIX(                               \
                    ts, ts->scratch[cursor2], ts->a, base1, len1, len1 - 1, &k);                   \
                if (status != KILN_C99_OK) {                                                       \
                    if (len2 > 0)                                                                  \
                        memcpy(&ts->a[dest - len2 + 1], ts->scratch, len2 * sizeof(TYPE));         \
                    return status;                                                                 \
                }                                                                                  \
                size_t n_copy = len1 - k;                                                          \
                if (n_copy > 0) {                                                                  \
                    dest -= n_copy;                                                                \
                    cursor1 -= n_copy;                                                             \
                    memmove(&ts->a[dest + 1], &ts->a[cursor1 + 1], n_copy * sizeof(TYPE));         \
                    len1 -= n_copy;                                                                \
                    if (len1 == 0)                                                                 \
                        goto outer_done_hi_##SUFFIX;                                               \
                }                                                                                  \
                ts->a[dest--] = ts->scratch[cursor2--];                                            \
                if (--len2 == 0)                                                                   \
                    goto outer_done_hi_##SUFFIX;                                                   \
                size_t m;                                                                          \
                status = kiln_c99_timsort_gallop_left_##SUFFIX(ts, ts->a[cursor1], ts->scratch, 0, \
                                                               len2, len2 - 1, &m);                \
                if (status != KILN_C99_OK) {                                                       \
                    if (len2 > 0)                                                                  \
                        memcpy(&ts->a[dest - len2 + 1], ts->scratch, len2 * sizeof(TYPE));         \
                    return status;                                                                 \
                }                                                                                  \
                size_t m_copy = len2 - m;                                                          \
                if (m_copy > 0) {                                                                  \
                    dest -= m_copy;                                                                \
                    cursor2 -= m_copy;                                                             \
                    memcpy(&ts->a[dest + 1], &ts->scratch[cursor2 + 1], m_copy * sizeof(TYPE));    \
                    len2 -= m_copy;                                                                \
                    if (len2 == 0)                                                                 \
                        goto outer_done_hi_##SUFFIX;                                               \
                }                                                                                  \
                ts->a[dest--] = ts->a[cursor1--];                                                  \
                if (--len1 == 0)                                                                   \
                    goto outer_done_hi_##SUFFIX;                                                   \
                if (n_copy < 7 && m_copy < 7) {                                                    \
                    min_gallop++;                                                                  \
                    break;                                                                         \
                }                                                                                  \
                if (min_gallop > 1)                                                                \
                    min_gallop--;                                                                  \
            }                                                                                      \
        }                                                                                          \
    outer_done_hi_##SUFFIX:                                                                        \
        ts->min_gallop = min_gallop < 1 ? 1 : min_gallop;                                          \
        if (len2 > 0) {                                                                            \
            memcpy(&ts->a[dest - len2 + 1], ts->scratch, len2 * sizeof(TYPE));                     \
        }                                                                                          \
        return KILN_C99_OK;                                                                        \
    }                                                                                              \
                                                                                                   \
    static int kiln_c99_timsort_merge_at_##SUFFIX(kiln_c99_timsort_state_##SUFFIX *ts, size_t i) { \
        size_t base1 = ts->stack[i].base;                                                          \
        size_t len1 = ts->stack[i].len;                                                            \
        size_t base2 = ts->stack[i + 1].base;                                                      \
        size_t len2 = ts->stack[i + 1].len;                                                        \
        ts->stack[i].len = len1 + len2;                                                            \
        if (i == ts->stack_len - 3) {                                                              \
            ts->stack[i + 1] = ts->stack[i + 2];                                                   \
        }                                                                                          \
        ts->stack_len--;                                                                           \
        size_t k;                                                                                  \
        int status =                                                                               \
            kiln_c99_timsort_gallop_right_##SUFFIX(ts, ts->a[base2], ts->a, base1, len1, 0, &k);   \
        if (status != KILN_C99_OK)                                                                 \
            return status;                                                                         \
        base1 += k;                                                                                \
        len1 -= k;                                                                                 \
        if (len1 == 0)                                                                             \
            return KILN_C99_OK;                                                                    \
        size_t m;                                                                                  \
        status = kiln_c99_timsort_gallop_left_##SUFFIX(ts, ts->a[base1 + len1 - 1], ts->a, base2,  \
                                                       len2, len2 - 1, &m);                        \
        if (status != KILN_C99_OK)                                                                 \
            return status;                                                                         \
        len2 = m;                                                                                  \
        if (len2 == 0)                                                                             \
            return KILN_C99_OK;                                                                    \
        if (len1 <= len2) {                                                                        \
            return kiln_c99_timsort_merge_lo_##SUFFIX(ts, base1, len1, base2, len2);              \
        } else {                                                                                   \
            return kiln_c99_timsort_merge_hi_##SUFFIX(ts, base1, len1, base2, len2);              \
        }                                                                                          \
    }                                                                                              \
                                                                                                   \
    static int kiln_c99_timsort_merge_collapse_##SUFFIX(kiln_c99_timsort_state_##SUFFIX *ts) {      \
        while (ts->stack_len > 1) {                                                                \
            size_t n = ts->stack_len - 2;                                                          \
            if (n > 0 && ts->stack[n - 1].len <= ts->stack[n].len + ts->stack[n + 1].len) {        \
                if (ts->stack[n - 1].len < ts->stack[n + 1].len)                                   \
                    n--;                                                                           \
                int status = kiln_c99_timsort_merge_at_##SUFFIX(ts, n);                            \
                if (status != KILN_C99_OK)                                                         \
                    return status;                                                                 \
            } else if (ts->stack[n].len <= ts->stack[n + 1].len) {                                 \
                int status = kiln_c99_timsort_merge_at_##SUFFIX(ts, n);                            \
                if (status != KILN_C99_OK)                                                         \
                    return status;                                                                 \
            } else {                                                                               \
                break;                                                                             \
            }                                                                                      \
        }                                                                                          \
        return KILN_C99_OK;                                                                        \
    }                                                                                              \
                                                                                                   \
    static int kiln_c99_timsort_merge_force_collapse_##SUFFIX(                                     \
        kiln_c99_timsort_state_##SUFFIX *ts) {                                                     \
        while (ts->stack_len > 1) {                                                                \
            size_t n = ts->stack_len - 2;                                                          \
            if (n > 0 && ts->stack[n - 1].len < ts->stack[n + 1].len)                              \
                n--;                                                                               \
            int status = kiln_c99_timsort_merge_at_##SUFFIX(ts, n);                                \
            if (status != KILN_C99_OK)                                                             \
                return status;                                                                     \
        }                                                                                          \
        return KILN_C99_OK;                                                                        \
    }                                                                                              \
                                                                                                   \
    static int kiln_c99_timsort_##SUFFIX(CTX_TYPE *ctx, TYPE *a, size_t len) {                     \
        if (len < 2)                                                                               \
            return KILN_C99_OK;                                                                    \
        if (len < 32) {                                                                            \
            kiln_c99_timsort_state_##SUFFIX ts;                                                    \
            memset(&ts, 0, sizeof(ts));                                                            \
            ts.ctx = ctx;                                                                          \
            ts.a = a;                                                                              \
            size_t run_len = 0;                                                                    \
            int status = kiln_c99_timsort_count_run_##SUFFIX(&ts, 0, len, &run_len);               \
            if (status != KILN_C99_OK)                                                             \
                return status;                                                                     \
            return kiln_c99_timsort_binary_sort_##SUFFIX(&ts, 0, len, run_len);                   \
        }                                                                                          \
        kiln_c99_timsort_state_##SUFFIX ts;                                                        \
        memset(&ts, 0, sizeof(ts));                                                                \
        ts.ctx = ctx;                                                                              \
        ts.a = a;                                                                                  \
        ts.min_gallop = 7;                                                                         \
        ts.scratch_capacity = (len >> 1) + 1;                                                      \
        ts.scratch = (TYPE *)malloc(ts.scratch_capacity * sizeof(TYPE));                            \
        if (ts.scratch == NULL)                                                                    \
            return KILN_C99_OUT_OF_MEMORY;                                                         \
        size_t min_run = kiln_c99_timsort_min_run_length(len);                                     \
        size_t lo = 0;                                                                             \
        int status = KILN_C99_OK;                                                                  \
        while (lo < len) {                                                                         \
            size_t run_len;                                                                        \
            status = kiln_c99_timsort_count_run_##SUFFIX(&ts, lo, len, &run_len);                  \
            if (status != KILN_C99_OK)                                                             \
                break;                                                                             \
            if (run_len < min_run) {                                                               \
                size_t force = len - lo < min_run ? len - lo : min_run;                            \
                status = kiln_c99_timsort_binary_sort_##SUFFIX(&ts, lo, lo + force, lo + run_len); \
                if (status != KILN_C99_OK)                                                         \
                    break;                                                                         \
                run_len = force;                                                                   \
            }                                                                                      \
            ts.stack[ts.stack_len].base = lo;                                                      \
            ts.stack[ts.stack_len].len = run_len;                                                  \
            ts.stack_len++;                                                                        \
            status = kiln_c99_timsort_merge_collapse_##SUFFIX(&ts);                                \
            if (status != KILN_C99_OK)                                                             \
                break;                                                                             \
            lo += run_len;                                                                         \
        }                                                                                          \
        if (status == KILN_C99_OK) {                                                               \
            status = kiln_c99_timsort_merge_force_collapse_##SUFFIX(&ts);                          \
        }                                                                                          \
        free(ts.scratch);                                                                          \
        return status;                                                                             \
    }

KILN_DEF_TIMSORT(val, kiln_val, kiln_c99_sort_context, kiln_c99_sort_compare_val)
KILN_DEF_TIMSORT(typed, double, kiln_c99_typed_sort_context, kiln_c99_sort_compare_typed)

/* The language has a distinct prototype object for Map and Set iterators. Keep one of each in
 * the realm so Object.getPrototypeOf can distinguish them from plain objects and so repeated
 * iterators share the same identity. Their iteration state remains on the instance. */
static int kiln_c99_collection_iterator_prototype(int is_map, kiln_value *out) {
    kiln_c99_realm_state *realm = kiln_c99_realm();
    kiln_value *slot = is_map ? &realm->map_iterator_prototype : &realm->set_iterator_prototype;
    int status;
    if (slot->tag == KILN_VALUE_OBJECT) {
        *out = *slot;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) return status;
    *slot = *out;
    status = kiln_c99_set_prototype(*out, kiln_c99_builtin(KILN_C99_BUILTIN_ITERATOR_PROTOTYPE));
    if (status == KILN_C99_OK) {
        status = kiln_c99_define_method(*out, "next", 4,
                                        kiln_c99_method(KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT));
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, *out, "@@toStringTag", 13,
                              kiln_c99_copy_text(is_map ? "Map Iterator" : "Set Iterator",
                                                 is_map ? 12 : 12),
                              0, NULL);
    }
    return status;
}

/* Array, String, RegExp String and TypedArray iterators also have distinct prototype identities.
 * Their common Iterator.prototype remains the parent, where lazy helpers live. */
static int kiln_c99_builtin_iterator_prototype(int kind, kiln_value *out) {
    kiln_c99_realm_state *realm = kiln_c99_realm();
    kiln_value *slot;
    const char *tag;
    size_t tag_len;
    int status;
    switch (kind) {
    case 0: slot = &realm->array_iterator_prototype; tag = "Array Iterator"; tag_len = 14; break;
    case 1: slot = &realm->string_iterator_prototype; tag = "String Iterator"; tag_len = 15; break;
    case 2: slot = &realm->regexp_string_iterator_prototype; tag = "RegExp String Iterator"; tag_len = 22; break;
    default: slot = &realm->typed_array_iterator_prototype; tag = "Array Iterator"; tag_len = 14; break;
    }
    if (slot->tag == KILN_VALUE_OBJECT) { *out = *slot; return KILN_C99_OK; }
    status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) return status;
    *slot = *out;
    status = kiln_c99_set_prototype(*out, kiln_c99_builtin(KILN_C99_BUILTIN_ITERATOR_PROTOTYPE));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "next", 4, kiln_c99_method(KILN_C99_METHOD_ITERATOR_NEXT));
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "@@toStringTag", 13, kiln_c99_copy_text(tag, tag_len), 0, NULL);
    return status;
}

static int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown) {
    kiln_c99_method_metadata metadata = kiln_c99_method_metadata_for(code);
    uint32_t numeric = metadata.numeric_arguments;
    uint32_t textual = metadata.textual_arguments;
    kiln_value inline_storage[KILN_C99_COERCED_INLINE];
    kiln_value *coerced = NULL;
    size_t index;
    int status = KILN_C99_OK;
    /* CallMethod is a fast path for an intrinsic property read plus call. A native namespace
     * can still acquire an own property at runtime (the Error bootstrap installs its public
     * stack formatter this way), so resolve that override before argument coercion or intrinsic
     * dispatch. This keeps the fast path equivalent to GetProperty followed by Call. */
    if (receiver.tag == KILN_VALUE_BUILTIN) {
        const char *name = kiln_c99_method_name(code);
        if (name != NULL) {
            kiln_value current;
            status = kiln_c99_read_property(host, receiver, name, strlen(name), depth, thrown,
                                            NULL, &current);
            if (status != KILN_C99_OK) return status;
            if (current.tag != KILN_VALUE_METHOD || current.bits != code) {
                return kiln_c99_call(host, result, current, receiver, args, argc, depth, thrown);
            }
        }
    }
    if (KILN_UNLIKELY(kiln_c99_stack_exhausted())) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Maximum call stack size exceeded");
    }
    if ((numeric | textual) == 0 || argc == 0) {
        return kiln_c99_invoke_method_dispatch(host, result, code, metadata, receiver, args, argc, depth,
                                               thrown);
    }
    for (index = 0; index < argc && index < 32; ++index) {
        int is_numeric = (numeric & (UINT32_C(1) << index)) != 0;
        int is_textual = (textual & (UINT32_C(1) << index)) != 0;
        const kiln_value *reading = coerced == NULL ? args : coerced;
        if (!is_numeric && !is_textual) {
            continue;
        }
        if (is_numeric ? !kiln_c99_needs_number_conversion(reading[index])
                       : !kiln_c99_needs_text_conversion(reading[index])) {
            continue;
        }
        /* An argument that already spells itself is the ordinary case and must not pay for a
         * copy; the copy is made the first time one does not. */
        if (coerced == NULL) {
            size_t copy;
            coerced = argc <= KILN_C99_COERCED_INLINE
                          ? inline_storage
                          : (kiln_value *)malloc(argc * sizeof *coerced);
            if (coerced == NULL) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            for (copy = 0; copy < argc; ++copy) {
                coerced[copy] = args[copy];
            }
        }
        status = is_numeric ? kiln_c99_to_number_argument(host, coerced[index], &coerced[index],
                                                          depth, thrown)
                            : kiln_c99_to_text_argument(host, coerced[index], &coerced[index],
                                                        depth, thrown);
        if (status != KILN_C99_OK) {
            break;
        }
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_invoke_method_dispatch(host, result, code, metadata, receiver,
                                                 coerced == NULL ? args : coerced, argc, depth,
                                                 thrown);
    }
    if (coerced != NULL && coerced != inline_storage) {
        free(coerced);
    }
    return status;
}

/* One iteration result. A step is an object, not a pair: `{ value, done }` is what the iteration
 * protocol is written against. */
static int kiln_c99_generator_step(kiln_host *host, kiln_value produced, int done, kiln_value *result,
                                   int depth, kiln_value *thrown) {
    kiln_value step;
    int status = kiln_c99_new_object(&step);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_put(host, step, "value", 5, produced, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_put(host, step, "done", 4, kiln_c99_boolean(done), depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    *result = step;
    return KILN_C99_OK;
}

static int kiln_c99_is_object(kiln_value val) {
    return val.tag != KILN_VALUE_UNDEFINED && val.tag != KILN_VALUE_NULL &&
           (kiln_c99_is_reference(val) || val.tag == KILN_VALUE_DATE);
}

static int kiln_c99_generator_raise(kiln_host *host, kiln_generator *frame, kiln_value error,
                                    kiln_value *result, int depth, kiln_value *thrown) {
    kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
    frame->delegate_exit = 0;
    if (frame->handler_top != 0 && !frame->done && !frame->async_generator) {
        kiln_val vproduced = KILN_VAL_UNDEFINED;
        frame->raising = 1;
        kiln_c99_heap_store_value(frame, &frame->sent, error);
        kiln_closure *closure = kiln_c99_closure_of(frame->function);
        frame->running = 1;
        int status = kiln_c99_invoke_closure_body(
            host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0, depth, thrown, frame, 0);
        frame->running = 0;
        frame->raising = 0;
        if (status == KILN_C99_YIELDED) {
            return kiln_c99_generator_step(host, kiln_val_to_boundary(vproduced), 0, result, depth, thrown);
        }
        frame->done = 1;
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_generator_step(host, kiln_val_to_boundary(vproduced), 1, result, depth, thrown);
    }
    frame->done = 1;
    if (thrown != NULL) {
        *thrown = error;
    }
    return KILN_C99_THROWN;
}

static int kiln_c99_generator_close_with(kiln_host *host, kiln_generator *frame, kiln_value return_val,
                                         kiln_value *result, int depth, kiln_value *thrown) {
    kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
    frame->delegate_exit = 0;
    if (frame->handler_top != 0 && !frame->done && !frame->async_generator) {
        frame->raising = 1;
        kiln_c99_heap_store_value(frame, &frame->sent, kiln_c99_return_signal());
        kiln_closure *closure = kiln_c99_closure_of(frame->function);
        frame->running = 1;
        kiln_val vproduced = KILN_VAL_UNDEFINED;
        int status = kiln_c99_invoke_closure_body(
            host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0, depth, thrown, frame, 0);
        frame->running = 0;
        frame->raising = 0;
        if (status == KILN_C99_YIELDED) {
            return kiln_c99_generator_step(host, kiln_val_to_boundary(vproduced), 0, result, depth, thrown);
        }
        frame->done = 1;
        if (status != KILN_C99_OK && thrown != NULL && kiln_val_from_boundary(*thrown) != KILN_VAL_RETURN_SIGNAL) {
            return status;
        }
    }
    frame->done = 1;
    return kiln_c99_generator_step(host, return_val, 1, result, depth, thrown);
}

static int kiln_c99_generator_delegate_return(kiln_host *host, kiln_generator *frame, kiln_value sent_val,
                                              kiln_value *result, int depth, kiln_value *thrown) {
    kiln_value delegate = frame->delegate;
    kiln_value return_method = kiln_c99_undefined();
    int status = kiln_c99_get(host, delegate, "return", 6, &return_method, depth, thrown);
    if (status != KILN_C99_OK) {
        return kiln_c99_generator_raise(host, frame, *thrown, result, depth, thrown);
    }
    if (return_method.tag == KILN_VALUE_UNDEFINED || return_method.tag == KILN_VALUE_NULL) {
        return kiln_c99_generator_close_with(host, frame, sent_val, result, depth, thrown);
    }
    if (!kiln_c99_is_callable(return_method)) {
        kiln_value err;
        kiln_c99_throw_error(&err, KILN_C99_BUILTIN_TYPE_ERROR, "return property is not callable");
        return kiln_c99_generator_raise(host, frame, err, result, depth, thrown);
    }
    kiln_value inner_res = kiln_c99_undefined();
    status = kiln_c99_call(host, &inner_res, return_method, delegate, &sent_val, 1, depth, thrown);
    if (status != KILN_C99_OK) {
        return kiln_c99_generator_raise(host, frame, *thrown, result, depth, thrown);
    }
    if (!kiln_c99_is_object(inner_res)) {
        kiln_value err;
        kiln_c99_throw_error(&err, KILN_C99_BUILTIN_TYPE_ERROR, "Iterator result is not an object");
        return kiln_c99_generator_raise(host, frame, err, result, depth, thrown);
    }
    kiln_value done_val = kiln_c99_undefined();
    status = kiln_c99_get(host, inner_res, "done", 4, &done_val, depth, thrown);
    if (status != KILN_C99_OK) {
        return kiln_c99_generator_raise(host, frame, *thrown, result, depth, thrown);
    }
    if (kiln_c99_truthy(done_val)) {
        kiln_value inner_val = kiln_c99_undefined();
        status = kiln_c99_get(host, inner_res, "value", 5, &inner_val, depth, thrown);
        if (status != KILN_C99_OK) {
            return kiln_c99_generator_raise(host, frame, *thrown, result, depth, thrown);
        }
        return kiln_c99_generator_close_with(host, frame, inner_val, result, depth, thrown);
    } else {
        *result = inner_res;
        return KILN_C99_OK;
    }
}

static int kiln_c99_generator_delegate_throw(kiln_host *host, kiln_generator *frame, kiln_value thrown_val,
                                             kiln_value *result, int depth, kiln_value *thrown) {
    kiln_value delegate = frame->delegate;
    kiln_value throw_method = kiln_c99_undefined();
    int status = kiln_c99_get(host, delegate, "throw", 5, &throw_method, depth, thrown);
    if (status != KILN_C99_OK) {
        return kiln_c99_generator_raise(host, frame, *thrown, result, depth, thrown);
    }
    if (throw_method.tag == KILN_VALUE_UNDEFINED || throw_method.tag == KILN_VALUE_NULL) {
        kiln_value return_method = kiln_c99_undefined();
        status = kiln_c99_get(host, delegate, "return", 6, &return_method, depth, thrown);
        if (status != KILN_C99_OK) {
            return kiln_c99_generator_raise(host, frame, *thrown, result, depth, thrown);
        }
        if (return_method.tag != KILN_VALUE_UNDEFINED && return_method.tag != KILN_VALUE_NULL) {
            if (kiln_c99_is_callable(return_method)) {
                kiln_value close_res = kiln_c99_undefined();
                status = kiln_c99_call(host, &close_res, return_method, delegate, NULL, 0, depth, thrown);
                if (status != KILN_C99_OK) {
                    return kiln_c99_generator_raise(host, frame, *thrown, result, depth, thrown);
                }
                if (!kiln_c99_is_object(close_res)) {
                    kiln_value err;
                    kiln_c99_throw_error(&err, KILN_C99_BUILTIN_TYPE_ERROR, "Iterator result is not an object");
                    return kiln_c99_generator_raise(host, frame, err, result, depth, thrown);
                }
            }
        }
        kiln_value err;
        kiln_c99_throw_error(&err, KILN_C99_BUILTIN_TYPE_ERROR, "Iterator does not have a throw method");
        return kiln_c99_generator_raise(host, frame, err, result, depth, thrown);
    }
    if (!kiln_c99_is_callable(throw_method)) {
        kiln_value err;
        kiln_c99_throw_error(&err, KILN_C99_BUILTIN_TYPE_ERROR, "throw property is not callable");
        return kiln_c99_generator_raise(host, frame, err, result, depth, thrown);
    }
    kiln_value inner_res = kiln_c99_undefined();
    status = kiln_c99_call(host, &inner_res, throw_method, delegate, &thrown_val, 1, depth, thrown);
    if (status != KILN_C99_OK) {
        return kiln_c99_generator_raise(host, frame, *thrown, result, depth, thrown);
    }
    if (!kiln_c99_is_object(inner_res)) {
        kiln_value err;
        kiln_c99_throw_error(&err, KILN_C99_BUILTIN_TYPE_ERROR, "Iterator result is not an object");
        return kiln_c99_generator_raise(host, frame, err, result, depth, thrown);
    }
    kiln_value done_val = kiln_c99_undefined();
    status = kiln_c99_get(host, inner_res, "done", 4, &done_val, depth, thrown);
    if (status != KILN_C99_OK) {
        return kiln_c99_generator_raise(host, frame, *thrown, result, depth, thrown);
    }
    if (kiln_c99_truthy(done_val)) {
        kiln_value inner_val = kiln_c99_undefined();
        status = kiln_c99_get(host, inner_res, "value", 5, &inner_val, depth, thrown);
        if (status != KILN_C99_OK) {
            return kiln_c99_generator_raise(host, frame, *thrown, result, depth, thrown);
        }
        kiln_c99_heap_store_value(frame, &frame->delegate, kiln_c99_undefined());
        frame->resume = frame->delegate_exit;
        frame->delegate_exit = 0;
        frame->raising = 0;
        kiln_c99_heap_store_value(frame, &frame->sent, inner_val);
        kiln_closure *closure = kiln_c99_closure_of(frame->function);
        frame->running = 1;
        kiln_val vproduced = KILN_VAL_UNDEFINED;
        status = kiln_c99_invoke_closure_body(
            host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0, depth, thrown, frame, 0);
        kiln_value produced = kiln_val_to_boundary(vproduced);
        frame->running = 0;
        int finished = 1;
        if (status == KILN_C99_YIELDED) {
            frame->started = 1;
            finished = 0;
        } else if (status != KILN_C99_OK) {
            frame->done = 1;
            return status;
        } else {
            frame->done = 1;
        }
        return kiln_c99_generator_step(host, produced, finished || frame->done, result, depth, thrown);
    } else {
        *result = inner_res;
        return KILN_C99_OK;
    }
}

int kiln_c99_default_require(void *context, kiln_host *host, kiln_value specifier,
                             kiln_value *result, kiln_value *thrown) {
    (void)context;
    if (specifier.tag == KILN_VALUE_STRING && specifier.ptr != NULL) {
        if (kiln_c99_string_equals_ascii(specifier, "node:ffi", 8))
            return kiln_c99_node_ffi_commonjs(host, result, thrown);
        kiln_text text = {0};
        int status;
        if (kiln_c99_text_append_c(&text, "Cannot find module '") != KILN_C99_OK ||
            kiln_c99_text_append_string(&text, specifier) != KILN_C99_OK ||
            kiln_c99_text_append_c(&text, "'") != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_value message = kiln_c99_text_value(&text);
        if (thrown == NULL) return KILN_C99_INVALID_ARGUMENT;
        status = kiln_c99_new_error(thrown, KILN_C99_BUILTIN_ERROR, message);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_put(NULL, *thrown, "code", 4,
                              kiln_c99_static_text("MODULE_NOT_FOUND", 16), 0, NULL);
        if (status != KILN_C99_OK) return status;
        return KILN_C99_THROWN;
    }
    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR, "Cannot find module");
}

/* `parseInt(s, radix)` over the Latin1 code units of `s` (`radix` already `ToNumber`ed; 0 when
 * none was given), and `parseFloat(s)`. Both read as far as they can and stop, rather than
 * rejecting a string with a tail: `parseInt('12px')` is 12. That tolerance is the whole difference
 * between these and `Number(x)`, which rejects the same input. Pure: a pool thread parses with
 * them too (parallel tier 4). */
static double kiln_c99_parse_int_bytes(const char *bytes, size_t len, double radix) {
    size_t at = 0;
    double magnitude = 0.0;
    int base;
    int negative = 0;
    size_t digits_at;
    while (at < len && kiln_c99_is_trimmed((uint16_t)(unsigned char)bytes[at])) {
        at += 1;
    }
    radix = isfinite(radix) ? trunc(radix) : 0.0;
    if (!(radix == 0.0 || (radix >= 2.0 && radix <= 36.0))) {
        return 0.0 / 0.0;
    }
    base = (int)radix;
    if (at < len && (bytes[at] == '-' || bytes[at] == '+')) {
        negative = bytes[at] == '-';
        at += 1;
    }
    /* `0x` selects hexadecimal when no radix was given, and is allowed when 16 was. */
    if ((base == 0 || base == 16) && at + 1 < len && bytes[at] == '0' &&
        (bytes[at + 1] == 'x' || bytes[at + 1] == 'X')) {
        base = 16;
        at += 2;
    } else if (base == 0) {
        base = 10;
    }
    digits_at = at;
    while (at < len) {
        int digit;
        char character = bytes[at];
        if (character >= '0' && character <= '9') {
            digit = character - '0';
        } else if (character >= 'a' && character <= 'z') {
            digit = character - 'a' + 10;
        } else if (character >= 'A' && character <= 'Z') {
            digit = character - 'A' + 10;
        } else {
            break;
        }
        if (digit >= base) {
            break;
        }
        /* Folded as a double rather than accumulated as an integer: the result is a double
         * either way, and a long run of digits has to saturate toward infinity rather than
         * overflow. */
        magnitude = magnitude * (double)base + (double)digit;
        at += 1;
    }
    return at == digits_at ? 0.0 / 0.0 : (negative ? -magnitude : magnitude);
}

static double kiln_c99_parse_float_bytes(const char *bytes, size_t len) {
    /* The longest prefix that is a decimal literal, scanned rather than handed to `strtod`
     * outright: C's parser also accepts `0x1f`, `inf` and `nan`, none of which this grammar has
     * -- `parseFloat('0x1f')` is 0. */
    size_t at = 0;
    size_t start;
    size_t scan;
    size_t end;
    size_t before = 0;
    size_t after = 0;
    while (at < len && kiln_c99_is_trimmed((uint16_t)(unsigned char)bytes[at])) {
        at += 1;
    }
    start = at;
    if (at < len && (bytes[at] == '-' || bytes[at] == '+')) {
        at += 1;
    }
    if (len - at >= 8 && memcmp(bytes + at, "Infinity", 8) == 0) {
        return bytes[start] == '-' ? -1.0 / 0.0 : 1.0 / 0.0;
    }
    scan = at;
    while (scan < len && bytes[scan] >= '0' && bytes[scan] <= '9') {
        scan += 1;
        before += 1;
    }
    if (scan < len && bytes[scan] == '.') {
        scan += 1;
        while (scan < len && bytes[scan] >= '0' && bytes[scan] <= '9') {
            scan += 1;
            after += 1;
        }
    }
    if (before + after == 0) {
        return 0.0 / 0.0;
    }
    end = scan;
    if (scan < len && (bytes[scan] == 'e' || bytes[scan] == 'E')) {
        size_t probe = scan + 1;
        if (probe < len && (bytes[probe] == '+' || bytes[probe] == '-')) {
            probe += 1;
        }
        if (probe < len && bytes[probe] >= '0' && bytes[probe] <= '9') {
            while (probe < len && bytes[probe] >= '0' && bytes[probe] <= '9') {
                probe += 1;
            }
            /* An exponent with no digits is not part of the literal: `1e` is 1. */
            end = probe;
        }
    }
    /* The scanned prefix is a `StringNumericLiteral` by construction, so the provider's parser
     * is exactly the right reader for it -- and it is the same one `Number(x)` uses, which is what
     * keeps the two agreeing. `strtod` would read the process locale's decimal point, and an
     * embedded module runs in its host's. */
    return kiln_string_to_number(bytes + start, end - start);
}

static int kiln_c99_invoke_method_dispatch(kiln_host *host, kiln_value *result, uint64_t code,
                                           kiln_c99_method_metadata metadata,
                                           kiln_value receiver, const kiln_value *args,
                                           size_t argc, int depth, kiln_value *thrown) {
    kiln_array *array;
    size_t index;
    int status;
    if (code == KILN_C99_METHOD_NUMBER_VALUE_OF || code == KILN_C99_METHOD_NUMBER_TO_STRING ||
        code == KILN_C99_METHOD_NUMBER_TO_FIXED ||
        code == KILN_C99_METHOD_NUMBER_TO_EXPONENTIAL ||
        code == KILN_C99_METHOD_NUMBER_TO_PRECISION) {
        kiln_value primitive;
        if (kiln_c99_unwrap_primitive_wrapper(receiver, KILN_C99_WRAPPER_NUMBER, &primitive) ||
            kiln_c99_unwrap_primitive_wrapper(receiver, KILN_C99_WRAPPER_BIGINT, &primitive)) {
            receiver = primitive;
        } else if (receiver.tag != KILN_VALUE_NUMBER && receiver.tag != KILN_VALUE_BIGINT) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Number.prototype method called on an incompatible receiver");
        }
    }
    if (code == KILN_C99_METHOD_INTL_SEGMENT_RAW)
        return kiln_c99_intl_segment_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_STRING_LOCALE_CASE_RAW) {
        kiln_value input = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value locale = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_value upper = argc > 2 ? args[2] : kiln_c99_undefined();
        uint16_t *units = NULL, *mapped = NULL;
        uint8_t *tag = NULL;
        size_t count = 0, tag_len = 0;
        uint64_t need = 0, capacity;
        int rc, status;
        if (input.tag != KILN_VALUE_STRING ||
            (locale.tag != KILN_VALUE_STRING && locale.tag != KILN_VALUE_UNDEFINED) ||
            upper.tag != KILN_VALUE_BOOLEAN)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Invalid internal string locale case arguments");
        status = kiln_c99_to_units(input, &units, &count);
        if (status != KILN_C99_OK) return status;
        if (locale.tag == KILN_VALUE_STRING) {
            status = kiln_c99_string_to_utf8_lossy(locale, &tag, &tag_len);
            if (status != KILN_C99_OK) { free(units); return status; }
        }
        capacity = (uint64_t)count * 3 + 8;
        mapped = (uint16_t *)malloc((size_t)capacity * sizeof *mapped);
        if (mapped == NULL) { free(tag); free(units); return KILN_C99_OUT_OF_MEMORY; }
        rc = kiln_intl_string_case(units, count, tag, tag_len,
                                  upper.bits != 0 ? 1u : 0u, mapped, capacity, &need);
        if (rc == -1 && need > capacity) {
            uint16_t *bigger = (uint16_t *)realloc(mapped, (size_t)need * sizeof *mapped);
            if (bigger == NULL) { free(mapped); free(tag); free(units); return KILN_C99_OUT_OF_MEMORY; }
            mapped = bigger;
            rc = kiln_intl_string_case(units, count, tag, tag_len,
                                      upper.bits != 0 ? 1u : 0u, mapped, need, &need);
        }
        free(tag);
        free(units);
        if (rc == -3) {
            free(mapped);
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "Incorrect locale information provided");
        }
        if (rc != 0) {
            free(mapped);
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR,
                                        "Intl string case provider failed");
        }
        *result = kiln_c99_from_units(mapped, (size_t)need);
        free(mapped);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_INTL_DATETIME_FORMAT_RAW) {
        kiln_value spec = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value epoch = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_value operation = argc > 2 ? args[2] : kiln_c99_undefined();
        const uint8_t *bytes = NULL;
        uint64_t length = 0;
        uint8_t *owned = NULL;
        size_t owned_len = 0;
        int rc;
        if (spec.tag != KILN_VALUE_STRING || epoch.tag != KILN_VALUE_NUMBER ||
            operation.tag != KILN_VALUE_NUMBER || !isfinite(kiln_c99_as_number(operation)) ||
            floor(kiln_c99_as_number(operation)) != kiln_c99_as_number(operation) ||
            kiln_c99_as_number(operation) < 0 || kiln_c99_as_number(operation) > 6)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Invalid internal Intl.DateTimeFormat arguments");
        status = kiln_c99_string_to_utf8_lossy(spec, &owned, &owned_len);
        if (status != KILN_C99_OK) return status;
        rc = kiln_intl_datetime_format(owned, owned_len, kiln_c99_as_number(epoch),
                                       (uint32_t)kiln_c99_as_number(operation), &bytes, &length);
        free(owned);
        if (rc == -1) return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                  "Invalid Intl.DateTimeFormat specification");
        if (rc == -3) return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                  "Invalid Intl.DateTimeFormat options");
        if (rc != 0 || (length != 0 && bytes == NULL))
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR,
                                        "Intl.DateTimeFormat provider failed");
        *result = kiln_val_to_boundary(kiln_c99_val_from_text((const char *)bytes, (size_t)length));
        if (result->tag == KILN_VALUE_UNDEFINED) return KILN_C99_OUT_OF_MEMORY;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_UINT8_TO_HEX_RAW)
        return kiln_c99_uint8_to_hex_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_UINT8_TO_BASE64_RAW)
        return kiln_c99_uint8_to_base64_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_URL_PARSE_RAW)
        return kiln_c99_url_parse_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_URL_SET_RAW)
        return kiln_c99_url_set_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_URL_FAST_RAW)
        return kiln_c99_url_fast_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_URL_ADOPT_RAW)
        return kiln_c99_url_adopt_raw(result, args, argc);
    if (code == KILN_C99_METHOD_ARRAY_BUFFER_DETACH_RAW)
        return kiln_c99_array_buffer_detach_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_CLONE_DATA_RAW)
        return kiln_c99_clone_data_raw(result, args, argc);
    if (code == KILN_C99_METHOD_SHARED_IS_RAW) {
        *result = kiln_c99_boolean(argc > 0 && args[0].tag == KILN_VALUE_SHARED);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_COLLECTION_SIZE_RAW) {
        if (argc == 0 || args[0].tag != KILN_VALUE_COLLECTION ||
            kiln_c99_collection_of(args[0])->is_weak || argc < 2 ||
            args[1].tag != KILN_VALUE_BOOLEAN ||
            kiln_c99_collection_of(args[0])->is_map != (args[1].bits != 0)) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "get size: receiver does not have the required internal slot");
        }
        *result = kiln_c99_number((double)kiln_c99_collection_of(args[0])->live);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_SHARED_WEAK_RAW) {
        double operation = argc > 0 && args[0].tag == KILN_VALUE_NUMBER
            ? kiln_c99_as_number(args[0]) : -1.0;
        *result = kiln_c99_undefined();
        if (operation == 0.0 && argc > 1 && args[1].tag == KILN_VALUE_SHARED) {
            status = kiln_c99_gc_keep_during_job(args[1]);
            if (status != KILN_C99_OK) return status;
            kiln_shared_facade *facade = (kiln_shared_facade *)(uintptr_t)args[1].bits;
            uint32_t graph = facade->graph, generation = facade->generation, node = facade->node;
            uint64_t lease = kiln_shared_graph_retain(graph, generation, node);
            kiln_value token;
            int answer;
            if (lease == 0) return KILN_C99_INVALID_ARGUMENT;
            answer = kiln_c99_new_array(&token);
            if (answer == KILN_C99_OK)
                answer = kiln_c99_array_push(token, kiln_c99_number((double)graph));
            if (answer == KILN_C99_OK)
                answer = kiln_c99_array_push(token, kiln_c99_number((double)generation));
            if (answer == KILN_C99_OK)
                answer = kiln_c99_array_push(token, kiln_c99_number((double)node));
            (void)kiln_shared_graph_release(lease);
            if (answer == KILN_C99_OK) *result = token;
            return answer;
        }
        if (operation == 1.0 && argc > 1 && args[1].tag == KILN_VALUE_ARRAY) {
            kiln_array *token = kiln_c99_array_of(args[1]);
            uint32_t words[3];
            size_t at;
            if (token->count != 3) return KILN_C99_INVALID_ARGUMENT;
            for (at = 0; at < 3; ++at) {
                kiln_value item = kiln_val_to_boundary(token->elements[at]);
                double number = item.tag == KILN_VALUE_NUMBER ? kiln_c99_as_number(item) : -1.0;
                if (!isfinite(number) || number < 0.0 || number > (double)UINT32_MAX ||
                    floor(number) != number) return KILN_C99_INVALID_ARGUMENT;
                words[at] = (uint32_t)number;
            }
            status = kiln_c99_new_shared_facade(words[0], words[1], words[2], result);
            if (status == KILN_C99_OK) status = kiln_c99_gc_keep_during_job(*result);
            return status == KILN_C99_INVALID_ARGUMENT ? KILN_C99_OK : status;
        }
        if (operation == 2.0 && argc > 2) {
            status = kiln_c99_weak_reference_create(args[1], args[2]);
            if (status == KILN_C99_INVALID_ARGUMENT)
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "WeakRef: invalid target or receiver");
            if (status == KILN_C99_OK) *result = kiln_c99_boolean(1);
            return status;
        }
        if (operation == 3.0 && argc > 1) {
            status = kiln_c99_weak_reference_deref(args[1], result);
            if (status == KILN_C99_INVALID_ARGUMENT)
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "WeakRef.prototype.deref called on an incompatible receiver");
            return status;
        }
        if (operation == 4.0 && argc > 2) {
            status = kiln_c99_finalization_registry_create(args[1], args[2]);
            if (status == KILN_C99_INVALID_ARGUMENT)
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "FinalizationRegistry: invalid receiver");
            if (status == KILN_C99_OK) *result = kiln_c99_boolean(1);
            return status;
        }
        if (operation == 5.0 && argc > 4) {
            status = kiln_c99_finalization_registry_register(args[1], args[2], args[3], args[4]);
            if (status == KILN_C99_INVALID_ARGUMENT)
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "FinalizationRegistry.prototype.register called on an incompatible receiver");
            return status;
        }
        if (operation == 6.0 && argc > 2) {
            status = kiln_c99_finalization_registry_unregister(args[1], args[2], result);
            if (status == KILN_C99_INVALID_ARGUMENT)
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "FinalizationRegistry.prototype.unregister called on an incompatible receiver");
            return status;
        }
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_SHARE_RAW) {
        int status = kiln_c99_share_value(argc > 0 ? args[0] : kiln_c99_undefined(), result);
        if (status == KILN_C99_INVALID_ARGUMENT)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "cannot share this value or property graph");
        return status;
    }
    if (code == KILN_C99_METHOD_TYPED_ARRAY_COPY_RAW)
        return kiln_c99_typed_array_copy_raw(result, args, argc);
    if (code == KILN_C99_METHOD_TYPED_ARRAY_SORT_RAW)
        return kiln_c99_typed_array_sort_raw(result, args, argc);
    if (code == KILN_C99_METHOD_JOBS_PENDING_RAW)
        return kiln_c99_jobs_pending_raw(result);
    if (code == KILN_C99_METHOD_WORKER_OP_RAW)
        return kiln_c99_worker_op_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_PORT_WAIT_RAW)
        return kiln_c99_port_wait_raw(result, args, argc);
    if (code == KILN_C99_METHOD_SHARED_BUFFER_RAW)
        return kiln_c99_shared_buffer_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_ATOMICS_RAW)
        return kiln_c99_atomics_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_FS_DESCRIPTOR_RAW)
        return kiln_c99_fs_descriptor_raw(result, args, argc, thrown);
    if (code == KILN_C99_METHOD_PROMISE_REJECTIONS_RAW)
        return kiln_c99_promise_rejections_raw(result, args, argc);
    if ((code & KILN_C99_FFI_DYNAMIC_BASE) != 0 ||
        code == KILN_C99_METHOD_NODE_FFI_DLOPEN ||
        code == KILN_C99_METHOD_NODE_FFI_GET_RAW_POINTER ||
        code == KILN_C99_METHOD_NODE_FFI_TO_ARRAY_BUFFER ||
        code == KILN_C99_METHOD_NODE_FFI_REGISTER_CALLBACK ||
        code == KILN_C99_METHOD_NODE_FFI_UNREGISTER_CALLBACK ||
        code == KILN_C99_METHOD_NODE_FFI_CLOSE)
        return kiln_c99_node_ffi(host, result, code, receiver, args, argc, depth, thrown);
    /* The generated descriptor answers all dispatch groups once. */
    uint32_t groups = metadata.groups;
    /* The iterator helpers, routed by *range* rather than by group. They are one contiguous block
     * with one handler, and a range test here is a comparison against two constants where a group
     * bit would have cost one of the twenty-one the mask has left. Nothing else may be numbered
     * inside 470..484 -- see the registry comment beside the codes. */
    if (code >= KILN_C99_METHOD_ITERATOR_MAP && code <= KILN_C99_METHOD_ITERATOR_FROM) {
        return kiln_c99_iterator_method(host, result, code, receiver, args, argc, depth, thrown);
    }
    /* Demand-driven prelude: each group block below is compiled out when the
     * executable retains no code that can produce its method codes (see
     * `retained_method_groups`). A dropped block is unreachable, so control
     * falls through to the remaining tests exactly as an unknown method would. */
#if KILN_RETAIN_GROUP_NODE_EVENT
    if ((groups & KILN_C99_GROUP_NODE_EVENT)) {
        return kiln_c99_node_event(host, result, code, receiver, args, argc, depth, thrown);
    }
#endif
    /* `setEncoding` is the one method name three unrelated Node receivers own: a `net.Socket`,
     * an `http.IncomingMessage`, and a `stream.Readable`. A dispatch group answers "which handler
     * owns this code", so a code carrying three group bits is answered by whichever group happens
     * to be tested first -- which is not a property of the call. Route this one by the receiver's
     * own hidden state, above every group test, so the name resolves to the object it was called
     * on rather than to the order these ifs are written in. */
    /* Receiver-routed, above every group test (see the note above): reachable
     * only through node socket/response/stream objects. Guarded on the union
     * of the three groups that can own this code. */
#if KILN_RETAIN_GROUP_NODE_NET || KILN_RETAIN_GROUP_NODE_HTTP || KILN_RETAIN_GROUP_NODE_STREAM
    if (code == KILN_C99_METHOD_NODE_READABLE_SET_ENCODING) {
        if (receiver.tag == KILN_VALUE_OBJECT) {
            if (kiln_c99_node_hidden_pointer(receiver, "@@node:socket", 13) != NULL) {
                return kiln_c99_node_net(host, result, code, receiver, args, argc, depth, thrown);
            }
            if (kiln_c99_node_hidden_pointer(receiver, "@@node:client", 13) != NULL ||
                kiln_c99_node_hidden_pointer(receiver, "@@node:response", 15) != NULL) {
                return kiln_c99_node_http(host, result, code, receiver, args, argc, depth,
                                          thrown);
            }
        }
        /* A plain `stream.Readable`, and the one path that refuses a receiver owning none of
         * that state -- by name, from the handler whose prototype the method came off. */
        return kiln_c99_node_stream(host, result, code, receiver, args, argc, depth, thrown);
    }
#endif
#if KILN_RETAIN_GROUP_NODE_NET
    if ((groups & KILN_C99_GROUP_NODE_NET)) {
        return kiln_c99_node_net(host, result, code, receiver, args, argc, depth, thrown);
    }
#endif
#if KILN_RETAIN_GROUP_NODE_HTTP
    if ((groups & KILN_C99_GROUP_NODE_HTTP)) {
        return kiln_c99_node_http(host, result, code, receiver, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_HTTP_SERVER) {
        return kiln_c99_node_http(host, result, code, receiver, args, argc, depth, thrown);
    }
#endif
#if KILN_RETAIN_GROUP_NODE_NET || KILN_RETAIN_GROUP_NODE_SERVER
    if (code == KILN_C99_METHOD_NODE_SERVER_SET_TIMEOUT) {
        return kiln_c99_node_net(host, result, code, receiver, args, argc, depth, thrown);
    }
#endif
#if KILN_RETAIN_GROUP_NODE_RESPONSE
    if ((groups & KILN_C99_GROUP_NODE_RESPONSE)) {
        return kiln_c99_node_http(host, result, code, receiver, args, argc, depth, thrown);
    }
#endif
#if KILN_RETAIN_GROUP_NODE_NET
    if (code == KILN_C99_METHOD_NODE_NET_CONNECT) {
        return kiln_c99_node_net(host, result, code, receiver, args, argc, depth, thrown);
    }
#endif
    if (code == KILN_C99_METHOD_URL_SEARCH_PARAMS_GET) {
        return kiln_c99_url_search_params_get(result, receiver, args, argc, thrown);
    }
#if KILN_RETAIN_GROUP_BUFFER
    if ((groups & KILN_C99_GROUP_BUFFER)) {
        return kiln_c99_buffer_method(result, code, receiver, args, argc, thrown);
    }
#endif
    if (code == KILN_C99_METHOD_SYMBOL_FOR) {
        return kiln_c99_symbol_for(result, argc > 0 ? args[0] : kiln_c99_undefined());
    }
    if (code == KILN_C99_METHOD_SYMBOL_KEY_FOR) {
        *result = kiln_c99_symbol_key_for(argc > 0 ? args[0] : kiln_c99_undefined());
        return KILN_C99_OK;
    }
    /* One range test for the whole of `DataView.prototype`: the block is contiguous by
     * construction, so the dispatcher asks "is this one of them" once rather than twenty times. */
    if (code >= KILN_C99_METHOD_DATA_VIEW_GET_INT8 &&
        code <= KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64) {
        return kiln_c99_data_view_method(host, result, code, receiver, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_TYPED_TO_STRING_TAG) {
        if (receiver.tag != KILN_VALUE_TYPED_ARRAY) {
            return kiln_c99_throw_error(
                thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                "get %TypedArray%.prototype[@@toStringTag] needs a typed array receiver");
        }
        {
            kiln_typed_array *view = kiln_c99_typed_array_of(receiver);
            const char *name = view->is_buffer ? "Uint8Array" : kiln_c99_typed_name(view->kind);
            *result = kiln_c99_copy_text(name, strlen(name));
            return KILN_C99_OK;
        }
    }
#if KILN_RETAIN_GROUP_NODE_STREAM
    if ((groups & KILN_C99_GROUP_NODE_STREAM)) {
        return kiln_c99_node_stream(host, result, code, receiver, args, argc, depth, thrown);
    }
#endif
    /* The global `performance.now` and `require("perf_hooks").performance.now` are the same
     * function in the language, so they are the same code here -- as `parseInt` and
     * `Number.parseInt` already are.
     *
     * It reads `kiln_realm_now` directly rather than `kiln_c99_wall_clock_milliseconds`, which
     * `Date.now` uses. The two are the same clock in an ordinary unit and part company under
     * `KILN_REALM_HOST_CLOCK`: a host-backed `Date` is a *wall* clock, and a wall clock that the
     * operating system steps backwards would make an elapsed-time measurement negative. The
     * interpreter draws the line in the same place and for the same reason, so the two engines
     * agree in both kinds of realm. */
    if (code == KILN_C99_METHOD_PERFORMANCE_NOW) {
        double milliseconds;
        status = kiln_realm_performance_now(&milliseconds);
        if (status == KILN_C99_OK) *result = kiln_c99_number(milliseconds);
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_FS_EXISTS_SYNC) {
        if (argc > 0 && args != NULL) {
            kiln_value path_val = kiln_c99_text(args[0]);
            uint8_t *path_bytes = NULL;
            size_t path_len = 0;
            if (path_val.tag == KILN_VALUE_STRING &&
                kiln_c99_string_to_utf8_lossy(path_val, &path_bytes, &path_len) == KILN_C99_OK) {
                char stack_buf[1024];
                char *buf = stack_buf;
                if (path_len >= sizeof(stack_buf)) {
                    buf = (char *)malloc(path_len + 1);
                }
                if (buf != NULL) {
                    if (path_len != 0) memcpy(buf, path_bytes, path_len);
                    buf[path_len] = '\0';
                    *result = kiln_c99_boolean(access(buf, F_OK) == 0);
                    if (buf != stack_buf) free(buf);
                    free(path_bytes);
                    return KILN_C99_OK;
                }
                free(path_bytes);
            }
        }
        *result = kiln_c99_boolean(0);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_FS_READ_FILE ||
        code == KILN_C99_METHOD_NODE_FS_STAT ||
        code == KILN_C99_METHOD_NODE_FS_WRITE_FILE ||
        code == KILN_C99_METHOD_NODE_FS_MKDIR ||
        code == KILN_C99_METHOD_NODE_STAT_IS_FILE ||
        code == KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY) {
        return kiln_c99_node_fs(host, result, code, receiver, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_FS_READ_FILE_SYNC ||
        code == KILN_C99_METHOD_NODE_FS_WRITE_FILE_SYNC ||
        code == KILN_C99_METHOD_NODE_FS_MKDIR_SYNC ||
        code == KILN_C99_METHOD_NODE_FS_STAT_SYNC ||
        code == KILN_C99_METHOD_NODE_FS_LSTAT_SYNC ||
        code == KILN_C99_METHOD_NODE_FS_READDIR_SYNC ||
        code == KILN_C99_METHOD_NODE_FS_REALPATH_SYNC ||
        code == KILN_C99_METHOD_NODE_FS_UNLINK_SYNC ||
        code == KILN_C99_METHOD_NODE_FS_RMDIR_SYNC ||
        code == KILN_C99_METHOD_NODE_STAT_IS_SYMBOLIC_LINK) {
        return kiln_c99_node_fs_sync(host, result, code, receiver, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_PROCESS_START_RAW || code == KILN_C99_METHOD_PROCESS_CANCEL_RAW)
        return kiln_c99_node_process_raw(host, result, code, args, argc, depth, thrown);
    if (code == KILN_C99_METHOD_NODE_WORKER_CONSTRUCTOR) {
        return kiln_c99_throw_error(
            thrown, KILN_C99_BUILTIN_ERROR,
            "node:worker_threads requires an isolated realm worker capability");
    }
    if (code == KILN_C99_METHOD_NODE_MODULE_CREATE_REQUIRE) {
        /* The generated graph launcher intercepts this operation to capture the filename in the
         * returned callable. Keep the standalone fallback for realms without a graph host. */
        if (host != NULL && host->call != NULL) {
            if (argc != 1 || args == NULL) return KILN_C99_INVALID_ARGUMENT;
            return host->call(host->context, KILN_C99_HOST_MODULE_CREATE_REQUIRE,
                              (uint16_t)argc, args, result);
        }
        return kiln_c99_commonjs_require_value(result, kiln_c99_default_require, NULL);
    }
    /* The `Module` class `require("node:module")` hands back. It exists so that `typeof` and
     * `module.createRequire` answer -- which is the whole of what a program reads off it here --
     * and constructing a module record is not something this backend has: its graph is resolved
     * before execution, so there is no loader for a `new Module()` to be part of. Refusing names
     * that, where an object built to look like a module record would be a wrong answer a program
     * would carry three frames further. */
    if (code == KILN_C99_METHOD_NODE_MODULE) {
        KILN_C99_REFUSE("the node:module Module constructor, which this backend does not build");
    }
    if (code == KILN_C99_METHOD_PROCESS_NEXT_TICK) {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        if (!kiln_c99_is_callable(callback))
            return kiln_c99_throw_not_a(thrown, callback, "function");
        *result = kiln_c99_undefined();
        return kiln_c99_enqueue_next_tick(callback, argc > 1 ? args + 1 : NULL,
                                          argc > 1 ? argc - 1 : 0);
    }
    if (code == KILN_C99_METHOD_PROCESS_CWD) {
        char cwd_buf[4096];
        if (getcwd(cwd_buf, sizeof(cwd_buf)) != NULL) {
            *result = kiln_c99_copy_text(cwd_buf, strlen(cwd_buf));
        } else {
            *result = kiln_c99_copy_text("/", 1);
        }
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_PROCESS_KILL) {
        if (argc > 1 && kiln_c99_truthy(args[1])) {
            kiln_value ignored;
            kiln_value process_val;
            kiln_value signal = args[1];
#if KILN_RETAIN_GROUP_NODE_EVENT
            if (kiln_c99_process_object(&process_val) == KILN_C99_OK) {
                kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_EMIT,
                                    process_val, &signal, 1, depth, NULL);
            }
#else
            (void)process_val;
            (void)signal;
#endif
        }
        *result = kiln_c99_boolean(1);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_PROCESS_EXIT) {
        int exit_code = 0;
        if (argc > 0 && args[0].tag == KILN_VALUE_NUMBER) {
            exit_code = (int)kiln_c99_as_number(args[0]);
        }
        /* In a worker (`node:worker_threads`) the call ends that worker's realm, not the
         * process: the runtime records the code, and the host-error status -- which no `catch`
         * takes -- unwinds to the launcher that ends the realm with it. */
        if (kiln_worker_exit(exit_code) == 0) return KILN_C99_HOST_ERROR;
        fflush(NULL);
        exit(exit_code);
    }
    if (code == KILN_C99_METHOD_PROCESS_HRTIME ||
        code == KILN_C99_METHOD_PROCESS_HRTIME_BIGINT) {
        int64_t high;
        uint64_t low;
        status = kiln_realm_now_nanoseconds(&high, &low);
        if (status != KILN_C99_OK) return status;
        if (code == KILN_C99_METHOD_PROCESS_HRTIME_BIGINT) {
            /* A Duration reaches 2^64 nanoseconds only after roughly 584 years. Refuse that
             * presently unreachable width rather than silently truncate it; ordinary readings
             * become the BigInt this API promises instead of a Number that poisons arithmetic. */
            if (high != 0)
                KILN_C99_REFUSE("a process.hrtime.bigint reading wider than uint64");
            return kiln_c99_u64_bigint(low, 0, result);
        }
        {
            kiln_value parts;
            status = kiln_c99_new_array(&parts);
            if (status == KILN_C99_OK)
                status = kiln_c99_array_push(parts,
                                             kiln_c99_number((double)(low / 1000000000)));
            if (status == KILN_C99_OK)
                status = kiln_c99_array_push(parts,
                                             kiln_c99_number((double)(low % 1000000000)));
            if (status == KILN_C99_OK) *result = parts;
            return status;
        }
    }
    if (code == KILN_C99_METHOD_ATOMICS_UNAVAILABLE) {
        return kiln_c99_throw_error(
            thrown, KILN_C99_BUILTIN_ERROR,
            "Atomics requires native SharedArrayBuffer support");
    }
    /* A member a node builtin publishes and this backend has not built.
     *
     * It refuses when it is *called*, not when it is read, and the difference is the whole design.
     * `const { existsSync, readFileSync, realpathSync } = require("fs")` is the line every
     * CommonJS bundle writes, and it reads all three whether or not it ever calls one -- so a
     * refusal on the read stops a program at its first import for a function it was never going
     * to use. Answering the read with this, which is a function in both engines, leaves
     * `typeof fs.readFileSync === "function"` agreeing with the interpreter and puts the refusal
     * where the program actually asks for the behaviour.
     *
     * What it does not do is answer `undefined`. That was the state this replaced, and it is the
     * one wrong answer this backend is not allowed to give: a name that reads `undefined` fails a
     * feature test silently, and a missing `path.dirname` surfaced as "a undefined is not a
     * function" three frames away inside `postcss`, naming nothing.
     *
     * No message: the refusal is reported against the instruction that made the call, and that
     * instruction already carries the member's name -- `CallMethod(readFileSync)`. */
    if (code == KILN_C99_METHOD_NODE_BUILTIN_UNMATERIALIZED) {
        KILN_C99_REFUSE("a member this Node built-in publishes and this backend has not built");
    }
    /* `console.log` and `console.info` reached as values, and `console.error` and `console.warn`
     * with them. The same host operation the compiled `console.log(x)` shape uses, and the same
     * argument reduction in front of it: the embedding boundary carries only the scalar tags, so
     * anything else is reduced to the text a console shows for it -- which is what the interpreter
     * prints for one too.
     *
     * `error` and `warn` used to refuse here, on the grounds that they go to standard *error* in
     * the interpreter while the host ABI publishes one captured output operation, so printing them
     * to stdout would be the two engines disagreeing about which stream a line went to. That was
     * not true, and is worth stating plainly because the refusal outlived it: the interpreter
     * routes `ConsoleError` to `HostOp::WriteStdout`, the same operation as `ConsoleLog`, and says
     * so at its own call site -- "until stderr receives its own versioned op". Measured, all three
     * come out of the interpreter on stdout and nothing on stderr.
     *
     * So refusing was avoiding a disagreement that did not exist, and creating a real one: the
     * interpreter printed the line and this backend stopped the program. Using the same operation
     * is what makes the two engines agree, which is what the old comment wanted. Both still differ
     * from node, which sends these to stderr; that difference is the interpreter's, it predates
     * this, and it ends when the ABI grows a stderr op rather than here. */
    if (code == KILN_C99_METHOD_CONSOLE_LOG || code == KILN_C99_METHOD_CONSOLE_ERROR) {
        kiln_value inline_args[KILN_C99_COERCED_INLINE];
        kiln_value *reduced = NULL;
        kiln_value host_result;
        size_t index;
        int called;
        (void)receiver;
        if (argc > 0) {
            reduced = argc <= KILN_C99_COERCED_INLINE
                          ? inline_args
                          : (kiln_value *)malloc(argc * sizeof *reduced);
            if (reduced == NULL) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            for (index = 0; index < argc; ++index) {
                reduced[index] = kiln_c99_crosses_boundary(args[index])
                                     ? kiln_c99_console_inspect(host, args[index])
                                     : args[index];
            }
        }
        called = host != NULL && host->call != NULL &&
                 host->call(host->context,
                            code == KILN_C99_METHOD_CONSOLE_ERROR ? UINT16_C(6) : UINT16_C(1),
                            (uint16_t)argc, argc == 0 ? NULL : reduced, &host_result) == 0;
        if (reduced != NULL && reduced != inline_args) {
            free(reduced);
        }
        if (!called) {
            return KILN_C99_HOST_ERROR;
        }
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    /* Claimed here because the string range further down is open-ended at its top. */
#if KILN_RETAIN_GROUP_TEMPORAL
    if ((groups & KILN_C99_GROUP_TEMPORAL)) {
        return kiln_c99_temporal_method(host, result, code, receiver, args, argc, depth, thrown);
    }
#endif
    /* An intrinsic method reached *through* a proxy runs against the proxy: `push` on one reads
     * `length` and writes an element through the handler's traps, which is how a copy-on-write
     * layer sees the write at all. Every method here operates on the receiver's own storage
     * instead, so borrowing one onto a proxy would quietly bypass the handler. */
    if (receiver.tag == KILN_VALUE_PROXY) {
        if (code == KILN_C99_METHOD_FUNCTION_CALL || code == KILN_C99_METHOD_FUNCTION_APPLY ||
            code == KILN_C99_METHOD_FUNCTION_BIND ||
            code == KILN_C99_METHOD_FUNCTION_TO_STRING) {
            /* These four touch no storage at all: the first three hand the receiver back to
             * `kiln_c99_call`, which is where the `apply` trap lives, and `toString` reads the
             * target's own text through a conversion that follows the proxy itself. */
        } else if (code == KILN_C99_METHOD_HAS_OWN_PROPERTY) {
            /* `Object.prototype.hasOwnProperty.call(p, k)` asks `[[GetOwnProperty]]`, which is a
             * trap -- so it must not read a store the proxy does not have. */
            kiln_value descriptor;
            int found;
            status = kiln_c99_proxy_describe(host, receiver,
                                             argc > 0 ? args[0] : kiln_c99_undefined(),
                                             &descriptor, &found, depth, thrown);
            *result = kiln_c99_boolean(found);
            return status;
        } else if (code == KILN_C99_METHOD_PROXY_REVOKE) {
            /* Withdrawing the capability. Idempotent, because the program holds an ordinary
             * function and may call it twice. */
            kiln_c99_proxy_of(receiver)->revoked = 1;
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        } else if (code == KILN_C99_METHOD_PUSH) {
            /* `push` written out in terms of the operations a handler can see, which is what the
             * language actually specifies it as and the reason a copy-on-write layer hears about
             * the write at all: read `length`, set the element, set the new `length`. The array
             * implementation below reaches the receiver's own storage instead and would go
             * straight past the handler. */
            kiln_value length_value;
            double length;
            status = kiln_c99_get(host, receiver, "length", 6, &length_value, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_to_number_checked(host, length_value, &length, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (!(length >= 0.0)) {
                length = 0.0;
            }
            for (index = 0; index < argc; ++index) {
                status = kiln_c99_set_computed(host, receiver,
                                               kiln_c99_text(kiln_c99_number(length + (double)index)),
                                               args[index], depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = kiln_c99_number(length + (double)argc);
            return kiln_c99_put(host, receiver, "length", 6, *result, depth, thrown);
        } else {
            KILN_C99_REFUSE("Array.prototype.push on a receiver that is not an array or an "
                            "array-like");
        }
    }
    if (receiver.tag == KILN_VALUE_STRING &&
        (code == KILN_C99_METHOD_STRING_MATCH || code == KILN_C99_METHOD_STRING_MATCH_ALL ||
         code == KILN_C99_METHOD_STRING_SEARCH ||
         ((code == KILN_C99_METHOD_REPLACE || code == KILN_C99_METHOD_REPLACE_ALL ||
           code == KILN_C99_METHOD_SPLIT) &&
          argc > 0 && args[0].tag == KILN_VALUE_REGEXP) ||
         ((code == KILN_C99_METHOD_REPLACE || code == KILN_C99_METHOD_REPLACE_ALL) && argc > 1 &&
          kiln_c99_is_callable(args[1])))) {
        if (code == KILN_C99_METHOD_SPLIT) {
            return kiln_c99_string_pattern_split(host, result, receiver, args, argc, depth, thrown);
        }
        return kiln_c99_string_pattern(host, result, code, receiver, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_ERROR_TO_STRING) {
        return kiln_c99_error_to_string(host, result, receiver, depth, thrown);
    }
    if (code == KILN_C99_METHOD_TYPED_OF || code == KILN_C99_METHOD_TYPED_FROM) {
        /* `Uint8Array.of(...)` and `Uint8Array.from(source, mapper)`. The kind comes from the
         * receiver -- the constructor the call named -- and every value goes through that
         * kind's conversion, which is the whole difference from `Array.of` followed by a
         * copy: `Uint8Array.of(-1)` is 255 and `Array.of(-1)` is -1.
         *
         * Gathered into a plain array first because `from` takes anything iterable and the
         * length is not known until the walk is over; `of` fills the same array from its
         * arguments so the conversion below is written once. */
        kiln_typed_kind kind;
        kiln_value gathered;
        kiln_array *items;
        kiln_value mapper = code == KILN_C99_METHOD_TYPED_FROM && argc > 1
                                ? args[1]
                                : kiln_c99_undefined();
        if (receiver.tag != KILN_VALUE_BUILTIN ||
            !kiln_c99_builtin_typed_kind(receiver.bits, &kind)) {
            KILN_C99_REFUSE("a typed-array static called on something other than a typed-array "
                            "constructor");
        }
        status = kiln_c99_new_array(&gathered);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (code == KILN_C99_METHOD_TYPED_OF) {
            for (index = 0; index < argc; ++index) {
                status = kiln_c99_array_push(gathered, args[index]);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
        } else {
            status = kiln_c99_iterate(host, gathered,
                                      argc > 0 ? args[0] : kiln_c99_undefined(), depth,
                                      thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        items = kiln_c99_array_of(gathered);
        status = kiln_c99_typed_alloc(result, kind, items->count);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < items->count; ++index) {
            kiln_value element = kiln_val_to_boundary(items->elements[index]);
            double converted;
            kiln_typed_array *view = kiln_c99_typed_array_of(*result);
            if (kiln_c99_is_callable(mapper)) {
                kiln_value visit[2];
                visit[0] = element;
                visit[1] = kiln_c99_number((double)index);
                status = kiln_c99_call(host, &element, mapper, kiln_c99_undefined(), visit, 2,
                                       depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            if (kiln_c99_typed_kind_is_bigint(kind)) {
                status = kiln_c99_typed_store(host, view, index, element, depth, thrown);
                if (status != KILN_C99_OK) return status;
                continue;
            }
            status = kiln_c99_to_number_checked(host, element, &converted, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            kiln_c99_typed_set(view, index, converted);
        }
        return KILN_C99_OK;
    }
    if ((groups & KILN_C99_GROUP_GENERIC)) {
        /* A namespace method takes no receiver, so `Math` contributes nothing beyond having been
         * the way the function was reached. */
        double first = 0.0 / 0.0;
        double second = 0.0 / 0.0;
        /* Only the methods that actually want a number reduce their arguments. `Object.assign`,
         * `JSON.stringify` and `Function.prototype.call` are reached through this same range, and
         * `call` in particular takes a *receiver* in `args[0]` -- running `valueOf` on it would be
         * a user-visible call the language never asks for. Coercing every argument here was safe
         * only while `kiln_c99_as_number` was pure; now that it can reach a program's `valueOf`,
         * the set has to be named. The global `isNaN`/`isFinite` are in it and the `Number` pair
         * beside them is not, for the reason given at their cases below. */
        /* `min`, `max` and `hypot` are variadic and reduce each argument themselves, so they must
         * not be reduced here as well: a `valueOf` that counts its own calls sees the difference,
         * and ToNumber is specified to happen once per argument. */
#if KILN_RETAIN_GROUP_MATH_NUMBER
        if ((groups & KILN_C99_GROUP_MATH_NUMBER) ||
            code == KILN_C99_METHOD_GLOBAL_IS_NAN || code == KILN_C99_METHOD_GLOBAL_IS_FINITE) {
#else
        if (code == KILN_C99_METHOD_GLOBAL_IS_NAN || code == KILN_C99_METHOD_GLOBAL_IS_FINITE) {
#endif
            if (argc > 0) {
                int reduced = kiln_c99_to_number_checked(host, args[0], &first, depth, thrown);
                if (reduced != KILN_C99_OK) {
                    return reduced;
                }
            }
            if (argc > 1) {
                int reduced = kiln_c99_to_number_checked(host, args[1], &second, depth, thrown);
                if (reduced != KILN_C99_OK) {
                    return reduced;
                }
            }
        } else {
            first = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0 / 0.0;
            second = argc > 1 ? kiln_c99_as_number(args[1]) : 0.0 / 0.0;
        }
        switch (code) {
        case KILN_C99_METHOD_FUNCTION_CALL:
            /* The receiver is the function; the first argument becomes *its* receiver. */
            return kiln_c99_call(host, result, receiver,
                                 argc > 0 ? args[0] : kiln_c99_undefined(),
                                 argc > 1 ? args + 1 : NULL, argc > 1 ? argc - 1 : 0, depth,
                                 thrown);
        case KILN_C99_METHOD_FUNCTION_APPLY: {
            /* `CreateListFromArrayLike`, and the *array-like* is the point: `f.apply(this,
             * arguments)` is how a wrapper forwards a call, and an `arguments` object is an
             * ordinary object with a `length`, not an array. This tested for the array tag alone
             * and quietly called the function with **no arguments at all** for everything else --
             * a silent wrong answer of the kind ADR 0042 forbids, and the one a bundled `immer`
             * dies of: its array proxy traps are `function() { return fn.apply(this, arguments) }`
             * and every trap therefore received `undefined` for the state it was handed.
             *
             * An array still takes the direct path, because that is the spelling that is hot and
             * its elements are already a contiguous list. */
            kiln_value list = argc > 1 ? args[1] : kiln_c99_undefined();
            kiln_array *given = NULL;
            kiln_value gathered;
            if (list.tag == KILN_VALUE_ARRAY) {
                given = kiln_c99_array_of(list);
                /* A hole is heap storage, never a JavaScript argument. It may also read an
                 * inherited indexed property, so sparse arrays take the ordinary Get path. */
                if (given->properties.tag == KILN_VALUE_OBJECT) {
                    given = NULL;
                } else {
                    for (size_t at = 0; at < given->count; ++at) {
                        if (given->elements[at] == KILN_VAL_HOLE) {
                            given = NULL;
                            break;
                        }
                    }
                }
            }
            if (given == NULL && list.tag != KILN_VALUE_UNDEFINED && list.tag != KILN_VALUE_NULL) {
                kiln_value length_value;
                double length;
                size_t at;
                status = kiln_c99_get(host, list, "length", 6, &length_value, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                status = kiln_c99_to_number_checked(host, length_value, &length, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                /* NaN and everything below one are no arguments at all. The upper bound is not
                 * the language's -- it has none -- but a list this long is a `length` that was
                 * never meant as one, and refusing names that rather than trying to build it. */
                if (!(length >= 1.0)) {
                    length = 0.0;
                }
                if (length > 1000000.0) {
                    KILN_C99_REFUSE("Function.prototype.apply was given a length no argument list "
                                    "could have");
                }
                status = kiln_c99_new_array(&gathered);
                if (status != KILN_C99_OK) {
                    return status;
                }
                for (at = 0; at < (size_t)length; ++at) {
                    kiln_value element;
                    status = kiln_c99_get_computed(host, list, kiln_c99_number((double)at),
                                                   &element, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                    status = kiln_c99_array_push(gathered, element);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                given = kiln_c99_array_of(gathered);
            }
            kiln_val call_result_val = KILN_VAL_UNDEFINED;
            status = kiln_c99_call_val(host, &call_result_val, kiln_val_from_boundary(receiver),
                                           argc > 0 ? kiln_val_from_boundary(args[0]) : KILN_VAL_UNDEFINED,
                                           given == NULL || given->count == 0 ? NULL : given->elements,
                                           given == NULL ? 0 : given->count, depth, thrown);
            if (status == KILN_C99_OK) *result = kiln_val_to_boundary(call_result_val);
            return status;
        }
        case KILN_C99_METHOD_FUNCTION_BIND: {
            kiln_value prototype, store;
            status = kiln_c99_invoke_method(host, &prototype, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                                            kiln_c99_undefined(), &receiver, 1, depth, thrown);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_bind_with(result, receiver,
                                      argc > 0 ? args[0] : kiln_c99_undefined(),
                                      argc > 1 ? args + 1 : NULL, argc > 1 ? argc - 1 : 0);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_callable_store(*result, &store, 1);
            if (status == KILN_C99_OK) status = kiln_c99_set_prototype(store, prototype);
            return status;
        }
        case KILN_C99_METHOD_FUNCTION_TO_STRING:
            /* The same conversion `String(f)` and `"" + f` already went through, so the three
             * spellings cannot disagree with each other or with the interpreter. */
            *result = kiln_c99_text(receiver);
            if (result->tag != KILN_VALUE_STRING) {
                KILN_C99_REFUSE("Function.prototype.toString has no text to answer");
            }
            return KILN_C99_OK;
        case KILN_C99_METHOD_ARRAY_BUFFER_IS_VIEW:
            /* Two tags now rather than one: a typed view (Node's Buffer included, which shares
             * that tag) and a DataView, which is the other thing the language calls a view. The
             * predicate stays on representation rather than enumerating constructors, so a
             * subclass and a `Buffer` answer the same as the view they are. */
            *result = kiln_c99_boolean(argc > 0 && (args[0].tag == KILN_VALUE_TYPED_ARRAY ||
                                                    args[0].tag == KILN_VALUE_DATA_VIEW));
            return KILN_C99_OK;
        case KILN_C99_METHOD_NUMBER_VALUE_OF:
            /* The three below are the exception to the comment above: they are reached through a
             * number rather than through a namespace, so the receiver is the whole subject. */
            if (receiver.tag == KILN_VALUE_BIGINT) {
                /* `valueOf` hands back the primitive it was called on, whichever type it is. */
                *result = receiver;
                return KILN_C99_OK;
            }
            *result = kiln_c99_number(kiln_c99_as_number(receiver));
            return KILN_C99_OK;
        case KILN_C99_METHOD_NUMBER_TO_STRING: {
            double subject;
            if (receiver.tag == KILN_VALUE_BIGINT) {
                double base = argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED ? first : 10.0;
                if (!(base >= 2.0 && base <= 36.0)) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                "toString() radix must be between 2 and 36");
                }
                *result = kiln_c99_bigint_text(receiver, (uint32_t)base);
                return KILN_C99_OK;
            }
            subject = kiln_c99_as_number(receiver);
            double radix = argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED ? first : 10.0;
            if (radix == 10.0) {
                /* Radix 10 is exactly `String(n)`, exponent rules included. */
                *result = kiln_c99_text(kiln_c99_number(subject));
                return KILN_C99_OK;
            }
            if (!(radix >= 2.0 && radix <= 36.0)) {
                /* A RangeError rather than a NaN, which would read as a result. */
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "toString() radix must be between 2 and 36");
            }
            return kiln_c99_number_in_radix(result, subject, (int)radix);
        }
        case KILN_C99_METHOD_NUMBER_TO_FIXED: {
            /* `ToIntegerOrInfinity`: `undefined` and NaN both mean zero digits, so
             * `(1.5).toFixed(undefined)` is `(1.5).toFixed()` and not a RangeError. */
            double digits = argc > 0 ? first : 0.0;
            if (digits != digits) digits = 0.0;
            digits = trunc(digits);
            if (!(digits >= 0.0 && digits <= 100.0)) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "toString() radix must be between 2 and 36");
            }
            return kiln_c99_to_fixed(result, kiln_c99_as_number(receiver), (int)digits);
        }
        case KILN_C99_METHOD_NUMBER_TO_EXPONENTIAL: {
            double subject = kiln_c99_as_number(receiver);
            char mantissa[128];
            int exponent;
            int given = argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED;
            double requested = 0.0;
            if (given) {
                /* `ToIntegerOrInfinity`, so NaN means zero digits rather than a refusal. */
                requested = first;
                if (requested != requested) requested = 0.0;
                requested = trunc(requested);
                /* The range check runs even for a value that is not finite, which is why an
                 * infinite receiver is answered *after* it rather than before it. */
                if (!(requested >= 0.0 && requested <= 100.0) && isfinite(subject)) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                "toString() radix must be between 2 and 36");
                }
                if (requested < 0.0) requested = 0.0;
                if (requested > 100.0) requested = 100.0;
            }
            if (!isfinite(subject)) {
                *result = kiln_c99_text(kiln_c99_number(subject));
                return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
            }
            if (given) {
                kiln_c99_significant_digits(fabs(subject), (size_t)requested + 1, mantissa,
                                            &exponent);
            } else {
                /* With no digit count the specification asks for the fewest digits that name the
                 * value exactly, which the number provider's shortest spelling already is. */
                status = kiln_c99_shortest_digits(fabs(subject), mantissa, sizeof mantissa,
                                                  &exponent);
                if (status != KILN_C99_OK) return status;
            }
            return kiln_c99_exponential_text(result, subject < 0.0, mantissa, exponent);
        }
        case KILN_C99_METHOD_NUMBER_TO_PRECISION: {
            double subject = kiln_c99_as_number(receiver);
            char mantissa[128];
            int exponent;
            double requested;
            /* With no argument at all this is `toString`, not a one-digit rendering. */
            if (argc == 0 || args[0].tag == KILN_VALUE_UNDEFINED || !isfinite(subject)) {
                *result = kiln_c99_text(kiln_c99_number(subject));
                return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
            }
            requested = first;
            if (requested != requested) requested = 0.0;
            requested = trunc(requested);
            /* One digit is the fewest a significant-digit rendering can have, which is why this
             * range starts at one where `toExponential`'s starts at zero. */
            if (!(requested >= 1.0 && requested <= 100.0)) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "toString() radix must be between 2 and 36");
            }
            kiln_c99_significant_digits(fabs(subject), (size_t)requested, mantissa, &exponent);
            return kiln_c99_precision_text(result, subject < 0.0, mantissa, (size_t)requested,
                                           exponent);
        }
        case KILN_C99_METHOD_ABORT_CONTROLLER_ABORT: {
            kiln_value signal;
            status = kiln_c99_get(host, receiver, "signal", 6, &signal, depth, thrown);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_abort(host, signal,
                                    argc > 0 ? args[0] : kiln_c99_undefined(), depth, thrown);
            if (status == KILN_C99_OK) *result = kiln_c99_undefined();
            return status;
        }
        case KILN_C99_METHOD_ABORT_SIGNAL_ABORT: {
            status = kiln_c99_new_abort_signal(result);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_abort(host, *result,
                                    argc > 0 ? args[0] : kiln_c99_undefined(), depth, thrown);
            return status;
        }
        case KILN_C99_METHOD_ABORT_SIGNAL_LISTEN:
        case KILN_C99_METHOD_ABORT_SIGNAL_UNLISTEN: {
            kiln_value listeners;
            kiln_value listener = argc > 1 ? args[1] : kiln_c99_undefined();
            size_t index;
            if (argc == 0 || args[0].tag != KILN_VALUE_STRING ||
                !kiln_c99_string_equals_ascii(args[0], "abort", 5)) {
                *result = kiln_c99_undefined();
                return KILN_C99_OK;
            }
            status = kiln_c99_get(host, receiver, "@@abort-listeners", 17, &listeners, depth,
                                  thrown);
            if (status != KILN_C99_OK || listeners.tag != KILN_VALUE_ARRAY) return status;
            if (code == KILN_C99_METHOD_ABORT_SIGNAL_UNLISTEN) {
                kiln_array *held = kiln_c99_array_of(listeners);
                size_t write = 0;
                for (index = 0; index < held->count; ++index) {
                    if (!kiln_c99_strict_equal(kiln_val_to_boundary(held->elements[index]), listener))
                        kiln_c99_heap_store_val(held, &held->elements[write++], held->elements[index]);
                }
                held->count = write;
            } else if (kiln_c99_is_callable(listener)) {
                kiln_array *held = kiln_c99_array_of(listeners);
                for (index = 0; index < held->count; ++index)
                    if (kiln_c99_strict_equal(kiln_val_to_boundary(held->elements[index]), listener)) break;
                if (index == held->count) {
                    status = kiln_c99_array_push(listeners, listener);
                    if (status != KILN_C99_OK) return status;
                }
            }
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ABORT_SIGNAL_THROW: {
            kiln_value aborted;
            status = kiln_c99_get(host, receiver, "aborted", 7, &aborted, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (kiln_c99_truthy(aborted)) {
                status = kiln_c99_get(host, receiver, "reason", 6, thrown, depth, thrown);
                return status == KILN_C99_OK ? KILN_C99_THROWN : status;
            }
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ABORT_SIGNAL_SETTLE: {
            status = kiln_c99_abort(host, receiver,
                                    argc > 0 ? args[0] : kiln_c99_undefined(), depth, thrown);
            if (status == KILN_C99_OK) *result = kiln_c99_undefined();
            return status;
        }
        case KILN_C99_METHOD_ABORT_SIGNAL_FOLLOW: {
            kiln_value reason;
            kiln_value source = argc > 0 ? args[0] : kiln_c99_undefined();
            status = kiln_c99_get(host, source, "reason", 6, &reason, depth, thrown);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_abort(host, receiver, reason, depth, thrown);
            if (status == KILN_C99_OK) *result = kiln_c99_undefined();
            return status;
        }
        case KILN_C99_METHOD_ABORT_SIGNAL_TIMEOUT: {
            kiln_timer *entry;
            kiln_value reason;
            kiln_value callback;
            double delay = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
            uint64_t id = 0;
            status = kiln_c99_new_abort_signal(result);
            if (status == KILN_C99_OK)
                status = kiln_c99_new_error(&reason, KILN_C99_BUILTIN_ERROR,
                                            kiln_c99_copy_text(
                                                "The operation was aborted due to timeout", 40));
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, reason, "name", 4,
                                      kiln_c99_copy_text("TimeoutError", 12), 0, NULL);
            if (status == KILN_C99_OK)
                status = kiln_c99_bind(&callback,
                                       kiln_c99_method(KILN_C99_METHOD_ABORT_SIGNAL_SETTLE),
                                       *result);
            if (status != KILN_C99_OK) return status;
            entry = (kiln_timer *)kiln_c99_gc_alloc(KILN_GC_TIMER, sizeof *entry);
            if (entry == NULL) return KILN_C99_OUT_OF_MEMORY;
            kiln_c99_heap_store_value(entry, &entry->callback, callback);
            entry->argc = 1;
            entry->args = (kiln_value *)malloc(sizeof *entry->args);
            if (entry->args == NULL) {
                kiln_c99_gc_release(entry);
                return KILN_C99_OUT_OF_MEMORY;
            }
            kiln_c99_heap_store_value(entry, &entry->args[0], reason);
            if (!(isfinite(delay) && delay > 0.0)) delay = 0.0;
            status = kiln_realm_set_timeout(delay, kiln_c99_run_timer, kiln_c99_drop_timer,
                                            entry, &id);
            if (status != KILN_C99_OK) kiln_c99_drop_timer(entry);
            return status;
        }
        case KILN_C99_METHOD_ABORT_SIGNAL_ANY: {
            kiln_value sources;
            kiln_array *items;
            status = kiln_c99_new_abort_signal(result);
            if (status == KILN_C99_OK) status = kiln_c99_new_array(&sources);
            if (status == KILN_C99_OK)
                status = kiln_c99_iterate(host, sources,
                                          argc > 0 ? args[0] : kiln_c99_undefined(), depth,
                                          thrown);
            if (status != KILN_C99_OK) return status;
            items = kiln_c99_array_of(sources);
            for (index = 0; index < items->count; ++index) {
                kiln_value aborted;
                kiln_value reason;
                kiln_value follower;
                kiln_value listen;
                kiln_value listen_args[2];
                kiln_value item_val = kiln_val_to_boundary(items->elements[index]);
                status = kiln_c99_get(host, item_val, "aborted", 7, &aborted,
                                      depth, thrown);
                if (status != KILN_C99_OK) return status;
                if (kiln_c99_truthy(aborted)) {
                    status = kiln_c99_get(host, item_val, "reason", 6, &reason,
                                          depth, thrown);
                    if (status != KILN_C99_OK) return status;
                    return kiln_c99_abort(host, *result, reason, depth, thrown);
                }
                status = kiln_c99_bind_with(&follower,
                                            kiln_c99_method(KILN_C99_METHOD_ABORT_SIGNAL_FOLLOW),
                                            *result, &item_val, 1);
                if (status == KILN_C99_OK)
                    status = kiln_c99_get(host, item_val, "addEventListener", 16,
                                          &listen, depth, thrown);
                listen_args[0] = kiln_c99_copy_text("abort", 5);
                listen_args[1] = follower;
                if (status == KILN_C99_OK)
                    status = kiln_c99_call(host, &reason, listen, item_val,
                                           listen_args, 2, depth, thrown);
                if (status != KILN_C99_OK) return status;
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_TEXT_ENCODER_ENCODE: {
            kiln_value text = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_from_units(NULL, 0);
            uint8_t *encoded = NULL;
            size_t encoded_len = 0;
            size_t byte;
            status = kiln_c99_string_to_utf8_lossy(text, &encoded, &encoded_len);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_typed_alloc(result, KILN_TYPED_UINT8, encoded_len);
            if (status != KILN_C99_OK) {
                free(encoded);
                return status;
            }
            for (byte = 0; byte < encoded_len; ++byte)
                kiln_c99_typed_set(kiln_c99_typed_array_of(*result), byte,
                                   (double)encoded[byte]);
            free(encoded);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_TEXT_DECODER_DECODE: {
            if (argc == 0 || args[0].tag == KILN_VALUE_UNDEFINED) {
                *result = kiln_c99_from_units(NULL, 0);
                return KILN_C99_OK;
            }
            if (args[0].tag == KILN_VALUE_TYPED_ARRAY) {
                kiln_typed_array *ta = kiln_c99_typed_array_of(args[0]);
                if (ta == NULL || ta->buffer == NULL || ta->buffer->bytes == NULL) {
                    *result = kiln_c99_from_units(NULL, 0);
                    return KILN_C99_OK;
                }
                size_t byte_len = ta->length * kiln_c99_typed_width(ta->kind);
                if (ta->byte_offset + byte_len > ta->buffer->byte_length) {
                    byte_len = ta->buffer->byte_length > ta->byte_offset ? ta->buffer->byte_length - ta->byte_offset : 0;
                }
                const char *bytes = (const char *)(ta->buffer->bytes + ta->byte_offset);
                *result = kiln_c99_copy_text(bytes, byte_len);
                return KILN_C99_OK;
            }
            if (args[0].tag == KILN_VALUE_ARRAY_BUFFER) {
                kiln_typed_buffer *buf = (kiln_typed_buffer *)(uintptr_t)args[0].bits;
                if (buf == NULL || buf->bytes == NULL) {
                    *result = kiln_c99_from_units(NULL, 0);
                    return KILN_C99_OK;
                }
                *result = kiln_c99_copy_text((const char *)buf->bytes, buf->byte_length);
                return KILN_C99_OK;
            }
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "The provided value is not of type ArrayBuffer or ArrayBufferView");
        }
        case KILN_C99_METHOD_SET_TIMEOUT:
        case KILN_C99_METHOD_SET_INTERVAL:
        case KILN_C99_METHOD_SET_IMMEDIATE:
        case KILN_C99_METHOD_QUEUE_IO_RAW: {
            kiln_timer *entry;
            double delay = 0.0;
            /* An immediate and an I/O callback (`__kilnQueueIo`) have no delay, so their
             * arguments start one earlier. */
            int undelayed =
                code == KILN_C99_METHOD_SET_IMMEDIATE || code == KILN_C99_METHOD_QUEUE_IO_RAW;
            /* `undefined` stays 0, which the realm makes node's silent 1 ms, as it does every
             * delay it does not take (`Realm::node_timer_delay`). */
            if (!undelayed && argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
                status = kiln_c99_to_number_checked(host, args[1], &delay, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            uint64_t id = 0;
            {
                /* No arguments past the delay, and a heap function: the function is the work
                 * (see `kiln_c99_timer_direct_run`). */
                size_t extra = undelayed ? (argc > 1 ? argc - 1 : 0) : (argc > 2 ? argc - 2 : 0);
                void *work = NULL;
                int (*direct)(void *, void *, void *) =
                    extra == 0 && argc > 0 ? kiln_c99_timer_direct_run(args[0], &work) : NULL;
                if (direct != NULL) {
                    status = kiln_c99_arm_timer(code, delay, direct, kiln_c99_drop_timer_value,
                                                work, &id);
                    if (status != KILN_C99_OK) return status;
                    *result = kiln_c99_number((double)id);
                    return KILN_C99_OK;
                }
            }
            entry = (kiln_timer *)kiln_c99_gc_alloc(KILN_GC_TIMER, sizeof *entry);
            if (entry == NULL) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            kiln_c99_heap_store_value(entry, &entry->callback, argc > 0 ? args[0] : kiln_c99_undefined());
            /* Everything after the delay is handed to the callback, which is how a timer carries
             * state without a closure. */
            entry->argc = undelayed ? (argc > 1 ? argc - 1 : 0) : (argc > 2 ? argc - 2 : 0);
            entry->args = NULL;
            if (entry->argc != 0) {
                entry->args = (kiln_value *)malloc(entry->argc * sizeof *entry->args);
                if (entry->args == NULL) {
                    kiln_c99_gc_release(entry);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                for (size_t index = 0; index < entry->argc; ++index)
                    kiln_c99_heap_store_value(entry, &entry->args[index],
                                              args[index + (undelayed ? 1 : 2)]);
            }
            status = kiln_c99_arm_timer(code, delay, kiln_c99_run_timer, kiln_c99_drop_timer,
                                        entry, &id);
            if (status != KILN_C99_OK) {
                kiln_c99_drop_timer(entry);
                return status;
            }
            *result = kiln_c99_number((double)id);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_CLEAR_TIMEOUT:
        case KILN_C99_METHOD_CLEAR_INTERVAL:
        case KILN_C99_METHOD_CLEAR_IMMEDIATE: {
            double id = 0.0 / 0.0;
            if (argc > 0) {
                status = kiln_c99_to_number_checked(host, args[0], &id, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            if (isfinite(id) && id >= 1.0 && floor(id) == id &&
                id <= 9007199254740991.0) {
                status = kiln_realm_clear_timer((uint64_t)id);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_QUEUE_MICROTASK: {
            /* The same queue a promise reaction goes on, which is the whole point of the
             * function: it is how a program gets in line behind the reactions already there
             * rather than in front of them. */
            if (argc == 0 || !kiln_c99_is_callable(args[0])) {
                return kiln_c99_throw_not_a(thrown, argc == 0 ? kiln_c99_undefined() : args[0],
                                            "function");
            }
            *result = kiln_c99_undefined();
            return kiln_c99_enqueue(args[0], kiln_c99_undefined(), kiln_c99_undefined(), 0, 0);
        }
        case KILN_C99_METHOD_ESCAPE:
        case KILN_C99_METHOD_UNESCAPE: {
            /* Annex B's pre-Unicode pair, which real code still reaches for:
             * `unescape(encodeURIComponent(s))` is how a library gets a string's UTF-8 bytes one
             * per code unit. They work in code units rather than scalars, which is exactly why
             * they were replaced and exactly why they cannot be written in terms of the four
             * below. */
            /* `ToString` of the argument -- for an object a call the program wrote, and one
             * that may therefore throw -- has already run in the coercion pass on the way in,
             * which is where every other built-in's text argument is converted too. */
            kiln_value subject = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            kiln_text built = {NULL, 0, 0};
            uint16_t *units = NULL;
            size_t count = 0;
            size_t at = 0;
            int failed = 0;
            if (subject.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_to_units(subject, &units, &count) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (code == KILN_C99_METHOD_ESCAPE) {
                for (at = 0; at < count && !failed; ++at) {
                    uint16_t unit = units[at];
                    char character = (char)unit;
                    if (unit < 128 &&
                        (isalnum((unsigned char)character) || character == '@' ||
                         character == '*' || character == '_' || character == '+' ||
                         character == '-' || character == '.' || character == '/')) {
                        failed = kiln_c99_text_append(&built, &character, 1) != KILN_C99_OK;
                        continue;
                    }
                    /* Two hex digits below 256, four with a `%u` prefix above it. That split is
                     * the whole reason `escape` was replaced. */
                    failed = kiln_c99_text_append_c(&built, unit < 256 ? "%" : "%u") !=
                                 KILN_C99_OK ||
                             kiln_c99_append_hex(&built, unit, unit < 256 ? 2 : 4) != KILN_C99_OK;
                }
                free(units);
                if (failed) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                *result = kiln_c99_text_value(&built);
                return KILN_C99_OK;
            }
            {
                uint16_t *decoded = (uint16_t *)malloc((count == 0 ? 1 : count) * sizeof *decoded);
                size_t written = 0;
                if (decoded == NULL) {
                    free(units);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                while (at < count) {
                    int high;
                    int low;
                    if (units[at] != (uint16_t)'%') {
                        decoded[written++] = units[at++];
                        continue;
                    }
                    if (at + 5 < count && units[at + 1] == (uint16_t)'u') {
                        int digit;
                        uint32_t value = 0;
                        int usable = 1;
                        for (digit = 0; digit < 4 && usable; ++digit) {
                            uint16_t unit = units[at + 2 + (size_t)digit];
                            int nibble = unit < 128 ? kiln_c99_hex_digit((char)unit) : -1;
                            usable = nibble >= 0;
                            value = (value << 4) | (uint32_t)(nibble < 0 ? 0 : nibble);
                        }
                        if (usable) {
                            decoded[written++] = (uint16_t)value;
                            at += 6;
                            continue;
                        }
                    }
                    high = at + 2 < count && units[at + 1] < 128
                               ? kiln_c99_hex_digit((char)units[at + 1])
                               : -1;
                    low = at + 2 < count && units[at + 2] < 128
                              ? kiln_c99_hex_digit((char)units[at + 2])
                              : -1;
                    if (high >= 0 && low >= 0) {
                        decoded[written++] = (uint16_t)(high * 16 + low);
                        at += 3;
                        continue;
                    }
                    /* A `%` that does not introduce an escape stands for itself, unlike
                     * `decodeURI` where the same text is an error. */
                    decoded[written++] = units[at++];
                }
                free(units);
                *result = kiln_c99_from_units(decoded, written);
                free(decoded);
                return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
            }
        }
        case KILN_C99_METHOD_ENCODE_URI:
        case KILN_C99_METHOD_ENCODE_URI_COMPONENT: {
            /* The four URI functions differ only in which characters they leave alone.
             * `encodeURI` keeps the ones that structure a URI, because it is encoding a whole
             * one; `encodeURIComponent` encodes those too, because a component containing a `/`
             * must not look like a path separator. */
            kiln_value subject = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            kiln_text built = {NULL, 0, 0};
            uint8_t *encoded = NULL;
            size_t encoded_len = 0;
            size_t at;
            if (subject.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_string_to_utf8_lossy(subject, &encoded, &encoded_len) != KILN_C99_OK)
                return KILN_C99_OUT_OF_MEMORY;
            for (at = 0; at < encoded_len; ++at) {
                char character = (char)encoded[at];
                unsigned char byte = (unsigned char)character;
                int unreserved =
                    byte < 128 &&
                    (isalnum(byte) || character == '-' || character == '_' || character == '.' ||
                     character == '!' || character == '~' || character == '*' ||
                     character == '\'' || character == '(' || character == ')');
                int reserved = byte < 128 && (character == ';' || character == '/' ||
                                              character == '?' || character == ':' ||
                                              character == '@' || character == '&' ||
                                              character == '=' || character == '+' ||
                                              character == '$' || character == ',' ||
                                              character == '#');
                int ok;
                if (unreserved || (code == KILN_C99_METHOD_ENCODE_URI && reserved)) {
                    ok = kiln_c99_text_append(&built, &character, 1) == KILN_C99_OK;
                } else {
                    /* A non-ASCII byte is never in either set, so walking bytes rather than
                     * scalars percent-encodes each byte of a multi-byte character, which is what
                     * the specification says to do. */
                    ok = kiln_c99_text_append_c(&built, "%") == KILN_C99_OK &&
                         kiln_c99_append_hex(&built, byte, 2) == KILN_C99_OK;
                }
                if (!ok) {
                    free(built.bytes);
                    free(encoded);
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            *result = kiln_c99_text_value(&built);
            free(encoded);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_DECODE_URI:
        case KILN_C99_METHOD_DECODE_URI_COMPONENT: {
            kiln_value subject = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            kiln_text built = {NULL, 0, 0};
            uint8_t *encoded = NULL;
            size_t encoded_len = 0;
            size_t at = 0;
            if (subject.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_string_to_utf8_lossy(subject, &encoded, &encoded_len) != KILN_C99_OK)
                return KILN_C99_OUT_OF_MEMORY;
            while (at < encoded_len) {
                int high;
                int low;
                char byte;
                if (encoded[at] != '%') {
                    if (kiln_c99_text_append(&built, (const char *)encoded + at, 1) != KILN_C99_OK) {
                        free(built.bytes);
                        free(encoded);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    at += 1;
                    continue;
                }
                high = at + 2 < encoded_len ? kiln_c99_hex_digit((char)encoded[at + 1]) : -1;
                low = at + 2 < encoded_len ? kiln_c99_hex_digit((char)encoded[at + 2]) : -1;
                if (high < 0 || low < 0) {
                    /* A `%` not followed by two hex digits is malformed, not a literal percent.
                     * `URIError` rather than `TypeError`: that kind exists for exactly this, and
                     * it is what a program guarding `decodeURI` on user input catches. */
                    free(built.bytes);
                    free(encoded);
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_URI_ERROR,
                                                "URI malformed");
                }
                byte = (char)(high * 16 + low);
                /* `decodeURI` leaves the reserved set encoded: undoing it would change what the
                 * URI means, which is the one thing decoding must not do. */
                if (code == KILN_C99_METHOD_DECODE_URI &&
                    (byte == ';' || byte == '/' || byte == '?' || byte == ':' || byte == '@' ||
                     byte == '&' || byte == '=' || byte == '+' || byte == '$' || byte == ',' ||
                     byte == '#')) {
                    if (kiln_c99_text_append(&built, (const char *)encoded + at, 3) != KILN_C99_OK) {
                        free(built.bytes);
                        free(encoded);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                } else if (kiln_c99_text_append(&built, &byte, 1) != KILN_C99_OK) {
                    free(built.bytes);
                    free(encoded);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                at += 3;
            }
            if (!kiln_c99_valid_utf8(built.bytes, built.len)) {
                /* The escapes named bytes that are not a string. */
                free(built.bytes);
                free(encoded);
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_URI_ERROR, "URI malformed");
            }
            *result = kiln_c99_text_value(&built);
            free(encoded);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_GLOBAL_IS_NAN:
            /* The global pair coerces where the `Number` pair does not: `isNaN('x')` is true and
             * `Number.isNaN('x')` is false, because the second asks whether the value *is* the
             * NaN number rather than whether it reads as one. */
            *result = kiln_c99_boolean(isnan(first));
            return KILN_C99_OK;
        case KILN_C99_METHOD_GLOBAL_IS_FINITE:
            *result = kiln_c99_boolean(isfinite(first) ? 1 : 0);
            return KILN_C99_OK;
        case KILN_C99_METHOD_PARSE_INT:
        case KILN_C99_METHOD_PARSE_FLOAT: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value text;
            const char *bytes;
            size_t len;
            /* These two read a *string*, so the string hint: `toString` before `valueOf`, and
             * `parseInt({toString: () => "99"})` is 99. The same split `kiln_c99_to_number_checked`
             * makes for the number hint -- `kiln_c99_text` is pure, so it cannot run a `toString`
             * the program wrote and answered NaN for every object that has one. */
            if (kiln_c99_is_reference(subject)) {
                int reduced = kiln_c99_to_primitive(host, subject, 1, &subject, depth, thrown);
                if (reduced != KILN_C99_OK) {
                    return reduced;
                }
            }
            text = kiln_c99_text(subject);
            if (text.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            bytes = kiln_c99_string_ascii_bytes(text, &len);
            char *owned = NULL;
            if (bytes == NULL) {
                /* A UTF-16 subject (parallel issue 30): it used to answer "out of memory". Every
                 * unit either grammar can consume is ASCII, so what they read is the leading
                 * white space (Unicode's included, which only a UTF-16 string can hold) and then
                 * the ASCII run after it; the first other unit ends the number wherever it is. */
                const kiln_string *units = (const kiln_string *)text.ptr;
                size_t at = 0, end, index;
                if (units == NULL) return KILN_C99_OUT_OF_MEMORY;
                while (at < units->unit_len && kiln_c99_is_trimmed(kiln_string_unit_at(units, at))) {
                    at += 1;
                }
                end = at;
                while (end < units->unit_len && kiln_string_unit_at(units, end) < 0x80) end += 1;
                owned = (char *)malloc(end - at + 1);
                if (owned == NULL) return KILN_C99_OUT_OF_MEMORY;
                for (index = at; index < end; ++index) {
                    owned[index - at] = (char)kiln_string_unit_at(units, index);
                }
                bytes = owned;
                len = end - at;
            }
            if (code == KILN_C99_METHOD_PARSE_INT) {
                *result = kiln_c99_number(
                    kiln_c99_parse_int_bytes(bytes, len, argc > 1 ? kiln_c99_as_number(args[1]) : 0.0));
            } else {
                *result = kiln_c99_number(kiln_c99_parse_float_bytes(bytes, len));
            }
            free(owned);
            return KILN_C99_OK;
        }
#if KILN_RETAIN_GROUP_MATH_NUMBER
        case KILN_C99_METHOD_MATH_FLOOR: *result = kiln_c99_number(floor(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_CEIL: *result = kiln_c99_number(ceil(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ROUND: {
            /* Half rounds *up*, not away from zero, so `floor(x + 0.5)` is the definition. It
             * loses the sign of `Math.round(-0.5)`, which is -0 and not 0 -- a difference a
             * console shows and `Object.is` can see. */
            double rounded = isfinite(first) ? floor(first + 0.5) : first;
            if (rounded == 0.0 && kiln_c99_signbit(first)) {
                rounded = -0.0;
            }
            *result = kiln_c99_number(rounded);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_MATH_TRUNC: *result = kiln_c99_number(trunc(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ABS: *result = kiln_c99_number(fabs(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_SIGN:
            *result = kiln_c99_number(isnan(first) || first == 0.0 ? first
                                      : first < 0.0                ? -1.0
                                                                   : 1.0);
            return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_SQRT: *result = kiln_c99_number(sqrt(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_CBRT: *result = kiln_c99_number(cbrt(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_POW: *result = kiln_c99_number(pow(first, second)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_LOG: *result = kiln_c99_number(log(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_LOG2: *result = kiln_c99_number(log2(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_LOG10: *result = kiln_c99_number(log10(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_EXP: *result = kiln_c99_number(exp(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ATAN2: *result = kiln_c99_number(atan2(first, second)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_SIN: *result = kiln_c99_number(sin(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_COS: *result = kiln_c99_number(cos(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_TAN: *result = kiln_c99_number(tan(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ASIN: *result = kiln_c99_number(asin(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ACOS: *result = kiln_c99_number(acos(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ATAN: *result = kiln_c99_number(atan(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_SINH: *result = kiln_c99_number(sinh(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_COSH: *result = kiln_c99_number(cosh(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_TANH: *result = kiln_c99_number(tanh(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ASINH: *result = kiln_c99_number(asinh(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ACOSH: *result = kiln_c99_number(acosh(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ATANH: *result = kiln_c99_number(atanh(first)); return KILN_C99_OK;
        /* `ln(1 + x)` and `e^x - 1` as one operation each, which is the whole point of both: the
         * two-step spelling loses every significant digit for small `x`. */
        case KILN_C99_METHOD_MATH_LOG1P: *result = kiln_c99_number(log1p(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_EXPM1: *result = kiln_c99_number(expm1(first)); return KILN_C99_OK;
        /* The nearest `float`, widened back. A round trip, not a truncation. */
        case KILN_C99_METHOD_MATH_FROUND:
            *result = kiln_c99_number((double)(float)first);
            return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_CLZ32: {
            uint32_t bits = kiln_c99_to_uint32(first);
            int leading = 0;
            while (leading < 32 && (bits & (UINT32_C(0x80000000) >> leading)) == 0) {
                leading += 1;
            }
            *result = kiln_c99_number((double)leading);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_MATH_IMUL: {
            /* Both operands are truncated to 32 bits *first*, which is what makes this different
             * from `a * b`: the product is exact in 32 bits and wraps rather than rounding. */
            uint32_t product = kiln_c99_to_uint32(first) * kiln_c99_to_uint32(second);
            *result = kiln_c99_number((double)(int32_t)product);
            return KILN_C99_OK;
        }
#endif
        case KILN_C99_METHOD_REGEXP_TO_STRING:
            if (receiver.tag != KILN_VALUE_REGEXP) {
                KILN_C99_REFUSE("RegExp.prototype.toString on a receiver that is not a regular "
                                "expression");
            }
            *result = kiln_c99_text(receiver);
            return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
        case KILN_C99_METHOD_REGEXP_TEST:
        case KILN_C99_METHOD_REGEXP_EXEC: {
            kiln_regexp *regexp;
            kiln_value subject;
            uint16_t *units = NULL;
            size_t count = 0;
            size_t start;
            size_t groups = 0;
            int matched = 0;
            int64_t stack_ranges[64];
            int64_t *ranges = stack_ranges;
            size_t ranges_capacity = sizeof(stack_ranges) / sizeof(stack_ranges[0]);
            /* `RegExp.prototype.test.call({})` has no pattern to match with. Note that
             * `toString` on a foreign receiver is *not* this: it answers rather than throwing,
             * so that guard stays a refusal. */
            if (receiver.tag != KILN_VALUE_REGEXP) {
                return kiln_c99_throw_not_a(thrown, receiver, "function");
            }
            regexp = kiln_c99_regexp_of(receiver);
            /* Coerce after the receiver brand check: object subjects run their string-hint
             * conversion, and a Symbol throws instead of matching its diagnostic spelling. */
            status = kiln_c99_to_text_argument(host, argc > 0 ? args[0] : kiln_c99_undefined(),
                                                &subject, depth, thrown);
            if (status != KILN_C99_OK) return status;
            int is_ascii = 0;
            if (regexp->cached_subject_ptr == subject.ptr && regexp->cached_subject_len == subject.len) {
                is_ascii = regexp->cached_is_ascii;
            } else {
                is_ascii = kiln_c99_string_is_ascii(subject);
                kiln_c99_heap_store_pointer(regexp, &regexp->cached_subject_ptr, subject.ptr);
                regexp->cached_subject_len = subject.len;
                regexp->cached_is_ascii = is_ascii;
                regexp->cached_group_count = 0;
            }
            /* Only a `g` or `y` pattern has a cursor; everything else searches from the start
             * however many times it is asked. */
            start = regexp->global || regexp->sticky
                        ? (regexp->last_index < 0.0 ? 0 : (size_t)regexp->last_index)
                        : 0;
            uint16_t stack_units[512];
            int stack_used = 0;
            if (is_ascii && regexp->matcher_ascii != NULL) {
                status = regexp->matcher_ascii(kiln_string_latin1((const kiln_string *)subject.ptr), subject.len, start,
                                               ranges, ranges_capacity, &groups, &matched);
                if (status == KILN_C99_OK && matched && groups > ranges_capacity / 2) {
                    ranges = (int64_t *)malloc(groups * 2 * sizeof(int64_t));
                    if (ranges == NULL) {
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    ranges_capacity = groups * 2;
                    status = regexp->matcher_ascii(kiln_string_latin1((const kiln_string *)subject.ptr), subject.len, start,
                                                   ranges, ranges_capacity, &groups, &matched);
                }
            } else {
                if (is_ascii && subject.len <= 512) {
                    size_t i;
                    stack_used = 1;
                    count = subject.len;
                    units = stack_units;
                    for (i = 0; i < count; ++i) {
                        stack_units[i] = kiln_string_unit_at((const kiln_string *)subject.ptr, i);
                    }
                } else {
                    if (kiln_c99_to_units(subject, &units, &count) != KILN_C99_OK) {
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
                status = kiln_c99_regexp_find(regexp, units, count, start, ranges,
                                              ranges_capacity, &groups, &matched);
                if (status == KILN_C99_OK && matched && groups > ranges_capacity / 2) {
                    ranges = (int64_t *)malloc(groups * 2 * sizeof(int64_t));
                    if (ranges == NULL) {
                        if (units != NULL && !stack_used) {
                            free(units);
                        }
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    ranges_capacity = groups * 2;
                    status = kiln_c99_regexp_find(regexp, units, count, start, ranges,
                                                  ranges_capacity, &groups, &matched);
                }
            }
            if (status != KILN_C99_OK) {
                if (ranges != stack_ranges) {
                    free(ranges);
                }
                if (units != NULL && !stack_used) {
                    free(units);
                }
                return status;
            }
            if (regexp->global || regexp->sticky) {
                /* A miss resets the cursor, which is what makes a `while (re.exec(s))` loop
                 * terminate rather than repeat its last match forever. */
                regexp->last_index = matched ? (double)ranges[1] : 0.0;
            }
            if (!matched) {
                if (ranges != stack_ranges) {
                    free(ranges);
                }
                if (units != NULL && !stack_used) {
                    free(units);
                }
                *result = code == KILN_C99_METHOD_REGEXP_TEST ? kiln_c99_boolean(0)
                                                              : kiln_c99_null();
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_REGEXP_TEST) {
                if (ranges != stack_ranges) {
                    free(ranges);
                }
                if (units != NULL && !stack_used) {
                    free(units);
                }
                *result = kiln_c99_boolean(1);
                return KILN_C99_OK;
            }
            {
                /* The array `exec` produces: the whole match, then each group, with `index`,
                 * `input` and `groups` alongside -- properties *of the array*, which is why an
                 * array needs somewhere to keep something that is not an element. */
                kiln_value built;
                size_t group;
                status = kiln_c99_new_array(&built);
                if (status == KILN_C99_OK) {
                    kiln_array *arr = kiln_c99_array_of(built);
                    if (groups > 0) {
                        status = kiln_c99_array_reserve(arr, groups);
                    }
                    int can_reuse = (regexp->cached_group_count == groups && groups <= 32);
                    if (can_reuse) {
                        for (group = 0; group < groups; ++group) {
                            if (regexp->cached_ranges[group * 2] != ranges[group * 2] ||
                                regexp->cached_ranges[group * 2 + 1] != ranges[group * 2 + 1]) {
                                can_reuse = 0;
                                break;
                            }
                        }
                    }
                    if (can_reuse) {
                        for (group = 0; group < groups; ++group) {
                            kiln_c99_heap_store_val(arr, &arr->elements[arr->count++], regexp->cached_elements[group]);
                        }
                    } else {
                        for (group = 0; group < groups && status == KILN_C99_OK; ++group) {
                            int64_t from = ranges[group * 2];
                            int64_t to = ranges[group * 2 + 1];
                            kiln_value val;
                            if (from < 0) {
                                val = kiln_c99_undefined();
                            } else if (is_ascii) {
                                const kiln_string *subject_string = (const kiln_string *)subject.ptr;
                                val = (to >= from && (size_t)to <= subject_string->unit_len)
                                          ? kiln_c99_copy_ascii(
                                                (const char *)kiln_string_latin1(subject_string) + from,
                                                (size_t)(to - from))
                                          : kiln_c99_undefined();
                            } else {
                                val = kiln_c99_from_units(units + (size_t)from, (size_t)(to - from));
                            }
                            kiln_val bval = kiln_val_from_boundary(val);
                            kiln_c99_heap_store_val(arr, &arr->elements[arr->count++], bval);
                            if (group < 32) {
                                kiln_c99_heap_store_val(regexp, &regexp->cached_elements[group], bval);
                                regexp->cached_ranges[group * 2] = from;
                                regexp->cached_ranges[group * 2 + 1] = to;
                            }
                        }
                        if (groups <= 32) {
                            regexp->cached_group_count = groups;
                        } else {
                            regexp->cached_group_count = 0;
                        }
                    }
                    arr->match_index = ranges[0];
                    kiln_c99_heap_store_value(arr, &arr->match_input, subject);
                    status = kiln_c99_match_groups(regexp, arr);
                    if (status != KILN_C99_OK) {
                        if (ranges != stack_ranges) free(ranges);
                        if (units != NULL && !stack_used) free(units);
                        return status;
                    }
                }
                if (ranges != stack_ranges) {
                    free(ranges);
                }
                if (units != NULL && !stack_used) {
                    free(units);
                }
                *result = built;
                return status;
            }
        }
        case KILN_C99_METHOD_MATH_RANDOM: {
            double random;
            int status = kiln_c99_random(host, &random);
            if (status != KILN_C99_OK) return status;
            *result = kiln_c99_number(random);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_MATH_HYPOT: {
            /* A fold of `hypot` rather than `sqrt(x*x + y*y)`: squaring overflows for arguments
             * whose result is perfectly representable, which is the whole reason it exists. */
            double total = 0.0;
            for (index = 0; index < argc; ++index) {
                double leg;
                int reduced = kiln_c99_to_number_checked(host, args[index], &leg, depth, thrown);
                if (reduced != KILN_C99_OK) {
                    return reduced;
                }
                total = hypot(total, leg);
            }
            *result = kiln_c99_number(total);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_MATH_MIN:
        case KILN_C99_METHOD_MATH_MAX: {
            /* No arguments is the identity for the comparison, which is the infinity that loses
             * every one of them. */
            double best = code == KILN_C99_METHOD_MATH_MIN ? 1.0 / 0.0 : -1.0 / 0.0;
            for (index = 0; index < argc; ++index) {
                double value;
                int reduced = kiln_c99_to_number_checked(host, args[index], &value, depth, thrown);
                if (reduced != KILN_C99_OK) {
                    return reduced;
                }
                if (isnan(value)) {
                    best = value;
                } else if (!isnan(best) && (code == KILN_C99_METHOD_MATH_MIN ? value < best : value > best)) {
                    best = value;
                }
            }
            *result = kiln_c99_number(best);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_IS_INTEGER:
        case KILN_C99_METHOD_IS_SAFE_INTEGER:
            /* `Number.isInteger` does not coerce, unlike the global `isNaN` and friends: a
             * string is simply not an integer. Same code with coercion would be a different
             * function, so it is a different function. */
            *result = kiln_c99_boolean(
                argc > 0 && args[0].tag == KILN_VALUE_NUMBER && isfinite(first) &&
                floor(first) == first &&
                (code == KILN_C99_METHOD_IS_INTEGER || fabs(first) <= 9007199254740991.0));
            return KILN_C99_OK;
        case KILN_C99_METHOD_IS_NAN:
            *result = kiln_c99_boolean(argc > 0 && args[0].tag == KILN_VALUE_NUMBER &&
                                       isnan(first));
            return KILN_C99_OK;
        case KILN_C99_METHOD_IS_FINITE:
            *result = kiln_c99_boolean(argc > 0 && args[0].tag == KILN_VALUE_NUMBER &&
                                       isfinite(first));
            return KILN_C99_OK;
        case KILN_C99_METHOD_IS_ARRAY: {
            kiln_value current = argc > 0 ? args[0] : kiln_c99_undefined();
            while (current.tag == KILN_VALUE_PROXY) {
                kiln_proxy *proxy = kiln_c99_proxy_of(current);
                if (proxy->revoked) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Cannot perform operation on a proxy that has been revoked");
                }
                current = proxy->target;
            }
            *result = kiln_c99_boolean(
                current.tag == KILN_VALUE_ARRAY ||
                (current.tag == KILN_VALUE_SHARED &&
                 kiln_c99_shared_is_array(current)) ||
                (current.tag == KILN_VALUE_BUILTIN &&
                 current.bits == KILN_C99_BUILTIN_ARRAY_PROTOTYPE));
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ARRAY_OF: {
            kiln_value built;
            status = kiln_c99_new_array(&built);
            for (index = 0; index < argc && status == KILN_C99_OK; ++index) {
                status = kiln_c99_array_push(built, args[index]);
            }
            *result = built;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_KEYS:
        case KILN_C99_METHOD_OBJECT_VALUES:
        case KILN_C99_METHOD_OBJECT_ENTRIES: {
            kiln_value built;
            kiln_object *object;
            if (argc == 0 || args[0].tag == KILN_VALUE_UNDEFINED ||
                args[0].tag == KILN_VALUE_NULL) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot convert undefined or null to object");
            }
            if (args[0].tag == KILN_VALUE_BOOLEAN || args[0].tag == KILN_VALUE_NUMBER ||
                args[0].tag == KILN_VALUE_SYMBOL || args[0].tag == KILN_VALUE_BIGINT) {
                status = kiln_c99_new_array(&built);
                *result = built;
                return status;
            }
            if (args[0].tag == KILN_VALUE_PROXY || args[0].tag == KILN_VALUE_SHARED) {
                /* A proxy's keys come from `ownKeys`; a shared facade's come from its sealed
                 * graph. Neither has a mutable property store to walk, and both use the
                 * ordinary computed read for values and entries. */
                kiln_value keys;
                kiln_array *listed;
                status = kiln_c99_own_keys(host, args[0], &keys, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                    *result = keys;
                    return KILN_C99_OK;
                }
                status = kiln_c99_new_array(&built);
                if (status != KILN_C99_OK) {
                    return status;
                }
                listed = kiln_c99_array_of(keys);
                for (index = 0; index < listed->count; ++index) {
                    kiln_value read;
                    kiln_value key = kiln_val_to_boundary(listed->elements[index]);
                    status = kiln_c99_get_computed(host, args[0], key, &read, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                    if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                        status = kiln_c99_array_push(built, read);
                    } else {
                        kiln_value pair;
                        status = kiln_c99_new_array(&pair);
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_array_push(pair, key);
                        }
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_array_push(pair, read);
                        }
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_array_push(built, pair);
                        }
                    }
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                    listed = kiln_c99_array_of(keys);
                }
                *result = built;
                return KILN_C99_OK;
            }
            /* An array's own enumerable keys are its indices, as strings, and a hole is not one
             * of them -- which is the same walk `for...in` does, so it is the same code. */
            if (args[0].tag == KILN_VALUE_ARRAY) {
                kiln_array *array = kiln_c99_array_of(args[0]);
                status = kiln_c99_new_array(&built);
                for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
                    if (array->elements[index] == KILN_VAL_HOLE) {
                        continue;
                    }
                    if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                        status = kiln_c99_array_push(
                            built, kiln_c99_text(kiln_c99_number((double)index)));
                    } else if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                        status = kiln_c99_array_push(built, kiln_val_to_boundary(array->elements[index]));
                    } else {
                        kiln_value pair;
                        status = kiln_c99_new_array(&pair);
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_array_push(
                                pair, kiln_c99_text(kiln_c99_number((double)index)));
                        }
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_array_push(pair, kiln_val_to_boundary(array->elements[index]));
                        }
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_array_push(built, pair);
                        }
                    }
                }
                if (array->properties.tag == KILN_VALUE_OBJECT) {
                    kiln_object *extra = kiln_c99_object_of(array->properties);
                    if (extra->properties == NULL && extra->shape != NULL &&
                        extra->shape->accessor_mask == 0) {
                        for (index = 0; index < extra->shape->property_count && status == KILN_C99_OK; ++index) {
                            const kiln_string *skey = extra->shape->keys[index];
                            kiln_value key;
                            kiln_value val;
                            if (kiln_c99_key_is_reserved(skey) ||
                                (extra->shape->attrs[index] & KILN_C99_ATTR_ENUM_FLIP) != 0) {
                                continue;
                            }
                            key = kiln_c99_stable_key_value(skey);
                            val = (extra->slots != NULL && index < extra->slot_capacity)
                                      ? kiln_val_to_boundary(extra->slots[index])
                                      : kiln_c99_undefined();
                            if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                                status = kiln_c99_array_push(built, key);
                            } else if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                                status = kiln_c99_array_push(built, val);
                            } else {
                                kiln_value pair;
                                status = kiln_c99_new_array(&pair);
                                if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, key);
                                if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, val);
                                if (status == KILN_C99_OK) status = kiln_c99_array_push(built, pair);
                            }
                        }
                    } else {
                        if (extra->shape != NULL) {
                            kiln_c99_ensure_dictionary(extra);
                        }
                        for (index = 0; index < extra->count && status == KILN_C99_OK; ++index) {
                            kiln_value key;
                            if (!extra->properties[index].enumerable) {
                                continue;
                            }
                            key = kiln_c99_stable_key_value(extra->properties[index].key);
                            if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                                status = kiln_c99_array_push(built, key);
                            } else if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                                status = kiln_c99_array_push(built, extra->properties[index].value);
                            } else {
                                kiln_value pair;
                                status = kiln_c99_new_array(&pair);
                                if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, key);
                                if (status == KILN_C99_OK) {
                                    status = kiln_c99_array_push(pair,
                                                                 extra->properties[index].value);
                                }
                                if (status == KILN_C99_OK) status = kiln_c99_array_push(built, pair);
                            }
                        }
                    }
                }
                *result = built;
                return status;
            }
            if (args[0].tag == KILN_VALUE_TYPED_ARRAY) {
                kiln_typed_array *view = kiln_c99_typed_array_of(args[0]);
                status = kiln_c99_new_array(&built);
                for (index = 0; index < view->length && status == KILN_C99_OK; ++index) {
                    kiln_value key = kiln_c99_text(kiln_c99_number((double)index));
                    kiln_value value;
                    status = kiln_c99_typed_element(view, index, &value);
                    if (status != KILN_C99_OK) break;
                    if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                        status = kiln_c99_array_push(built, key);
                    } else if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                        status = kiln_c99_array_push(built, value);
                    } else {
                        kiln_value pair;
                        status = kiln_c99_new_array(&pair);
                        if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, key);
                        if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, value);
                        if (status == KILN_C99_OK) status = kiln_c99_array_push(built, pair);
                    }
                }
                *result = built;
                return status;
            }
            if (args[0].tag == KILN_VALUE_DATA_VIEW) {
                /* Empty, and provably empty rather than plausibly so. A DataView has no indices
                 * to enumerate, and `kiln_c99_exotic_store` gives it no property store either --
                 * a write by name refuses instead of landing somewhere this walk cannot see. So
                 * there is nothing an own enumerable key could be.
                 *
                 * That is exactly what the `else` below is right to refuse for the tags it
                 * catches: a Date and a collection *do* have stores, so an empty answer for one
                 * of those would be a wrong answer rather than a complete one. */
                status = kiln_c99_new_array(&built);
                *result = built;
                return status;
            }
            if (args[0].tag == KILN_VALUE_FUNCTION || args[0].tag == KILN_VALUE_BOUND ||
                args[0].tag == KILN_VALUE_BUILTIN || args[0].tag == KILN_VALUE_METHOD ||
                args[0].tag == KILN_VALUE_SETTLER || args[0].tag == KILN_VALUE_COMMONJS_REQUIRE) {
                /* A function is an object: `f.version = 2` is an own enumerable property of it,
                 * and it is kept on the closure's store rather than in a slot list of its own.
                 * `name`, `length` and `prototype` are read from the closure and are not in that
                 * store, which is right -- none of the three is enumerated. */
                kiln_value store;
                status = kiln_c99_callable_store(args[0], &store, 0);
                if (status != KILN_C99_OK) return status;
                if (store.tag != KILN_VALUE_OBJECT) {
                    /* Nothing was ever assigned to it, so there is nothing to list. */
                    status = kiln_c99_new_array(&built);
                    *result = built;
                    return status;
                }
                object = kiln_c99_object_of(store);
            } else if (args[0].tag == KILN_VALUE_COLLECTION) {
                kiln_value store = kiln_c99_collection_of(args[0])->properties;
                if (store.tag != KILN_VALUE_OBJECT) {
                    status = kiln_c99_new_array(&built);
                    *result = built;
                    return status;
                }
                object = kiln_c99_object_of(store);
            } else if (args[0].tag == KILN_VALUE_GENERATOR) {
                kiln_value store = kiln_c99_generator_of(args[0])->properties;
                if (store.tag != KILN_VALUE_OBJECT) {
                    status = kiln_c99_new_array(&built);
                    *result = built;
                    return status;
                }
                object = kiln_c99_object_of(store);
            } else if (args[0].tag == KILN_VALUE_PROMISE) {
                kiln_value store = kiln_c99_promise_of(args[0])->properties;
                if (store.tag != KILN_VALUE_OBJECT) {
                    status = kiln_c99_new_array(&built);
                    *result = built;
                    return status;
                }
                object = kiln_c99_object_of(store);
            } else if (kiln_c99_exotic_store(args[0]) != NULL) {
                kiln_value store = *kiln_c99_exotic_store(args[0]);
                if (store.tag != KILN_VALUE_OBJECT) {
                    status = kiln_c99_new_array(&built);
                    *result = built;
                    return status;
                }
                object = kiln_c99_object_of(store);
            } else if (args[0].tag == KILN_VALUE_OBJECT) {
                object = kiln_c99_object_of(args[0]);
            } else {
                KILN_C99_REFUSE("an own-key listing of a value with no property store");
            }
            if (object->properties == NULL && object->shape != NULL &&
                object->shape->accessor_mask == 0) {
                status = kiln_c99_new_array(&built);
                if (object->elements != NULL) {
                    for (index = 0; index < object->element_count && status == KILN_C99_OK; ++index) {
                        if (object->elements[index] == KILN_VAL_HOLE) continue;
                        char num_buf[32];
                        size_t nlen = kiln_c99_double_to_js((double)index, num_buf);
                        kiln_value key = kiln_c99_copy_text(num_buf, nlen);
                        kiln_value val = kiln_val_to_boundary(object->elements[index]);
                        if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                            status = kiln_c99_array_push(built, key);
                        } else if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                            status = kiln_c99_array_push(built, val);
                        } else {
                            kiln_value pair;
                            status = kiln_c99_new_array(&pair);
                            if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, key);
                            if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, val);
                            if (status == KILN_C99_OK) status = kiln_c99_array_push(built, pair);
                        }
                    }
                }
                for (index = 0; index < object->shape->property_count && status == KILN_C99_OK; ++index) {
                    const kiln_string *skey = object->shape->keys[index];
                    kiln_value key;
                    kiln_value val;
                    if (kiln_c99_key_is_reserved(skey) ||
                        (object->shape->attrs[index] & KILN_C99_ATTR_ENUM_FLIP) != 0) {
                        continue;
                    }
                    key = kiln_c99_stable_key_value(skey);
                    val = (object->slots != NULL && index < object->slot_capacity)
                              ? kiln_val_to_boundary(object->slots[index])
                              : kiln_c99_undefined();
                    if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                        status = kiln_c99_array_push(built, key);
                    } else if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                        status = kiln_c99_array_push(built, val);
                    } else {
                        kiln_value pair;
                        status = kiln_c99_new_array(&pair);
                        if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, key);
                        if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, val);
                        if (status == KILN_C99_OK) status = kiln_c99_array_push(built, pair);
                    }
                }
                *result = built;
                return status;
            }
            {
                kiln_value keys;
                size_t count;
                if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                    status = kiln_c99_property_walk(object, &count);
                    if (status != KILN_C99_OK) return status;
                    status = kiln_c99_new_array(&built);
                    if (status != KILN_C99_OK) return status;
                    for (index = 0; index < count; ++index) {
                        kiln_property projected;
                        const kiln_property *property =
                            kiln_c99_property_view(object, index, &projected);
                        if (!property->enumerable) continue;
                        status = kiln_c99_array_push(
                            built, kiln_c99_stable_key_value(property->key));
                        if (status != KILN_C99_OK) return status;
                    }
                    *result = built;
                    return KILN_C99_OK;
                }
                status = kiln_c99_property_key_snapshot(object, &keys);
                if (status != KILN_C99_OK) return status;
                status = kiln_c99_new_array(&built);
                if (status != KILN_C99_OK) return status;
                for (index = 0; index < kiln_c99_array_of(keys)->count; ++index) {
                    const kiln_string *name = (const kiln_string *)
                        kiln_val_to_boundary(kiln_c99_array_of(keys)->elements[index]).ptr;
                    kiln_property property;
                    kiln_value key;
                    kiln_value value;
                    if (!kiln_c99_property_read_own(object, name, name, &property) ||
                        !property.enumerable) continue;
                    key = kiln_c99_stable_key_value(name);
                    if (property.accessor) {
                        if (property.getter.tag == KILN_VALUE_UNDEFINED) {
                            value = kiln_c99_undefined();
                        } else {
                            status = kiln_c99_call(host, &value, property.getter, args[0],
                                                   NULL, 0, depth, thrown);
                            if (status != KILN_C99_OK) return status;
                        }
                    } else {
                        value = property.value;
                    }
                    if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                        status = kiln_c99_array_push(built, value);
                    } else {
                        kiln_value pair;
                        status = kiln_c99_new_array(&pair);
                        if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, key);
                        if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, value);
                        if (status == KILN_C99_OK) status = kiln_c99_array_push(built, pair);
                    }
                    if (status != KILN_C99_OK) return status;
                }
            }
            *result = built;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_ASSIGN: {
            kiln_value target;
            size_t argument;
            /* A function is a legal target, and attaching statics to an exported one --
             * `Object.assign(postcss, { parse, stringify })` -- is among the most common shapes in
             * a bundle. Only the target check rejected it: `kiln_c99_put` already writes to a
             * function, which is why `f.parse = parse` has always worked. An array is a target for
             * the same reason and by the same path.
             *
             * Everything else still refuses. The specification would send a primitive through
             * ToObject and copy onto the wrapper, which is observable only through the return
             * value; refusing by name says so, where answering would be a guess. */
            if (argc == 0 ||
                (args[0].tag != KILN_VALUE_OBJECT && args[0].tag != KILN_VALUE_ARRAY &&
                 args[0].tag != KILN_VALUE_FUNCTION && args[0].tag != KILN_VALUE_BUILTIN &&
                 args[0].tag != KILN_VALUE_METHOD && args[0].tag != KILN_VALUE_BOUND &&
                 args[0].tag != KILN_VALUE_GENERATOR && args[0].tag != KILN_VALUE_REGEXP)) {
                KILN_C99_REFUSE("Object.assign target must be an object, an array or a function");
            }
            target = args[0];
            for (argument = 1; argument < argc; ++argument) {
                kiln_object *source;
                if (args[argument].tag == KILN_VALUE_UNDEFINED ||
                    args[argument].tag == KILN_VALUE_NULL) {
                    continue;
                }
                kiln_value src_val = args[argument];
                if (src_val.tag == KILN_VALUE_ARRAY || src_val.tag == KILN_VALUE_TYPED_ARRAY ||
                    src_val.tag == KILN_VALUE_PROXY || src_val.tag == KILN_VALUE_SHARED ||
                    src_val.tag == KILN_VALUE_GENERATOR || src_val.tag == KILN_VALUE_REGEXP) {
                    kiln_value keys;
                    status = kiln_c99_all_own_keys(host, src_val, &keys, depth, thrown);
                    if (status != KILN_C99_OK) return status;
                    /* Snapshot keys before any getter or target setter runs. Recheck each own
                     * descriptor, since an earlier copy may have deleted a later property. */
                    for (index = 0; index < kiln_c99_array_of(keys)->count; ++index) {
                        kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(keys)->elements[index]);
                        kiln_value name = kiln_c99_property_key(key);
                        const kiln_string *view = (const kiln_string *)name.ptr;
                        kiln_value descriptor, enumerable, copied;
                        int found;
                        if (src_val.tag == KILN_VALUE_TYPED_ARRAY) {
                            int is_enumerable;
                            kiln_c99_typed_own_key(src_val, view, &found, &is_enumerable);
                            enumerable = kiln_c99_boolean(is_enumerable);
                        } else {
                            status = kiln_c99_describe_property_key(host, src_val, view, NULL,
                                &descriptor, &found, depth, thrown);
                            if (status != KILN_C99_OK) return status;
                            if (found) {
                                status = kiln_c99_get(host, descriptor, "enumerable", 10,
                                    &enumerable, depth, thrown);
                                if (status != KILN_C99_OK) return status;
                            }
                        }
                        if (!found) continue;
                        if (!kiln_c99_truthy(enumerable)) continue;
                        status = kiln_c99_get_computed(host, src_val, key, &copied, depth, thrown);
                        if (status == KILN_C99_OK)
                            status = kiln_c99_put_mode_key(host, target, view, NULL,
                                copied, 1, depth, thrown);
                        if (status != KILN_C99_OK) return status;
                    }
                    continue;
                }
                if (src_val.tag == KILN_VALUE_NUMBER || src_val.tag == KILN_VALUE_BOOLEAN ||
                    src_val.tag == KILN_VALUE_SYMBOL || src_val.tag == KILN_VALUE_BIGINT)
                    continue;
                if (src_val.tag == KILN_VALUE_STRING) {
                    size_t count;
                    uint16_t *units = NULL;
                    status = kiln_c99_to_units(src_val, &units, &count);
                    free(units);
                    if (status != KILN_C99_OK) return status;
                    for (index = 0; index < count; ++index) {
                        kiln_value key = kiln_c99_text(kiln_c99_number((double)index));
                        kiln_value copied;
                        status = kiln_c99_get_computed(host, src_val, key, &copied, depth, thrown);
                        if (status == KILN_C99_OK)
                            status = kiln_c99_put_mode_key(host, target, (const kiln_string *)key.ptr,
                                NULL, copied, 1, depth, thrown);
                        if (status != KILN_C99_OK) return status;
                    }
                    continue;
                }
                if (src_val.tag == KILN_VALUE_FUNCTION || src_val.tag == KILN_VALUE_METHOD ||
                    src_val.tag == KILN_VALUE_BOUND || src_val.tag == KILN_VALUE_BUILTIN) {
                    kiln_value store;
                    int st = kiln_c99_callable_store(src_val, &store, 0);
                    if (st != KILN_C99_OK) return st;
                    if (store.tag != KILN_VALUE_OBJECT) continue;
                    src_val = store;
                }
                if (src_val.tag != KILN_VALUE_OBJECT) {
                    KILN_C99_REFUSE("Object.assign source that is not an object and has no "
                                    "property store");
                }
                source = kiln_c99_object_of(src_val);
                if (source->sealed != NULL) {
                    kiln_value keys;
                    status = kiln_c99_property_key_snapshot(source, &keys);
                    if (status != KILN_C99_OK) return status;
                    for (index = 0; index < kiln_c99_array_of(keys)->count; ++index) {
                        const kiln_string *name = (const kiln_string *)
                            kiln_val_to_boundary(kiln_c99_array_of(keys)->elements[index]).ptr;
                        kiln_property property;
                        kiln_value copied;
                        if (!kiln_c99_property_read_own(source, name, name, &property) ||
                            !kiln_c99_property_is_enumerable(&property)) continue;
                        if (property.accessor) {
                            status = kiln_c99_get_key(host, src_val, name, name, &copied,
                                                      depth, thrown);
                            if (status != KILN_C99_OK) return status;
                        } else {
                            copied = property.value;
                        }
                        status = kiln_c99_put_mode_key(host, target, name, name, copied, 1,
                                                       depth, thrown);
                        if (status != KILN_C99_OK) return status;
                    }
                    continue;
                }
                if (source->properties == NULL && source->shape != NULL &&
                    source->shape->special_mask == 0) {
                    if (source->elements != NULL) {
                        for (index = 0; index < source->element_count; ++index) {
                            if (source->elements[index] == KILN_VAL_HOLE) continue;
                            char num_buf[32];
                            size_t nlen = kiln_c99_double_to_js((double)index, num_buf);
                            status = kiln_c99_put_mode(host, target, num_buf, nlen, kiln_val_to_boundary(source->elements[index]), 1, depth, thrown);
                            if (status != KILN_C99_OK) return status;
                        }
                    }
                    for (index = 0; index < source->shape->property_count; ++index) {
                        const kiln_string *skey = source->shape->keys[index];
                        kiln_value sval;
                        if (kiln_c99_key_is_reserved(skey)) {
                            kiln_value symbol;
                            if (!kiln_c99_symbol_of_key_view(skey, &symbol)) {
                                continue;
                            }
                        }
                        sval = (source->slots != NULL && index < source->slot_capacity)
                                   ? kiln_val_to_boundary(source->slots[index])
                                   : kiln_c99_undefined();
                        status = kiln_c99_put_mode_key(host, target, skey, skey, sval, 1, depth,
                                                       thrown);
                        if (status != KILN_C99_OK) {
                            return status;
                        }
                    }
                } else {
                    if (source->shape != NULL) {
                        kiln_c99_ensure_dictionary(source);
                    }
                    for (index = 0; index < source->count; ++index) {
                        kiln_value copied;
                        /* What the program sees as enumerable: a symbol key is stored
                         * non-enumerable and carries its answer in `symbol_hidden`, and an
                         * accessor is read through its getter -- as the shaped walk above and
                         * object spread do. */
                        if (!kiln_c99_property_is_enumerable(&source->properties[index])) {
                            continue;
                        }
                        if (source->properties[index].accessor) {
                            status = kiln_c99_get_key(host, src_val, source->properties[index].key,
                                                      source->properties[index].key, &copied,
                                                      depth, thrown);
                            if (status != KILN_C99_OK) return status;
                            if (index >= source->count) break;
                        } else {
                            copied = source->properties[index].value;
                        }
                        status = kiln_c99_put_mode_key(host, target, source->properties[index].key,
                                                       source->properties[index].key,
                                                       copied, 1, depth, thrown);
                        if (status != KILN_C99_OK) {
                            return status;
                        }
                    }
                }
            }
            *result = target;
            return KILN_C99_OK;
        }
