
/* Which of a built-in's arguments name a *number*, as a bitmask over argument positions.
 *
 * The C twin of the interpreter's `numeric_arguments`, and deliberately the same list written
 * twice: the two engines run the same bytecode, so a position coerced in one and not in the
 * other is a divergence by construction -- which is exactly how
 * `(5.5).toFixed({valueOf: () => 2})` came to answer `"6"` here and `"5.50"` there, with the
 * program's own `valueOf` never called at all.
 *
 * Only positions that really are numbers are listed. `fill`'s value and `indexOf`'s needle are
 * any value at all, and converting either would be wrong. */
static uint32_t kiln_c99_numeric_arguments(uint64_t code) {
    /* `DataView.prototype`, answered by a range test rather than twenty `case`s.
     *
     * Every accessor takes its byte offset first, and a numeric setter takes a value second --
     * both `ToNumber`, and both converted here because this is the pass with a machine to run a
     * `valueOf` with. The order matters and is observable: the interpreter converts the value
     * *before* it checks the bounds, so a `valueOf` that throws throws even when the write could
     * never have landed.
     *
     * The two BigInt setters name only the offset. `ToNumber` on a BigInt is a TypeError, which
     * is the one type those two accept, so their value is `ToBigInt`ed by the handler instead. */
    if (code >= KILN_C99_METHOD_DATA_VIEW_GET_INT8 &&
        code <= KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64) {
        if (code >= KILN_C99_METHOD_DATA_VIEW_SET_INT8 &&
            code <= KILN_C99_METHOD_DATA_VIEW_SET_FLOAT64) {
            return UINT32_C(0x3);
        }
        return UINT32_C(0x1);
    }
    switch (code) {
    /* `(start, end)`. */
    case KILN_C99_METHOD_SLICE:
    case KILN_C99_METHOD_ARRAY_SPLICE:
    case KILN_C99_METHOD_ARRAY_TO_SPLICED:
    case KILN_C99_METHOD_STRING_SLICE:
    case KILN_C99_METHOD_SUBSTRING:
    case KILN_C99_METHOD_STRING_SUBSTR:
    case KILN_C99_METHOD_TYPED_SUBARRAY:
        return UINT32_C(0x3);
    /* `(target, start, end)`. */
    case KILN_C99_METHOD_ARRAY_COPY_WITHIN:
        return UINT32_C(0x7);
    /* `(value, start, end)`: the value is any value at all, the two bounds are numbers. */
    case KILN_C99_METHOD_ARRAY_FILL:
        return UINT32_C(0x6);
    /* `(searched, from)`: the needle is any value at all, the position is a number. */
    case KILN_C99_METHOD_INDEX_OF:
    case KILN_C99_METHOD_ARRAY_LAST_INDEX_OF:
    case KILN_C99_METHOD_INCLUDES:
    case KILN_C99_METHOD_STRING_INDEX_OF:
    case KILN_C99_METHOD_LAST_INDEX_OF:
    case KILN_C99_METHOD_STRING_INCLUDES:
    case KILN_C99_METHOD_STARTS_WITH:
    case KILN_C99_METHOD_ENDS_WITH:
    case KILN_C99_METHOD_TYPED_SET:
        return UINT32_C(0x2);
    /* One number, in the first position. */
    case KILN_C99_METHOD_ARRAY_AT:
    /* `with`'s index only. Its second argument is stored exactly as it was handed over -- it is
     * the one argument of the four copying methods that must not be converted to anything. */
    case KILN_C99_METHOD_ARRAY_WITH:
    case KILN_C99_METHOD_STRING_AT:
    case KILN_C99_METHOD_CHAR_AT:
    case KILN_C99_METHOD_CHAR_CODE_AT:
    case KILN_C99_METHOD_STRING_CODE_POINT_AT:
    case KILN_C99_METHOD_PAD_START:
    case KILN_C99_METHOD_PAD_END:
    case KILN_C99_METHOD_REPEAT:
    case KILN_C99_METHOD_NUMBER_TO_FIXED:
    case KILN_C99_METHOD_NUMBER_TO_EXPONENTIAL:
    case KILN_C99_METHOD_NUMBER_TO_PRECISION:
    case KILN_C99_METHOD_NUMBER_TO_STRING:
        return UINT32_C(0x1);
    default:
        return UINT32_C(0);
    }
}

/* Which of a built-in's arguments it reads as *text*, on the same terms as the table above.
 *
 * The C twin of the interpreter's `textual_arguments`. The four `CreateHTML` wrappers that take
 * an attribute belong here too and are absent only because this backend does not carry them --
 * it refuses them by name, which is an answer, where a wrong string would not be. */
static uint32_t kiln_c99_textual_arguments(uint64_t code) {
    switch (code) {
    case KILN_C99_METHOD_ESCAPE:
    case KILN_C99_METHOD_UNESCAPE:
        return UINT32_C(0x1);
    default:
        return UINT32_C(0);
    }
}

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
         * The eight numeric setters arrive already converted -- `kiln_c99_numeric_arguments`
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

/* The bytes one `set` accessor is being asked to store, as an unsigned integer of its width.
 *
 * Its own function because the conversion is the half with an order to get right, and the only
 * half that can call back into the program. */
static int kiln_c99_data_view_write_bits(kiln_host *host, uint64_t code, kiln_value value,
                                         uint64_t *out, int depth, kiln_value *thrown) {
    int status;
    switch (code) {
    case KILN_C99_METHOD_DATA_VIEW_SET_BIG_INT64:
    case KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64: {
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
            if (value.len == 0 ||
                kiln_bigint_parse((const unsigned char *)value.ptr, value.len, 10,
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

static int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown) {
    uint32_t numeric = kiln_c99_numeric_arguments(code);
    uint32_t textual = kiln_c99_textual_arguments(code);
    kiln_value inline_storage[KILN_C99_COERCED_INLINE];
    kiln_value *coerced = NULL;
    size_t index;
    int status = KILN_C99_OK;
    if ((numeric | textual) == 0 || argc == 0) {
        return kiln_c99_invoke_method_dispatch(host, result, code, receiver, args, argc, depth,
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
        status = kiln_c99_invoke_method_dispatch(host, result, code, receiver,
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

static int kiln_c99_default_require(void *context, kiln_host *host, kiln_value specifier,
                                    kiln_value *result, kiln_value *thrown) {
    (void)context;
    if (specifier.tag == KILN_VALUE_STRING && specifier.ptr != NULL) {
        if (specifier.len == 8 && memcmp(specifier.ptr, "node:ffi", 8) == 0)
            return kiln_c99_node_ffi_commonjs(host, result, thrown);
        kiln_text text = {0};
        int status;
        if (kiln_c99_text_append_c(&text, "Cannot find module '") != KILN_C99_OK ||
            kiln_c99_text_append(&text, specifier.ptr, specifier.len) != KILN_C99_OK ||
            kiln_c99_text_append_c(&text, "'") != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_value message = kiln_c99_text_value(&text);
        kiln_value error;
        status = kiln_c99_new_error(&error, KILN_C99_BUILTIN_ERROR, message);
        if (status == KILN_C99_OK && thrown != NULL) {
            *thrown = error;
        }
        return KILN_C99_HOST_ERROR;
    }
    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR, "Cannot find module");
}

static int kiln_c99_invoke_method_dispatch(kiln_host *host, kiln_value *result, uint64_t code,
                                           kiln_value receiver, const kiln_value *args,
                                           size_t argc, int depth, kiln_value *thrown) {
    kiln_array *array;
    size_t index;
    int status;
    if ((code & KILN_C99_FFI_DYNAMIC_BASE) != 0 ||
        code == KILN_C99_METHOD_NODE_FFI_DLOPEN ||
        code == KILN_C99_METHOD_NODE_FFI_GET_RAW_POINTER ||
        code == KILN_C99_METHOD_NODE_FFI_TO_ARRAY_BUFFER ||
        code == KILN_C99_METHOD_NODE_FFI_REGISTER_CALLBACK ||
        code == KILN_C99_METHOD_NODE_FFI_UNREGISTER_CALLBACK ||
        code == KILN_C99_METHOD_NODE_FFI_CLOSE)
        return kiln_c99_node_ffi(host, result, code, receiver, args, argc, depth, thrown);
    /* `kiln_c99_method_groups` is a generated switch over every method code this backend knows,
     * and every group test below used to run it again from scratch just to AND one bit out of
     * its answer -- a code that reaches its group late in this chain (an ordinary node:http
     * method, behind the event and net tests) paid for the switch three or four times over on
     * every call. `code` cannot change underneath this function, so its groups cannot either;
     * one call up front and a mask test everywhere below answers exactly the same questions. */
    uint32_t groups = kiln_c99_method_groups(code);
    /* The iterator helpers, routed by *range* rather than by group. They are one contiguous block
     * with one handler, and a range test here is a comparison against two constants where a group
     * bit would have cost one of the twenty-one the mask has left. Nothing else may be numbered
     * inside 470..484 -- see the registry comment beside the codes. */
    if (code >= KILN_C99_METHOD_ITERATOR_MAP && code <= KILN_C99_METHOD_ITERATOR_FROM) {
        return kiln_c99_iterator_method(host, result, code, receiver, args, argc, depth, thrown);
    }
    if ((groups & KILN_C99_GROUP_NODE_EVENT)) {
        return kiln_c99_node_event(host, result, code, receiver, args, argc, depth, thrown);
    }
    /* `setEncoding` is the one method name three unrelated Node receivers own: a `net.Socket`,
     * an `http.IncomingMessage`, and a `stream.Readable`. A dispatch group answers "which handler
     * owns this code", so a code carrying three group bits is answered by whichever group happens
     * to be tested first -- which is not a property of the call. Route this one by the receiver's
     * own hidden state, above every group test, so the name resolves to the object it was called
     * on rather than to the order these ifs are written in. */
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
    if ((groups & KILN_C99_GROUP_NODE_NET)) {
        return kiln_c99_node_net(host, result, code, receiver, args, argc, depth, thrown);
    }
    if ((groups & KILN_C99_GROUP_NODE_HTTP)) {
        return kiln_c99_node_http(host, result, code, receiver, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_HTTP_SERVER) {
        return kiln_c99_node_http(host, result, code, receiver, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_SERVER_SET_TIMEOUT) {
        return kiln_c99_node_net(host, result, code, receiver, args, argc, depth, thrown);
    }
    if ((groups & KILN_C99_GROUP_NODE_RESPONSE)) {
        return kiln_c99_node_http(host, result, code, receiver, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_NET_CONNECT) {
        return kiln_c99_node_net(host, result, code, receiver, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_URL_SEARCH_PARAMS_GET) {
        return kiln_c99_url_search_params_get(result, receiver, args, argc, thrown);
    }
    if ((groups & KILN_C99_GROUP_BUFFER)) {
        return kiln_c99_buffer_method(result, code, receiver, args, argc, thrown);
    }
    if ((groups & KILN_C99_GROUP_NODE_UTIL)) {
        return kiln_c99_node_util(host, result, code, args, argc, depth, thrown);
    }
    if ((groups & KILN_C99_GROUP_NODE_DIAGNOSTICS)) {
        return kiln_c99_node_diagnostics(host, result, code, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_HTTPS_CREATE_SERVER) {
        return kiln_c99_throw_error(
            thrown, KILN_C99_BUILTIN_ERROR,
            "node:https requires a TLS host capability that this realm does not provide");
    }
    if (code == KILN_C99_METHOD_NODE_HTTP2_CREATE_SERVER ||
        code == KILN_C99_METHOD_NODE_HTTP2_CREATE_SECURE_SERVER) {
        return kiln_c99_throw_error(
            thrown, KILN_C99_BUILTIN_ERROR,
            "node:http2 requires an HTTP/2 host capability that this realm does not provide");
    }
    if (code == KILN_C99_METHOD_NODE_DNS_LOOKUP) {
        return kiln_c99_node_dns_lookup(host, result, args, argc, depth, thrown);
    }
    if (code == KILN_C99_METHOD_SYMBOL_FOR) {
        return kiln_c99_symbol_for(result, argc > 0 ? args[0] : kiln_c99_undefined());
    }
    if (code == KILN_C99_METHOD_SYMBOL_KEY_FOR) {
        *result = kiln_c99_symbol_key_for(argc > 0 ? args[0] : kiln_c99_undefined());
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_OS_HOSTNAME) {
        *result = kiln_c99_copy_text("kiln", 4);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_OS_NETWORK_INTERFACES) {
        kiln_value interfaces;
        kiln_value loopback;
        kiln_value entry;
        status = kiln_c99_new_object(&interfaces);
        if (status == KILN_C99_OK) status = kiln_c99_new_array(&loopback);
        if (status == KILN_C99_OK) status = kiln_c99_new_object(&entry);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, entry, "address", 7,
                                  kiln_c99_copy_text("127.0.0.1", 9), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, entry, "family", 6,
                                  kiln_c99_copy_text("IPv4", 4), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, entry, "internal", 8, kiln_c99_boolean(1), 0, NULL);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(loopback, entry);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, interfaces, "loopback", 8, loopback, 0, NULL);
        if (status == KILN_C99_OK) *result = interfaces;
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_TTY_ISATTY) {
        /* Native Node execution does not inherit the embedding process's terminal descriptors.
         * This is the same deterministic contract as the interpreter and avoids an ambient
         * isatty(3) read crossing the realm boundary. */
        *result = kiln_c99_boolean(0);
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
    if ((groups & KILN_C99_GROUP_NODE_STREAM)) {
        return kiln_c99_node_stream(host, result, code, receiver, args, argc, depth, thrown);
    }
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
        status = kiln_realm_now(&milliseconds);
        if (status == KILN_C99_OK) *result = kiln_c99_number(milliseconds);
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_FS_EXISTS_SYNC) {
        /* A deterministic realm has no ambient synchronous filesystem. Its captured module
         * graph is resolved before execution, so an optional host path is conservatively absent.
         * Host-backed filesystem work remains asynchronous and crosses the realm-op boundary. */
        *result = kiln_c99_boolean(0);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_FS_READ_FILE ||
        code == KILN_C99_METHOD_NODE_FS_STAT ||
        code == KILN_C99_METHOD_NODE_FS_WRITE_FILE ||
        code == KILN_C99_METHOD_NODE_STAT_IS_FILE ||
        code == KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY) {
        return kiln_c99_node_fs(host, result, code, receiver, args, argc, depth, thrown);
    }
    if ((groups & KILN_C99_GROUP_NODE_PATH)) {
        return kiln_c99_node_path(result, code, args, argc, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_ASSERT_OK) {
        if (!kiln_c99_truthy(argc > 0 ? args[0] : kiln_c99_undefined())) {
            kiln_value message = argc > 1 ? kiln_c99_text(args[1])
                                          : kiln_c99_copy_text(
                                                "The expression evaluated to a falsy value", 41);
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR,
                                        message.ptr != NULL ? message.ptr :
                                        "The expression evaluated to a falsy value");
        }
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_WORKER_CONSTRUCTOR) {
        return kiln_c99_throw_error(
            thrown, KILN_C99_BUILTIN_ERROR,
            "node:worker_threads requires an isolated realm worker capability");
    }
    if (code == KILN_C99_METHOD_NODE_MODULE_CREATE_REQUIRE) {
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
    if (code == KILN_C99_METHOD_NODE_URL_FILE_URL_TO_PATH) {
        kiln_value input = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
        kiln_value encoded;
        kiln_value decode_args[1];
        if (input.tag != KILN_VALUE_STRING || input.len < 7 ||
            memcmp(input.ptr, "file://", 7) != 0)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "fileURLToPath requires a file: URL");
        encoded = kiln_c99_copy_text(input.ptr + 7, input.len - 7);
        decode_args[0] = encoded;
        return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_DECODE_URI_COMPONENT,
                                      kiln_c99_undefined(), decode_args, 1, depth, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_URL_PATH_TO_FILE_URL) {
        kiln_value input = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
        kiln_text url = {0};
        kiln_value construct_args[1];
        if (input.tag != KILN_VALUE_STRING)
            return kiln_c99_throw_not_a(thrown, input, "string path");
        status = kiln_c99_text_append_c(&url, "file://");
        if (status == KILN_C99_OK && (input.len == 0 || input.ptr[0] != '/'))
            status = kiln_c99_text_append_c(&url, "/");
        if (status == KILN_C99_OK) status = kiln_c99_text_append(&url, input.ptr, input.len);
        if (status != KILN_C99_OK) { free(url.bytes); return status; }
        construct_args[0] = kiln_c99_text_value(&url);
        return kiln_c99_url_construct(result, construct_args, 1, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_URL_PARSE) {
        return kiln_c99_url_construct(result, args, argc, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_QUERYSTRING_PARSE) {
        return kiln_c99_node_querystring_parse(result, args, argc);
    }
    if (code == KILN_C99_METHOD_NODE_ASYNC_RESOURCE) {
        kiln_value resource = receiver;
        if (resource.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&resource);
            if (status != KILN_C99_OK) return status;
        }
        status = kiln_c99_define_method(
            resource, "bind", 4,
            kiln_c99_method(KILN_C99_METHOD_NODE_ASYNC_RESOURCE_BIND));
        if (status == KILN_C99_OK)
            status = kiln_c99_define_method(
                resource, "emitDestroy", 11,
                kiln_c99_method(KILN_C99_METHOD_NODE_ASYNC_RESOURCE_DESTROY));
        *result = resource;
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_ASYNC_RESOURCE_BIND) {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value this_value = argc > 1 ? args[1] : kiln_c99_undefined();
        if (!kiln_c99_is_callable(callback))
            return kiln_c99_throw_not_a(thrown, callback, "function");
        return kiln_c99_bind(result, callback, this_value);
    }
    if (code == KILN_C99_METHOD_NODE_ASYNC_RESOURCE_DESTROY) {
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_ASYNC_LOCAL_STORAGE) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Class constructor AsyncLocalStorage cannot be invoked without 'new'");
    }
    if (code == KILN_C99_METHOD_NODE_ALS_GET_STORE) {
        if (receiver.tag != KILN_VALUE_OBJECT) {
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        int status = kiln_c99_get(host, receiver, "*store", 6, result, depth, thrown);
        if (status != KILN_C99_OK) *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_ALS_ENTER_WITH) {
        if (receiver.tag == KILN_VALUE_OBJECT) {
            kiln_value store = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_c99_put(host, receiver, "*store", 6, store, 0, NULL);
        }
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_ALS_DISABLE) {
        if (receiver.tag == KILN_VALUE_OBJECT) {
            kiln_c99_put(host, receiver, "*store", 6, kiln_c99_undefined(), 0, NULL);
        }
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_ALS_RUN) {
        if (argc < 2) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "AsyncLocalStorage.run requires at least 2 arguments");
        }
        kiln_value store = args[0];
        kiln_value callback = args[1];
        if (!kiln_c99_is_callable(callback)) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "AsyncLocalStorage callback is not a function");
        }
        kiln_value old_store = kiln_c99_undefined();
        if (receiver.tag == KILN_VALUE_OBJECT) {
            kiln_c99_get(host, receiver, "*store", 6, &old_store, depth, thrown);
            kiln_c99_put(host, receiver, "*store", 6, store, 0, NULL);
        }
        const kiln_value *cb_args = argc > 2 ? args + 2 : NULL;
        size_t cb_argc = argc > 2 ? argc - 2 : 0;
        int status = kiln_c99_call(host, result, callback, kiln_c99_undefined(), cb_args, cb_argc, depth, thrown);
        if (receiver.tag == KILN_VALUE_OBJECT) {
            kiln_c99_put(host, receiver, "*store", 6, old_store, 0, NULL);
        }
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_ALS_EXIT) {
        if (argc < 1) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "AsyncLocalStorage.exit requires a callback");
        }
        kiln_value callback = args[0];
        if (!kiln_c99_is_callable(callback)) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "AsyncLocalStorage callback is not a function");
        }
        kiln_value old_store = kiln_c99_undefined();
        if (receiver.tag == KILN_VALUE_OBJECT) {
            kiln_c99_get(host, receiver, "*store", 6, &old_store, depth, thrown);
            kiln_c99_put(host, receiver, "*store", 6, kiln_c99_undefined(), 0, NULL);
        }
        const kiln_value *cb_args = argc > 1 ? args + 1 : NULL;
        size_t cb_argc = argc > 1 ? argc - 1 : 0;
        int status = kiln_c99_call(host, result, callback, kiln_c99_undefined(), cb_args, cb_argc, depth, thrown);
        if (receiver.tag == KILN_VALUE_OBJECT) {
            kiln_c99_put(host, receiver, "*store", 6, old_store, 0, NULL);
        }
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_CRYPTO_RANDOM_UUID) {
        static const char hex[] = "0123456789abcdef";
        uint8_t bytes[16];
        uint64_t first;
        uint64_t second;
        char *uuid;
        size_t index;
        size_t at = 0;
        status = kiln_c99_entropy_word(host, &first);
        if (status == KILN_C99_OK) status = kiln_c99_entropy_word(host, &second);
        if (status != KILN_C99_OK) return status;
        for (index = 0; index < 8; ++index) {
            bytes[index] = (uint8_t)(first >> (56 - index * 8));
            bytes[index + 8] = (uint8_t)(second >> (56 - index * 8));
        }
        bytes[6] = (uint8_t)((bytes[6] & 0x0f) | 0x40);
        bytes[8] = (uint8_t)((bytes[8] & 0x3f) | 0x80);
        uuid = kiln_c99_gc_alloc_text(36);
        if (uuid == NULL) return KILN_C99_OUT_OF_MEMORY;
        for (index = 0; index < 16; ++index) {
            if (index == 4 || index == 6 || index == 8 || index == 10) uuid[at++] = '-';
            uuid[at++] = hex[bytes[index] >> 4];
            uuid[at++] = hex[bytes[index] & 0x0f];
        }
        *result = kiln_c99_value(KILN_VALUE_STRING, 0, uuid, 36);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_CRYPTO_CREATE_HASH) {
        return kiln_c99_throw_error(
            thrown, KILN_C99_BUILTIN_ERROR,
            "node:crypto createHash is not yet implemented by the native crypto provider");
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
        *result = kiln_c99_copy_text("/", 1);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_PROCESS_HRTIME ||
        code == KILN_C99_METHOD_PROCESS_HRTIME_BIGINT) {
        int64_t high;
        uint64_t low;
        status = kiln_realm_now_nanoseconds(&high, &low);
        if (status != KILN_C99_OK) return status;
        if (code == KILN_C99_METHOD_PROCESS_HRTIME_BIGINT) {
            *result = kiln_c99_number((double)low);
            return KILN_C99_OK;
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
                                     ? kiln_c99_inspect(args[index])
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
    if ((groups & KILN_C99_GROUP_TEMPORAL)) {
        return kiln_c99_temporal_method(host, result, code, receiver, args, argc, depth, thrown);
    }
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
            status = kiln_c99_to_number_checked(host, element, &converted, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            kiln_c99_typed_set(kiln_c99_typed_array_of(*result), index, converted);
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
        if ((groups & KILN_C99_GROUP_MATH_NUMBER) ||
            code == KILN_C99_METHOD_GLOBAL_IS_NAN || code == KILN_C99_METHOD_GLOBAL_IS_FINITE) {
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
            } else if (list.tag != KILN_VALUE_UNDEFINED && list.tag != KILN_VALUE_NULL) {
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
        case KILN_C99_METHOD_FUNCTION_BIND:
            return kiln_c99_bind_with(result, receiver,
                                      argc > 0 ? args[0] : kiln_c99_undefined(),
                                      argc > 1 ? args + 1 : NULL, argc > 1 ? argc - 1 : 0);
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
                !kiln_c99_same_name(args[0].ptr, args[0].len, "abort")) {
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
                        held->elements[write++] = held->elements[index];
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
            entry->callback = callback;
            entry->argc = 1;
            entry->args = (kiln_value *)malloc(sizeof *entry->args);
            if (entry->args == NULL) {
                kiln_c99_gc_release(entry);
                return KILN_C99_OUT_OF_MEMORY;
            }
            entry->args[0] = reason;
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
            size_t byte;
            status = kiln_c99_typed_alloc(result, KILN_TYPED_UINT8, text.len);
            if (status != KILN_C99_OK) return status;
            for (byte = 0; byte < text.len; ++byte)
                kiln_c99_typed_set(kiln_c99_typed_array_of(*result), byte,
                                   (double)(unsigned char)text.ptr[byte]);
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
        case KILN_C99_METHOD_SET_IMMEDIATE: {
            kiln_timer *entry;
            double delay = 0.0;
            if (code != KILN_C99_METHOD_SET_IMMEDIATE && argc > 1) {
                status = kiln_c99_to_number_checked(host, args[1], &delay, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            uint64_t id = 0;
            /* A negative, missing or NaN delay is zero: "as soon as possible", which is still
             * after everything already queued. */
            if (!(isfinite(delay) && delay > 0.0)) {
                delay = 0.0;
            }
            entry = (kiln_timer *)kiln_c99_gc_alloc(KILN_GC_TIMER, sizeof *entry);
            if (entry == NULL) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            entry->callback = argc > 0 ? args[0] : kiln_c99_undefined();
            /* Everything after the delay is handed to the callback, which is how a timer carries
             * state without a closure. */
            entry->argc = code == KILN_C99_METHOD_SET_IMMEDIATE
                              ? (argc > 1 ? argc - 1 : 0)
                              : (argc > 2 ? argc - 2 : 0);
            entry->args = NULL;
            if (entry->argc != 0) {
                entry->args = (kiln_value *)malloc(entry->argc * sizeof *entry->args);
                if (entry->args == NULL) {
                    kiln_c99_gc_release(entry);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                memcpy(entry->args,
                       args + (code == KILN_C99_METHOD_SET_IMMEDIATE ? 1 : 2),
                       entry->argc * sizeof *entry->args);
            }
            status = code == KILN_C99_METHOD_SET_INTERVAL
                         ? kiln_realm_set_interval(delay, kiln_c99_run_timer,
                                                   kiln_c99_drop_timer, entry, &id)
                         : kiln_realm_set_timeout(delay, kiln_c99_run_timer,
                                                  kiln_c99_drop_timer, entry, &id);
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
            size_t at;
            if (subject.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            for (at = 0; at < subject.len; ++at) {
                char character = subject.ptr[at];
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
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            *result = kiln_c99_text_value(&built);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_DECODE_URI:
        case KILN_C99_METHOD_DECODE_URI_COMPONENT: {
            kiln_value subject = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            kiln_text built = {NULL, 0, 0};
            size_t at = 0;
            if (subject.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            while (at < subject.len) {
                int high;
                int low;
                char byte;
                if (subject.ptr[at] != '%') {
                    if (kiln_c99_text_append(&built, subject.ptr + at, 1) != KILN_C99_OK) {
                        free(built.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    at += 1;
                    continue;
                }
                high = at + 2 < subject.len ? kiln_c99_hex_digit(subject.ptr[at + 1]) : -1;
                low = at + 2 < subject.len ? kiln_c99_hex_digit(subject.ptr[at + 2]) : -1;
                if (high < 0 || low < 0) {
                    /* A `%` not followed by two hex digits is malformed, not a literal percent.
                     * `URIError` rather than `TypeError`: that kind exists for exactly this, and
                     * it is what a program guarding `decodeURI` on user input catches. */
                    free(built.bytes);
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
                    if (kiln_c99_text_append(&built, subject.ptr + at, 3) != KILN_C99_OK) {
                        free(built.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                } else if (kiln_c99_text_append(&built, &byte, 1) != KILN_C99_OK) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                at += 3;
            }
            if (!kiln_c99_valid_utf8(built.bytes, built.len)) {
                /* The escapes named bytes that are not a string. */
                free(built.bytes);
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_URI_ERROR, "URI malformed");
            }
            *result = kiln_c99_text_value(&built);
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
            size_t at = 0;
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
            bytes = text.ptr;
            len = text.len;
            /* Both read as far as they can and stop, rather than rejecting a string with a tail:
             * `parseInt('12px')` is 12. That tolerance is the whole difference between these and
             * `Number(x)`, which rejects the same input. */
            while (at < len && kiln_c99_is_trimmed((uint16_t)(unsigned char)bytes[at])) {
                at += 1;
            }
            if (code == KILN_C99_METHOD_PARSE_INT) {
                double radix = argc > 1 ? kiln_c99_as_number(args[1]) : 0.0;
                double magnitude = 0.0;
                int base;
                int negative = 0;
                size_t digits_at;
                radix = isfinite(radix) ? trunc(radix) : 0.0;
                if (!(radix == 0.0 || (radix >= 2.0 && radix <= 36.0))) {
                    *result = kiln_c99_number(0.0 / 0.0);
                    return KILN_C99_OK;
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
                    /* Folded as a double rather than accumulated as an integer: the result is a
                     * double either way, and a long run of digits has to saturate toward infinity
                     * rather than overflow. */
                    magnitude = magnitude * (double)base + (double)digit;
                    at += 1;
                }
                *result = kiln_c99_number(at == digits_at ? 0.0 / 0.0
                                                          : (negative ? -magnitude : magnitude));
                return KILN_C99_OK;
            }
            {
                /* The longest prefix that is a decimal literal, scanned rather than handed to
                 * `strtod` outright: C's parser also accepts `0x1f`, `inf` and `nan`, none of
                 * which this grammar has -- `parseFloat('0x1f')` is 0. */
                size_t start = at;
                size_t scan;
                size_t end;
                size_t before = 0;
                size_t after = 0;
                double parsed;
                if (at < len && (bytes[at] == '-' || bytes[at] == '+')) {
                    at += 1;
                }
                if (len - at >= 8 && memcmp(bytes + at, "Infinity", 8) == 0) {
                    *result = kiln_c99_number(bytes[start] == '-' ? -1.0 / 0.0 : 1.0 / 0.0);
                    return KILN_C99_OK;
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
                    *result = kiln_c99_number(0.0 / 0.0);
                    return KILN_C99_OK;
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
                /* The scanned prefix is a `StringNumericLiteral` by construction, so the
                 * provider's parser is exactly the right reader for it -- and it is the same one
                 * `Number(x)` uses, which is what keeps the two agreeing. `strtod` would read the
                 * process locale's decimal point, and an embedded module runs in its host's. */
                parsed = kiln_string_to_number(bytes + start, end - start);
                *result = kiln_c99_number(parsed);
                return KILN_C99_OK;
            }
        }
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
            int64_t ranges[64];
            /* `RegExp.prototype.test.call({})` has no pattern to match with. Note that
             * `toString` on a foreign receiver is *not* this: it answers rather than throwing,
             * so that guard stays a refusal. */
            if (receiver.tag != KILN_VALUE_REGEXP) {
                return kiln_c99_throw_not_a(thrown, receiver, "function");
            }
            regexp = kiln_c99_regexp_of(receiver);
            subject = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            if (subject.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            int is_ascii = 0;
            if (regexp->cached_subject_ptr == subject.ptr && regexp->cached_subject_len == subject.len) {
                is_ascii = regexp->cached_is_ascii;
            } else {
                is_ascii = kiln_c99_string_is_ascii(subject);
                regexp->cached_subject_ptr = subject.ptr;
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
                status = regexp->matcher_ascii((const uint8_t *)subject.ptr, subject.len, start,
                                               ranges, sizeof ranges / sizeof *ranges, &groups, &matched);
            } else {
                if (is_ascii && subject.len <= 512) {
                    size_t i;
                    stack_used = 1;
                    count = subject.len;
                    units = stack_units;
                    for (i = 0; i < count; ++i) {
                        stack_units[i] = (uint16_t)(unsigned char)subject.ptr[i];
                    }
                } else {
                    if (kiln_c99_to_units(subject, &units, &count) != KILN_C99_OK) {
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
                status = kiln_c99_regexp_find(regexp, units, count, start, ranges,
                                              sizeof ranges / sizeof *ranges, &groups, &matched);
            }
            if (status != KILN_C99_OK) {
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
                if (units != NULL && !stack_used) {
                    free(units);
                }
                *result = code == KILN_C99_METHOD_REGEXP_TEST ? kiln_c99_boolean(0)
                                                              : kiln_c99_null();
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_REGEXP_TEST) {
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
                    int can_reuse = (regexp->cached_group_count == groups);
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
                            arr->elements[arr->count++] = regexp->cached_elements[group];
                        }
                    } else {
                        for (group = 0; group < groups && status == KILN_C99_OK; ++group) {
                            int64_t from = ranges[group * 2];
                            int64_t to = ranges[group * 2 + 1];
                            kiln_value val;
                            if (from < 0) {
                                val = kiln_c99_undefined();
                            } else if (is_ascii) {
                                val = kiln_c99_copy_text(subject.ptr + (size_t)from,
                                                         (size_t)(to - from));
                            } else {
                                val = kiln_c99_from_units(units + (size_t)from, (size_t)(to - from));
                            }
                            kiln_val bval = kiln_val_from_boundary(val);
                            arr->elements[arr->count++] = bval;
                            if (group < 32) {
                                regexp->cached_elements[group] = bval;
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
                    arr->match_input = subject;
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
        case KILN_C99_METHOD_IS_ARRAY:
            /* Follows a proxy to what it stands in front of, which is what the operation is
             * specified to do -- a proxy of an array *is* an array to every library that
             * branches on this. */
            *result = kiln_c99_boolean(
                argc > 0 && kiln_c99_proxy_inner(args[0]).tag == KILN_VALUE_ARRAY);
            return KILN_C99_OK;
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
            if (argc == 0) {
                KILN_C99_REFUSE("Object.keys, Object.values or Object.entries with no argument");
            }
            if (args[0].tag == KILN_VALUE_PROXY) {
                /* A proxy has no store to walk: its keys come from the `ownKeys` trap and each
                 * value from a `get`, which is the pair of trips the language specifies and the
                 * reason a logging handler sees both. */
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
                    if (extra->properties == NULL && extra->shape != NULL) {
                        for (index = 0; index < extra->shape->property_count && status == KILN_C99_OK; ++index) {
                            const char *skey = extra->shape->keys[index];
                            size_t key_len = kiln_c99_key_len(skey);
                            kiln_value key;
                            kiln_value val;
                            if (key_len >= 2 && skey[0] == '@' && skey[1] == '@') {
                                continue;
                            }
                            key = kiln_c99_copy_text(skey, key_len);
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
                            key = kiln_c99_copy_text(extra->properties[index].key,
                                                     extra->properties[index].key_len);
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
                    kiln_value value = kiln_c99_number(kiln_c99_typed_get(view, index));
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
            if (args[0].tag == KILN_VALUE_FUNCTION) {
                /* A function is an object: `f.version = 2` is an own enumerable property of it,
                 * and it is kept on the closure's store rather than in a slot list of its own.
                 * `name`, `length` and `prototype` are read from the closure and are not in that
                 * store, which is right -- none of the three is enumerated. */
                kiln_value store = kiln_c99_closure_of(args[0])->properties;
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
            } else if (args[0].tag == KILN_VALUE_PROMISE) {
                kiln_value store = kiln_c99_promise_of(args[0])->properties;
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
            if (object->properties == NULL && object->shape != NULL) {
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
                    const char *skey = object->shape->keys[index];
                    size_t key_len = kiln_c99_key_len(skey);
                    kiln_value key;
                    kiln_value val;
                    if (key_len >= 2 && skey[0] == '@' && skey[1] == '@') {
                        continue;
                    }
                    key = kiln_c99_copy_text(skey, key_len);
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
            if (object->shape != NULL) {
                kiln_c99_ensure_dictionary(object);
            }
            status = kiln_c99_new_array(&built);
            for (index = 0; index < object->count && status == KILN_C99_OK; ++index) {
                kiln_value key;
                if (!object->properties[index].enumerable) {
                    continue;
                }
                key = kiln_c99_copy_text(object->properties[index].key,
                                         object->properties[index].key_len);
                if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                    status = kiln_c99_array_push(built, key);
                } else if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                    status = kiln_c99_array_push(built, object->properties[index].value);
                } else {
                    kiln_value pair;
                    status = kiln_c99_new_array(&pair);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(pair, key);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(pair, object->properties[index].value);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(built, pair);
                    }
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
                 args[0].tag != KILN_VALUE_METHOD && args[0].tag != KILN_VALUE_BOUND)) {
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
                if (source->properties == NULL && source->shape != NULL) {
                    if (source->elements != NULL) {
                        for (index = 0; index < source->element_count; ++index) {
                            if (source->elements[index] == KILN_VAL_HOLE) continue;
                            char num_buf[32];
                            size_t nlen = kiln_c99_double_to_js((double)index, num_buf);
                            status = kiln_c99_put(host, target, num_buf, nlen, kiln_val_to_boundary(source->elements[index]), depth, thrown);
                            if (status != KILN_C99_OK) return status;
                        }
                    }
                    for (index = 0; index < source->shape->property_count; ++index) {
                        const char *skey = source->shape->keys[index];
                        size_t skey_len = kiln_c99_key_len(skey);
                        kiln_value sval;
                        if (skey_len >= 2 && skey[0] == '@' && skey[1] == '@') {
                            kiln_value symbol;
                            if (!kiln_c99_symbol_of_key(skey, skey_len, &symbol)) {
                                continue;
                            }
                        }
                        sval = (source->slots != NULL && index < source->slot_capacity)
                                   ? kiln_val_to_boundary(source->slots[index])
                                   : kiln_c99_undefined();
                        status = kiln_c99_put(host, target, skey, skey_len, sval, depth, thrown);
                        if (status != KILN_C99_OK) {
                            return status;
                        }
                    }
                } else {
                    if (source->shape != NULL) {
                        kiln_c99_ensure_dictionary(source);
                    }
                    for (index = 0; index < source->count; ++index) {
                        if (!source->properties[index].enumerable) {
                            continue;
                        }
                        status = kiln_c99_put(host, target, source->properties[index].key,
                                              source->properties[index].key_len,
                                              source->properties[index].value, depth, thrown);
                        if (status != KILN_C99_OK) {
                            return status;
                        }
                    }
                }
            }
            *result = target;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_DATE_NOW:
        {
            double milliseconds;
            status = kiln_c99_wall_clock_milliseconds(host, &milliseconds);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_number(milliseconds);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_COLLECTION_GET:
        case KILN_C99_METHOD_COLLECTION_SET:
        case KILN_C99_METHOD_COLLECTION_ADD:
        case KILN_C99_METHOD_COLLECTION_HAS:
        case KILN_C99_METHOD_COLLECTION_DELETE:
        case KILN_C99_METHOD_COLLECTION_CLEAR:
        case KILN_C99_METHOD_COLLECTION_FOR_EACH:
        case KILN_C99_METHOD_COLLECTION_KEYS:
        case KILN_C99_METHOD_COLLECTION_VALUES:
        case KILN_C99_METHOD_COLLECTION_ENTRIES:
        case KILN_C99_METHOD_COLLECTION_SELF: {
            kiln_collection *collection;
            kiln_value first = argc > 0 ? args[0] : kiln_c99_undefined();
            double at;
            /* `Map.prototype.get.call({})`. A collection method reached through a receiver that
             * has no backing collection is a brand check the language fails with a TypeError, not
             * a method this backend is missing -- it has the method, and the receiver is wrong. */
            if (receiver.tag != KILN_VALUE_COLLECTION) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Method called on an incompatible receiver");
            }
            collection = kiln_c99_collection_of(receiver);
            switch (code) {
            case KILN_C99_METHOD_COLLECTION_GET:
                at = kiln_c99_collection_find(collection, first);
                *result = at < 0.0 ? kiln_c99_undefined() : collection->entries[(size_t)at].value;
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_HAS:
                *result = kiln_c99_boolean(kiln_c99_collection_find(collection, first) >= 0.0);
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_SET:
            case KILN_C99_METHOD_COLLECTION_ADD:
                /* A Set stores each value under itself, which is what makes `has` one lookup
                 * rather than two shapes. Both answer with the collection, so they chain. */
                status = kiln_c99_collection_put(receiver, first,
                                                 code == KILN_C99_METHOD_COLLECTION_SET
                                                     ? (argc > 1 ? args[1] : kiln_c99_undefined())
                                                     : first);
                if (status != KILN_C99_OK) {
                    return status;
                }
                *result = receiver;
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_DELETE:
                at = kiln_c99_collection_find(collection, first);
                if (at < 0.0) {
                    *result = kiln_c99_boolean(0);
                    return KILN_C99_OK;
                }
                /* A hole, not a removal: a walk already past this point must not have what
                 * follows shifted under it, and one not yet there must not see this entry
                 * again. `x,z` rather than `x,y,z` when the loop deletes `y` after visiting `x`. */
                collection->entries[(size_t)at].dead = 1;
                collection->live -= 1;
                kiln_c99_collection_hash_remove(collection, (size_t)at, first);
                *result = kiln_c99_boolean(1);
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_CLEAR:
                collection->count = 0;
                collection->live = 0;
                if (collection->hash_table != NULL) {
                    memset(collection->hash_table, 0, collection->hash_capacity * sizeof(uint32_t));
                }
                *result = kiln_c99_undefined();
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_FOR_EACH: {
                /* The callback takes `(value, key, collection)`. A Set passes its value twice,
                 * which is the specification's way of giving both one signature. */
                size_t length = collection->count;
                for (index = 0; index < length && index < collection->count; ++index) {
                    kiln_value visit[3];
                    kiln_value ignored;
                    if (collection->entries[index].dead) {
                        continue;
                    }
                    visit[0] = collection->entries[index].value;
                    visit[1] = collection->entries[index].key;
                    visit[2] = receiver;
                    status = kiln_c99_call(host, &ignored, first,
                                           argc > 1 ? args[1] : kiln_c99_undefined(), visit, 3,
                                           depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                *result = kiln_c99_undefined();
                return KILN_C99_OK;
            }
            default: {
                /* A *live* iterator, holding the collection rather than a copy of it: a delete
                 * during the walk has to be seen, and a snapshot is exactly what cannot see one.
                 * What each step hands back is decided here, where the collection's kind is
                 * known, so the iterator itself needs no reference back to it. */
                int pairs = code == KILN_C99_METHOD_COLLECTION_ENTRIES ||
                            (code == KILN_C99_METHOD_COLLECTION_SELF && collection->is_map);
                int wants_keys = code == KILN_C99_METHOD_COLLECTION_KEYS;
                kiln_value iterator;
                status = kiln_c99_new_object(&iterator);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "@@iterated", 10, receiver, depth,
                                          thrown);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "@@index", 7, kiln_c99_number(0.0),
                                          depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    /* 0 the value, 1 the key, 2 a `[key, value]` pair. */
                    status = kiln_c99_put(host, iterator, "@@part", 6,
                                          kiln_c99_number(pairs ? 2.0 : (wants_keys ? 1.0 : 0.0)),
                                          depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "next", 4,
                                          kiln_c99_method(KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT),
                                          depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "@@iterator", 10,
                                          kiln_c99_method(KILN_C99_METHOD_ITERATOR_SELF), depth,
                                          thrown);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                *result = iterator;
                return KILN_C99_OK;
            }
            }
        }
        case KILN_C99_METHOD_THEN:
        case KILN_C99_METHOD_CATCH:
        case KILN_C99_METHOD_FINALLY: {
            kiln_value on_fulfilled = kiln_c99_undefined();
            kiln_value on_rejected = kiln_c99_undefined();
            /* `catch(r)` is `Invoke(this, "then", [undefined, r])` -- literally, including the
             * read -- so a thenable that is not a promise is handled by its *own* `then`, and a
             * subclass that overrides `then` has that one run. Only `catch` delegates: `then`
             * itself needs a promise to attach a reaction to, and `finally` needs a pair of
             * closures over the receiver that this backend has no way to make for a thenable. */
            if (code == KILN_C99_METHOD_CATCH && receiver.tag != KILN_VALUE_PROMISE) {
                kiln_value method;
                kiln_value forwarded[2];
                status = kiln_c99_get(host, receiver, "then", 4, &method, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (!kiln_c99_is_callable(method)) {
                    return kiln_c99_throw_not_a(thrown, method, "function");
                }
                forwarded[0] = kiln_c99_undefined();
                forwarded[1] = argc > 0 ? args[0] : kiln_c99_undefined();
                return kiln_c99_call(host, result, method, receiver, forwarded, 2, depth, thrown);
            }
            /* `Promise.prototype.then.call({})` has no promise to attach a reaction to. The
             * interpreter reports this against the receiver, and the two are compared byte for
             * byte, so the same sentence is built here. */
            if (receiver.tag != KILN_VALUE_PROMISE) {
                return kiln_c99_throw_not_a(thrown, receiver, "function");
            }
            if (code == KILN_C99_METHOD_THEN) {
                on_fulfilled = argc > 0 ? args[0] : kiln_c99_undefined();
                on_rejected = argc > 1 ? args[1] : kiln_c99_undefined();
            } else if (code == KILN_C99_METHOD_CATCH) {
                on_rejected = argc > 0 ? args[0] : kiln_c99_undefined();
            } else {
                /* `finally` runs on both sides and passes the outcome through untouched, which is
                 * what separates it from `then(f, f)`. Each side is the handler wrapped in a step
                 * that restores what it was given -- bound to the handler itself, so the pair
                 * needs no state object of its own. */
                kiln_value handler = argc > 0 ? args[0] : kiln_c99_undefined();
                status = kiln_c99_bind(&on_fulfilled,
                                       kiln_c99_method(KILN_C99_METHOD_FINALLY_PASS), handler);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_bind(&on_rejected,
                                           kiln_c99_method(KILN_C99_METHOD_FINALLY_RAISE),
                                           handler);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            return kiln_c99_then(receiver, on_fulfilled, on_rejected, result);
        }
        case KILN_C99_METHOD_PROMISE_RESOLVE:
        case KILN_C99_METHOD_PROMISE_REJECT: {
            kiln_value value = argc > 0 ? args[0] : kiln_c99_undefined();
            /* `Promise.resolve(p)` is `p`, not a promise of a promise -- which is what lets a
             * function take "a value or a promise of one" without asking which it got. */
            if (code == KILN_C99_METHOD_PROMISE_RESOLVE && value.tag == KILN_VALUE_PROMISE &&
                receiver.tag != KILN_VALUE_FUNCTION) {
                *result = value;
                return KILN_C99_OK;
            }
            status = kiln_c99_new_promise(result);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (receiver.tag == KILN_VALUE_FUNCTION) {
                kiln_value proto;
                kiln_value tally_val;
                if (kiln_c99_get(host, receiver, "prototype", 9, &proto, depth, thrown) == KILN_C99_OK) {
                    (void)kiln_c99_set_prototype(*result, proto);
                }
                if (kiln_c99_get(host, receiver, "tally", 5, &tally_val, depth, thrown) == KILN_C99_OK &&
                    tally_val.tag == KILN_VALUE_NUMBER) {
                    (void)kiln_c99_put(host, receiver, "tally", 5,
                                       kiln_c99_number(kiln_c99_as_number(tally_val) + 1.0),
                                       0, thrown);
                }
            }
            return kiln_c99_settle(*result, value, code == KILN_C99_METHOD_PROMISE_REJECT);
        }
        case KILN_C99_METHOD_PROMISE_WITH_RESOLVERS: {
            kiln_value promise;
            kiln_value resolve;
            kiln_value reject;
            status = kiln_c99_new_promise(&promise);
            if (status == KILN_C99_OK)
                status = kiln_c99_new_settler(&resolve, promise, 0);
            if (status == KILN_C99_OK)
                status = kiln_c99_new_settler(&reject, promise, 1);
            if (status == KILN_C99_OK)
                status = kiln_c99_new_object(result);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, *result, "promise", 7, promise, 0, NULL);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, *result, "resolve", 7, resolve, 0, NULL);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, *result, "reject", 6, reject, 0, NULL);
            return status;
        }
        case KILN_C99_METHOD_PROMISE_ALL:
        case KILN_C99_METHOD_PROMISE_RACE:
        case KILN_C99_METHOD_PROMISE_ALL_SETTLED:
        case KILN_C99_METHOD_PROMISE_ANY: {
            kiln_value gathered;
            kiln_value derived;
            kiln_array *given;
            /* All four are one machine differing in what each arrival does, which is why the mode
             * travels in the shared state rather than in four copies of the wiring below. */
            double mode = code == KILN_C99_METHOD_PROMISE_RACE          ? 1.0
                          : code == KILN_C99_METHOD_PROMISE_ALL_SETTLED ? 2.0
                          : code == KILN_C99_METHOD_PROMISE_ANY         ? 3.0
                                                                        : 0.0;
            int racing = mode == 1.0;
            status = kiln_c99_new_array(&gathered);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_iterate(host, gathered, argc > 0 ? args[0] : kiln_c99_undefined(),
                                      depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_new_promise(&derived);
            if (status != KILN_C99_OK) {
                return status;
            }
            given = kiln_c99_array_of(gathered);
            if (!racing && given->count == 0) {
                /* Nothing to wait for, so it is already done. `any` is the exception: with no
                 * promise to succeed, every one of them has failed. */
                if (mode == 3.0) {
                    kiln_value aggregate;
                    status = kiln_c99_aggregate_error(&aggregate, gathered);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_settle(derived, aggregate, 1);
                    }
                } else {
                    status = kiln_c99_settle(derived, gathered, 0);
                }
                *result = derived;
                return status;
            }
            {
                /* One shared state object, and one bound step per element that knows its own
                 * index. The index has to travel with the handler, and a handler is one value,
                 * so the pair is an object the step reads itself out of. */
                kiln_value state;
                kiln_value results;
                size_t index;
                status = kiln_c99_new_object(&state);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_new_array(&results);
                }
                for (index = 0; index < given->count && status == KILN_C99_OK; ++index) {
                    status = kiln_c99_array_push(results, kiln_c99_undefined());
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@results", 9, results, 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@remaining", 11,
                                          kiln_c99_number((double)given->count), 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@derived", 9, derived, 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@mode", 6, kiln_c99_number(mode), 0,
                                          NULL);
                }
                for (index = 0; index < given->count && status == KILN_C99_OK; ++index) {
                    kiln_value element;
                    kiln_value slot;
                    kiln_value step;
                    kiln_value rejecter;
                    kiln_value ignored;
                    status = kiln_c99_new_object(&slot);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(NULL, slot, "@@state", 7, state, 0, NULL);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(NULL, slot, "@@index", 7,
                                              kiln_c99_number((double)index), 0, NULL);
                    }
                    if (status != KILN_C99_OK) {
                        break;
                    }
                    element = kiln_val_to_boundary(given->elements[index]);
                    if (element.tag != KILN_VALUE_PROMISE) {
                        kiln_value immediate;
                        status = kiln_c99_new_promise(&immediate);
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_settle(immediate, element, 0);
                        }
                        element = immediate;
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_bind(&step, kiln_c99_method(KILN_C99_METHOD_ALL_STEP),
                                               slot);
                    }
                    if (status == KILN_C99_OK) {
                        /* For `all` and `race` a rejection settles the whole thing, whichever
                         * element it came from, and a settler is exactly that. The other two have
                         * something to record instead, so they get a step of their own. */
                        status = mode == 2.0 || mode == 3.0
                                     ? kiln_c99_bind(&rejecter,
                                                     kiln_c99_method(KILN_C99_METHOD_REJECT_STEP),
                                                     slot)
                                     : kiln_c99_new_settler(&rejecter, derived, 1);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_then(element, step, rejecter, &ignored);
                    }
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = derived;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_FINALLY_PASS:
        case KILN_C99_METHOD_FINALLY_RAISE: {
            kiln_value passed = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value ignored;
            if (kiln_c99_is_callable(receiver)) {
                /* The handler's own failure wins: a `finally` that throws replaces the outcome
                 * it was passing through, which is the one thing that can change it. */
                status = kiln_c99_call(host, &ignored, receiver, kiln_c99_undefined(), NULL, 0,
                                       depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            {
                /* `Promise.resolve(f()).then(() => v)`, which is the specification's own spelling
                 * and the reason it matters: a handler that returns a promise is *waited for*, and
                 * the extra turn that costs is visible in the order things print. Settling here
                 * would put the outcome a turn early. */
                kiln_value awaited;
                kiln_value restore;
                status = kiln_c99_new_promise(&awaited);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_settle(awaited, ignored, 0);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_bind(
                        &restore,
                        kiln_c99_method(code == KILN_C99_METHOD_FINALLY_RAISE
                                            ? KILN_C99_METHOD_FINALLY_RERAISE
                                            : KILN_C99_METHOD_FINALLY_RESTORE),
                        passed);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                return kiln_c99_then(awaited, restore, kiln_c99_undefined(), result);
            }
        }
        case KILN_C99_METHOD_ADOPT_STEP: {
            /* Two settlers bound to the outer promise: whichever the inner one reaches, the outer
             * one follows. */
            kiln_promise *inner;
            kiln_reaction adopt;
            if (receiver.tag != KILN_VALUE_PROMISE || argc == 0 ||
                args[0].tag != KILN_VALUE_PROMISE) {
                /* Named, because this runs as a microtask and a microtask's refusal has no
                 * instruction to be attributed to: without a detail the report is the phase it
                 * happened in and nothing else. `@@adopt` is scheduled only by `kiln_c99_settle`
                 * and only with two promises, so reaching here means something else resolved a
                 * promise with a value this backend cannot follow. */
                KILN_C99_REFUSE("promise adoption was handed something that is not a pair of "
                                "promises");
            }
            inner = kiln_c99_promise_of(args[0]);
            status = kiln_c99_new_settler(&adopt.on_fulfilled, receiver, 0);
            if (status == KILN_C99_OK) {
                status = kiln_c99_new_settler(&adopt.on_rejected, receiver, 1);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            adopt.derived = kiln_c99_undefined();
            adopt.continuation = 0;
            *result = kiln_c99_undefined();
            if (inner->state != KILN_C99_PENDING) {
                return kiln_c99_schedule(inner, &adopt);
            }
            status = kiln_c99_reserve((void **)&inner->reactions, &inner->capacity,
                                      inner->count + 1, sizeof *inner->reactions);
            if (status != KILN_C99_OK) {
                return status;
            }
            inner->reactions[inner->count] = adopt;
            inner->count += 1;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_FINALLY_RESTORE:
            /* The outcome travelled here as the receiver, because a reaction is handed the value
             * it is reacting to and that is the handler's result rather than the original. */
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_FINALLY_RERAISE:
            /* A rejection travels on, which a reaction says by throwing. */
            *thrown = receiver;
            return KILN_C99_THROWN;
        case KILN_C99_METHOD_ALL_STEP:
        case KILN_C99_METHOD_REJECT_STEP: {
            kiln_value state;
            kiln_value at;
            kiln_value results;
            kiln_value remaining;
            kiln_value derived;
            kiln_value mode;
            kiln_value arrived = argc > 0 ? args[0] : kiln_c99_undefined();
            int rejected = code == KILN_C99_METHOD_REJECT_STEP;
            if (receiver.tag != KILN_VALUE_OBJECT) {
                /* Another microtask with no instruction of its own to blame. The slot object is
                 * built by `Promise.all` and its siblings, so a receiver that is not one means a
                 * combinator's bookkeeping went missing rather than a program did anything. */
                KILN_C99_REFUSE("a promise combinator's per-element slot was not the object it "
                                "builds");
            }
            status = kiln_c99_get(host, receiver, "@@state", 7, &state, depth, thrown);
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, receiver, "@@index", 7, &at, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@results", 9, &results, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@remaining", 11, &remaining, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@derived", 9, &derived, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@mode", 6, &mode, depth, thrown);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_undefined();
            if (!rejected && (kiln_c99_as_number(mode) == 1.0 || kiln_c99_as_number(mode) == 3.0)) {
                /* `race` and `any` are both settled by the first arrival that counts, and the
                 * rest are ignored -- which `settle` already does by refusing a second
                 * settlement. For `any` only a fulfilment counts, which is what puts this on the
                 * fulfilment side alone. */
                return kiln_c99_settle(derived, arrived, 0);
            }
            if (results.tag == KILN_VALUE_ARRAY) {
                kiln_array *array = kiln_c99_array_of(results);
                size_t slot = (size_t)kiln_c99_as_number(at);
                kiln_value recorded = arrived;
                if (kiln_c99_as_number(mode) == 2.0) {
                    /* `allSettled` reports the outcome as well as the value, so every element
                     * gets a record rather than a bare result. */
                    status = kiln_c99_new_object(&recorded);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(host, recorded, "status", 6,
                                              kiln_c99_copy_text(rejected ? "rejected"
                                                                          : "fulfilled",
                                                                 rejected ? 8 : 9),
                                              depth, thrown);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(host, recorded, rejected ? "reason" : "value",
                                              rejected ? 6 : 5, arrived, depth, thrown);
                    }
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                if (slot < array->count) {
                    array->elements[slot] = kiln_val_from_boundary(recorded);
                }
            }
            {
                double left = kiln_c99_as_number(remaining) - 1.0;
                status = kiln_c99_put(host, state, "@@remaining", 11, kiln_c99_number(left), depth,
                                      thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (left <= 0.0) {
                    if (kiln_c99_as_number(mode) == 3.0) {
                        /* Every one of them failed, and the reasons are what the caller gets. */
                        kiln_value aggregate;
                        status = kiln_c99_aggregate_error(&aggregate, results);
                        if (status != KILN_C99_OK) {
                            return status;
                        }
                        return kiln_c99_settle(derived, aggregate, 1);
                    }
                    return kiln_c99_settle(derived, results, 0);
                }
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ASYNC_RESUME:
        case KILN_C99_METHOD_ASYNC_THROW:
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                /* `@@resume` and `@@throw` are bound to the suspended frame that awaited, and
                 * they run a whole turn later -- so this refusal, like the two above, arrives
                 * with no instruction behind it and must say for itself what it wanted. */
                KILN_C99_REFUSE("an await resumed against something that is not a suspended frame");
            }
            *result = kiln_c99_undefined();
            /* The same two handlers serve both drivers: what an arrival means depends on the
             * frame it is arriving at, not on who attached them. */
            return kiln_c99_generator_of(receiver)->async_generator
                       ? kiln_c99_async_generator_step(
                             host, receiver, argc > 0 ? args[0] : kiln_c99_undefined(),
                             code == KILN_C99_METHOD_ASYNC_THROW, depth, thrown)
                       : kiln_c99_async_step(host, receiver,
                                             argc > 0 ? args[0] : kiln_c99_undefined(),
                                             code == KILN_C99_METHOD_ASYNC_THROW, depth, thrown,
                                             NULL, 0);
        case KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT: {
            kiln_value source;
            kiln_value at;
            kiln_value part;
            kiln_value step;
            kiln_value produced = kiln_c99_undefined();
            kiln_collection *collection;
            size_t position;
            int finished;
            if (receiver.tag != KILN_VALUE_OBJECT) {
                KILN_C99_REFUSE("a collection iterator whose state is not the object it builds");
            }
            status = kiln_c99_get(host, receiver, "@@iterated", 10, &source, depth, thrown);
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, receiver, "@@index", 7, &at, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, receiver, "@@part", 6, &part, depth, thrown);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            if (source.tag != KILN_VALUE_COLLECTION) {
                KILN_C99_REFUSE("a collection iterator whose source is not a collection");
            }
            collection = kiln_c99_collection_of(source);
            position = (size_t)kiln_c99_as_number(at);
            /* Past every dead entry: one deleted ahead of the cursor is gone, and one deleted
             * behind it has already been handed over. */
            while (position < collection->count && collection->entries[position].dead) {
                position += 1;
            }
            finished = position >= collection->count;
            if (!finished) {
                double which = kiln_c99_as_number(part);
                if (which == 2.0) {
                    status = kiln_c99_new_array(&produced);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(produced,
                                                     collection->entries[position].key);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(produced,
                                                     collection->entries[position].value);
                    }
                } else {
                    produced = which == 1.0 ? collection->entries[position].key
                                            : collection->entries[position].value;
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, receiver, "@@index", 7,
                                          kiln_c99_number((double)(position + 1)), depth, thrown);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            status = kiln_c99_new_object(&step);
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(host, step, "value", 5, produced, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(host, step, "done", 4, kiln_c99_boolean(finished), depth,
                                      thrown);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = step;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ITERATOR_NEXT: {
            kiln_value items;
            kiln_value at;
            kiln_value step;
            kiln_array *array;
            kiln_typed_array *typed;
            kiln_value part_value;
            size_t position;
            int finished;
            int part = 0;
            if (receiver.tag != KILN_VALUE_OBJECT) {
                KILN_C99_REFUSE("an array iterator whose state is not the object it builds");
            }
            status = kiln_c99_get(host, receiver, "@@iterated", 10, &items, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_get(host, receiver, "@@index", 7, &at, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (items.tag != KILN_VALUE_ARRAY && items.tag != KILN_VALUE_TYPED_ARRAY) {
                KILN_C99_REFUSE("an array iterator whose source is not an array or a typed array");
            }
            position = (size_t)kiln_c99_as_number(at);
            if (items.tag == KILN_VALUE_TYPED_ARRAY) {
                typed = kiln_c99_typed_array_of(items);
                finished = position >= typed->length;
            } else {
                array = kiln_c99_array_of(items);
                finished = position >= array->count;
            }
            if (items.tag == KILN_VALUE_TYPED_ARRAY &&
                kiln_c99_get(host, receiver, "@@iterpart", 10, &part_value, depth, thrown) ==
                    KILN_C99_OK) {
                part = (int)kiln_c99_as_number(part_value);
            }
            if (!finished) {
                status = kiln_c99_put(host, receiver, "@@index", 7,
                                      kiln_c99_number((double)(position + 1)), depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            status = kiln_c99_new_object(&step);
            if (status != KILN_C99_OK) {
                return status;
            }
            {
                kiln_value produced = kiln_c99_undefined();
                if (!finished) {
                    if (items.tag == KILN_VALUE_TYPED_ARRAY) {
                        produced = part == 2 ? kiln_c99_number((double)position)
                                             : kiln_c99_number(kiln_c99_typed_get(typed, position));
                        if (part == 1) {
                            status = kiln_c99_new_array(&produced);
                            if (status == KILN_C99_OK) {
                                status = kiln_c99_array_push(produced, kiln_c99_number((double)position));
                            }
                            if (status == KILN_C99_OK) {
                                status = kiln_c99_array_push(
                                    produced, kiln_c99_number(kiln_c99_typed_get(typed, position)));
                            }
                        }
                    } else {
                        produced = kiln_val_to_boundary(array->elements[position]);
                    }
                }
                status = status == KILN_C99_OK
                             ? kiln_c99_put(host, step, "value", 5, produced, depth, thrown)
                             : status;
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_put(host, step, "done", 4, kiln_c99_boolean(finished), depth,
                                  thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = step;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_HAS_OWN_PROPERTY: {
            /* `Object.hasOwn(x, k)` is the same question with the receiver moved into the first
             * argument, and it is answered by the same helper so the two cannot drift apart
             * again -- which they had, in four separate ways. */
            int found = 0;
            /* An absent argument is the one spelling the two engines do not agree about: the
             * language asks after the property named "undefined" and the interpreter asks after
             * the one named "". Neither answer may be given here until that is settled, so this
             * keeps saying so. */
            if (argc == 0) {
                KILN_C99_REFUSE("hasOwnProperty with no argument names a property the two engines "
                                "disagree about");
            }
            status = kiln_c99_own_property_present(host, receiver, args[0], &found, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_boolean(found);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_THROW_TYPE_ERROR:
            /* `%ThrowTypeError%`, in full. It is both halves of an unmapped `arguments`
             * object's `callee`, and the message is the one every engine gives. */
            return kiln_c99_throw_error(
                thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                "'caller', 'callee', and 'arguments' properties may not be accessed on strict "
                "mode functions or the arguments objects for calls to them");
        case KILN_C99_METHOD_OBJECT_TO_STRING: {
            /* `Object.prototype.toString`, the oldest type test there is -- and it reports the
             * *kind* rather than converting: `[object Array]`, not `1,2,3`. A library that has to
             * tell an array from an arguments object still reaches for it. */
            kiln_text built = {NULL, 0, 0};
            kiln_value custom = kiln_c99_undefined();
            const char *tag;
            /* `Symbol.toStringTag` overrides the built-in answer, which is the whole reason the
             * symbol exists: it is how a class says what a library's type test should call it.
             * Read through the ordinary chain walk, so an accessor on a prototype runs -- the
             * usual spelling is a getter on the class rather than a field on each instance.
             *
             * Only for an object, because only an object can carry one. A primitive's tag comes
             * from its kind and asking it for a property would be a conversion the operation does
             * not make. */
            if (receiver.tag == KILN_VALUE_OBJECT) {
                int status = kiln_c99_get(host, receiver, "@@toStringTag", 13, &custom, depth,
                                          thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            if (custom.tag == KILN_VALUE_STRING) {
                if (kiln_c99_text_append_c(&built, "[object ") != KILN_C99_OK ||
                    kiln_c99_text_append(&built, custom.ptr, custom.len) != KILN_C99_OK ||
                    kiln_c99_text_append_c(&built, "]") != KILN_C99_OK) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                *result = kiln_c99_text_value(&built);
                return KILN_C99_OK;
            }
            switch (receiver.tag) {
            case KILN_VALUE_UNDEFINED: tag = "Undefined"; break;
            case KILN_VALUE_NULL: tag = "Null"; break;
            case KILN_VALUE_BOOLEAN: tag = "Boolean"; break;
            case KILN_VALUE_NUMBER: tag = "Number"; break;
            case KILN_VALUE_STRING: tag = "String"; break;
            case KILN_VALUE_ARRAY: tag = "Array"; break;
            case KILN_VALUE_DATE: tag = "Date"; break;
            case KILN_VALUE_REGEXP: tag = "RegExp"; break;
            case KILN_VALUE_PROMISE: tag = "Promise"; break;
            /* Reached through the `Symbol.toStringTag` a generator's prototype carries rather
             * than through a builtinTag, and lands in the same string. The interpreter answers
             * `[object Generator]`; answering `[object Object]` here was a parity divergence. */
            case KILN_VALUE_GENERATOR: tag = "Generator"; break;
            case KILN_VALUE_SYMBOL: tag = "Symbol"; break;
            case KILN_VALUE_BIGINT: tag = "BigInt"; break;
            case KILN_VALUE_ARRAY_BUFFER: tag = "ArrayBuffer"; break;
            case KILN_VALUE_DATA_VIEW: tag = "DataView"; break;
            case KILN_VALUE_TYPED_ARRAY:
                tag = kiln_c99_typed_name(kiln_c99_typed_array_of(receiver)->kind);
                break;
            /* One tag holds both collections, so which one it is comes from the collection
             * rather than from the value tag. */
            case KILN_VALUE_COLLECTION:
                {
                    kiln_collection *collection = kiln_c99_collection_of(receiver);
                    tag = collection->is_weak
                              ? (collection->is_map ? "WeakMap" : "WeakSet")
                              : (collection->is_map ? "Map" : "Set");
                }
                break;
            /* Each of the eight names itself: `[object Temporal.PlainDate]`, which is the
             * `Symbol.toStringTag` the language gives them and the string every type test built
             * on this operation reads. `[object Object]` was the answer before. */
            case KILN_VALUE_TEMPORAL: tag = kiln_c99_temporal_tag(receiver); break;
            case KILN_VALUE_BUILTIN:
                /* `Math` and `JSON` are the two namespaces the language gives a tag of their own,
                 * and they are not functions -- calling them `Function` was the reason
                 * `Object.prototype.toString.call(Math)` disagreed with every other engine. A
                 * constructor really is a function and keeps that answer. */
                tag = receiver.bits == KILN_C99_BUILTIN_MATH   ? "Math"
                      : receiver.bits == KILN_C99_BUILTIN_JSON ? "JSON"
                      : kiln_c99_is_namespace(receiver.bits)   ? "Object"
                                                               : "Function";
                break;
            case KILN_VALUE_FUNCTION:
            case KILN_VALUE_METHOD:
            case KILN_VALUE_BOUND:
            case KILN_VALUE_COMMONJS_REQUIRE:
            case KILN_VALUE_SETTLER: tag = "Function"; break;
            /* An `arguments` object is ordinary in every respect but this one, and this one is
             * exactly what the type test above is asked to tell apart.
             *
             * So is an error, which carries no C-level brand: `@@error` is the reserved slot
             * `kiln_c99_new_error` attaches, and it is what stands in for the [[ErrorData]] the
             * specification brands on. Every error kind reports `Error`, the constructor not
             * being what is asked -- `toString.call(e) === "[object Error]"` is how a library
             * recognises an error it did not construct, across realms where `instanceof` cannot
             * answer. `[object Object]` was the answer before, in both engines; the interpreter
             * was fixed first and this is the other half. */
            case KILN_VALUE_OBJECT:
                tag = kiln_c99_object_of(receiver)->arguments_brand ? "Arguments"
                      : kiln_c99_object_of(receiver)->error_brand   ? "Error"
                                                                    : "Object";
                break;
            default: tag = "Object"; break;
            }
            if (kiln_c99_text_append_c(&built, "[object ") != KILN_C99_OK ||
                kiln_c99_text_append_c(&built, tag) != KILN_C99_OK ||
                kiln_c99_text_append_c(&built, "]") != KILN_C99_OK) {
                free(built.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            *result = kiln_c99_text_value(&built);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING: {
            /* `Invoke(ToObject(this), "toString")`, not the tag itself: a class that overrides
             * `toString` for display is the whole reason the method exists. It was not in this
             * table at all, so `({}).toLocaleString()` was a TypeError about `undefined` -- a
             * missing name reported as a missing function. */
            kiln_value method;
            if (receiver.tag == KILN_VALUE_UNDEFINED || receiver.tag == KILN_VALUE_NULL) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot convert undefined or null to object");
            }
            status = kiln_c99_get(host, receiver, "toString", 8, &method, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            return kiln_c99_call(host, result, method, receiver, NULL, 0, depth, thrown);
        }
        case KILN_C99_METHOD_OBJECT_VALUE_OF:
            /* The receiver unchanged, which is the identity that makes ToPrimitive fall through
             * to `toString` for a plain object.
             *
             * `ToObject(this)` says a primitive receiver answers with its *box*, and there is no
             * wrapper object in this backend to hand back. It cannot be refused here either: a
             * primitive's own `valueOf` -- `"abc".valueOf()` -- is routed to this code rather
             * than to the string one, so refusing would refuse that too. The only way to reach
             * `Object.prototype.valueOf` with a primitive is through `.call`, which this backend
             * refuses on a method receiver already, so the difference is not reachable here. */
            if (receiver.tag == KILN_VALUE_UNDEFINED || receiver.tag == KILN_VALUE_NULL) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot convert undefined or null to object");
            }
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_PRIMITIVE_TO_STRING:
            *result = kiln_c99_text(receiver);
            return KILN_C99_OK;
        case KILN_C99_METHOD_DATE_PARSE: {
            /* Parsing accepts the ISO forms and rejects the rest, rather than guessing: a date
             * a program did not write is worse than no date at all. */
            kiln_value source = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_undefined();
            *result = kiln_c99_number(source.tag == KILN_VALUE_STRING
                                          ? kiln_c99_parse_date(source.ptr, source.len)
                                          : 0.0 / 0.0);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_DATE_UTC:
        case KILN_C99_METHOD_GET_TIME:
        case KILN_C99_METHOD_GET_FULL_YEAR:
        case KILN_C99_METHOD_GET_MONTH:
        case KILN_C99_METHOD_GET_DATE:
        case KILN_C99_METHOD_GET_DAY:
        case KILN_C99_METHOD_GET_HOURS:
        case KILN_C99_METHOD_GET_MINUTES:
        case KILN_C99_METHOD_GET_SECONDS:
        case KILN_C99_METHOD_GET_MILLISECONDS:
        case KILN_C99_METHOD_GET_TIMEZONE_OFFSET:
        case KILN_C99_METHOD_TO_ISO_STRING:
        case KILN_C99_METHOD_TO_JSON:
        case KILN_C99_METHOD_DATE_TO_STRING:
        case KILN_C99_METHOD_SET_TIME:
        case KILN_C99_METHOD_SET_FULL_YEAR:
        case KILN_C99_METHOD_SET_MONTH:
        case KILN_C99_METHOD_SET_DATE:
        case KILN_C99_METHOD_SET_HOURS:
        case KILN_C99_METHOD_SET_MINUTES:
        case KILN_C99_METHOD_SET_SECONDS:
        case KILN_C99_METHOD_SET_MILLISECONDS: {
            kiln_date_parts parts;
            double time;
            if (code == KILN_C99_METHOD_DATE_UTC) {
                double fields[7];
                for (index = 0; index < 7; ++index) {
                    /* Day defaults to 1 and everything else to 0, which is what makes
                     * `Date.UTC(2026, 7)` the first of August. */
                    fields[index] = index < argc  ? kiln_c99_as_number(args[index])
                                    : index == 2 ? 1.0
                                                 : 0.0;
                }
                *result = kiln_c99_number(kiln_c99_instant_from_parts(fields));
                return KILN_C99_OK;
            }
            /* `Date.prototype.getTime.call({})` has no instant to read. Both engines and node
             * spell this fault the same way, so it is spelled that way here too. */
            if (receiver.tag != KILN_VALUE_DATE) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "this is not a Date object.");
            }
            time = kiln_c99_date_time(receiver);
            if (code == KILN_C99_METHOD_GET_TIME) {
                *result = kiln_c99_number(time);
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_GET_TIMEZONE_OFFSET) {
                /* Zero, honestly reported rather than left to be discovered: the realm is UTC. */
                *result = kiln_c99_number(0.0);
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_TO_ISO_STRING || code == KILN_C99_METHOD_TO_JSON) {
                kiln_value iso;
                status = kiln_c99_iso_string(&iso, time);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (iso.tag == KILN_VALUE_STRING) {
                    *result = iso;
                    return KILN_C99_OK;
                }
                /* An instant with no ISO spelling makes `toISOString` throw where `toJSON`
                 * answers null, so the enclosing document still serialises. */
                if (code == KILN_C99_METHOD_TO_JSON) {
                    *result = kiln_c99_null();
                    return KILN_C99_OK;
                }
                /* A `RangeError` the program can catch, which is what the sentence above already
                 * says happens -- it was ending the process instead. */
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "Invalid time value");
            }
            if (code == KILN_C99_METHOD_DATE_TO_STRING) {
                return kiln_c99_date_to_string(result, time);
            }
            if ((groups & KILN_C99_GROUP_DATE_SETTER)) {
                /* A setter writes through the instant the receiver already holds, which is why
                 * a Date is a heap slot rather than a value: `d.setUTCDate(31)` has to be
                 * visible through every other name for the same date.
                 *
                 * Each one takes over the fields from its own downwards -- `setUTCHours(h, m)`
                 * writes minutes too -- and leaves the rest alone. Out-of-range parts roll over
                 * rather than clamping, which falls out of rebuilding the instant from the
                 * fields instead of validating them: month 12 is January of the next year. */
                double fields[7];
                double written;
                size_t supplied;
                if (code == KILN_C99_METHOD_SET_TIME) {
                    written = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0 / 0.0;
                    *(double *)(uintptr_t)receiver.bits = written;
                    *result = kiln_c99_number(written);
                    return KILN_C99_OK;
                }
                parts = kiln_c99_date_parts(time);
                if (!parts.valid) {
                    /* An invalid date stays invalid: there are no fields to write into. */
                    *result = kiln_c99_number(0.0 / 0.0);
                    return KILN_C99_OK;
                }
                fields[0] = (double)parts.year;
                fields[1] = (double)(parts.month - 1);
                fields[2] = (double)parts.day;
                fields[3] = (double)parts.hour;
                fields[4] = (double)parts.minute;
                fields[5] = (double)parts.second;
                fields[6] = (double)parts.millisecond;
                /* `setUTCFullYear` starts at field 0, `setUTCMonth` at 1, and so on down. */
                supplied = (size_t)(code - KILN_C99_METHOD_SET_FULL_YEAR);
                for (index = 0; index < argc && supplied + index < 7; ++index) {
                    fields[supplied + index] = kiln_c99_as_number(args[index]);
                }
                written = kiln_c99_instant_from_parts(fields);
                *(double *)(uintptr_t)receiver.bits = written;
                *result = kiln_c99_number(written);
                return KILN_C99_OK;
            }
            parts = kiln_c99_date_parts(time);
            if (!parts.valid) {
                *result = kiln_c99_number(0.0 / 0.0);
                return KILN_C99_OK;
            }
            switch (code) {
            case KILN_C99_METHOD_GET_FULL_YEAR:
                *result = kiln_c99_number((double)parts.year);
                break;
            /* Zero-based, alone among the readers, which is the language's oldest wart. */
            case KILN_C99_METHOD_GET_MONTH:
                *result = kiln_c99_number((double)(parts.month - 1));
                break;
            case KILN_C99_METHOD_GET_DATE: *result = kiln_c99_number((double)parts.day); break;
            case KILN_C99_METHOD_GET_DAY: *result = kiln_c99_number((double)parts.weekday); break;
            case KILN_C99_METHOD_GET_HOURS: *result = kiln_c99_number((double)parts.hour); break;
            case KILN_C99_METHOD_GET_MINUTES:
                *result = kiln_c99_number((double)parts.minute);
                break;
            case KILN_C99_METHOD_GET_SECONDS:
                *result = kiln_c99_number((double)parts.second);
                break;
            default: *result = kiln_c99_number((double)parts.millisecond); break;
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ITERATOR_SELF:
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_ARRAY_VALUES: {
            /* Over a snapshot, so a `for...of` that appends to the array it is walking
             * terminates -- which is what the length being read once means. */
            kiln_value snapshot;
            kiln_array *array;
            if (receiver.tag == KILN_VALUE_TYPED_ARRAY) {
                return kiln_c99_new_typed_iterator(result, receiver, 0);
            }
            /* An ordinary object with a `length` -- an `arguments` object above all, whose own
             * `@@iterator` is this very method -- iterates as the array-like it is. Refusing
             * here is what made `[...arguments]` stop compiling the moment `arguments` stopped
             * being an array, and the interpreter has walked an array-like receiver all along. */
            if (receiver.tag == KILN_VALUE_OBJECT) {
                kiln_value length;
                double count;
                status = kiln_c99_get(host, receiver, "length", 6, &length, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                count = kiln_c99_as_number(length);
                if (!(count > 0.0)) {
                    count = 0.0;
                }
                if (count > 4294967295.0) {
                    count = 4294967295.0;
                }
                status = kiln_c99_new_array(&snapshot);
                for (index = 0; index < (size_t)count && status == KILN_C99_OK; ++index) {
                    kiln_value element;
                    kiln_value key = kiln_c99_property_key(kiln_c99_number((double)index));
                    if (key.tag != KILN_VALUE_STRING) {
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    status = kiln_c99_get(host, receiver, key.ptr, key.len, &element, depth,
                                          thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                    status = kiln_c99_array_push(snapshot, element);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                return kiln_c99_new_iterator(result, snapshot);
            }
            /* A string's `@@iterator` is this same method, and it walks *characters*: an astral
             * pair is one step, which is the difference between iterating a string and indexing
             * it. Without this a string receiver fell to the refusal below, so
             * `for (const c of text)` refused where `[...text]` worked -- the two differ only in
             * that spread reaches `kiln_c99_iterate` directly while `for...of` is compiled as
             * `text[@@iterator]()` and so arrives here. A bundled `marked` refuses on exactly
             * that, tokenising a list item. The walk is `kiln_c99_iterate`'s rather than a second
             * one written out here, so the two spellings of one loop cannot come to disagree
             * about where a surrogate pair ends. */
            if (receiver.tag == KILN_VALUE_STRING) {
                status = kiln_c99_new_array(&snapshot);
                if (status != KILN_C99_OK) {
                    return status;
                }
                status = kiln_c99_iterate(host, snapshot, receiver, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                return kiln_c99_new_iterator(result, snapshot);
            }
            if (receiver.tag != KILN_VALUE_ARRAY) {
                KILN_C99_REFUSE("an array iteration method on a receiver that is not an array");
            }
            array = kiln_c99_array_of(receiver);
            status = kiln_c99_new_array(&snapshot);
            for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
                /* A hole reads as undefined here: `for...of` visits every position, unlike
                 * `forEach`, which skips one. */
                status = kiln_c99_array_push(snapshot,
                                             array->elements[index] == KILN_VAL_HOLE
                                                 ? kiln_c99_undefined()
                                                 : kiln_val_to_boundary(array->elements[index]));
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            return kiln_c99_new_iterator(result, snapshot);
        }
        case KILN_C99_METHOD_GENERATOR_SELF:
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                KILN_C99_REFUSE("a generator method on a receiver that is not a generator");
            }
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_GENERATOR_THROW: {
            kiln_generator *frame;
            kiln_closure *closure;
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                KILN_C99_REFUSE("a generator method on a receiver that is not a generator");
            }
            frame = kiln_c99_generator_of(receiver);
            if (frame->running) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Generator is already running");
            }
            /* A suspended body with a handler over the suspension gets the value thrown *at*
             * the `yield`, so its `catch` or `finally` runs; it may yield again, finish, or
             * let the throw out. A body that has not started, is done, or has nothing to
             * catch with, is finished and the throw goes straight to the caller. */
            if (frame->started && !frame->done && frame->handler_top != 0 && !frame->async_generator) {
                kiln_val vproduced = KILN_VAL_UNDEFINED;
                frame->raising = 1;
                frame->sent = argc > 0 ? args[0] : kiln_c99_undefined();
                closure = kiln_c99_closure_of(frame->function);
                frame->running = 1;
                status = kiln_c99_invoke_closure_body(
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
                *thrown = argc > 0 ? args[0] : kiln_c99_undefined();
            }
            return KILN_C99_THROWN;
        }
        case KILN_C99_METHOD_GENERATOR_NEXT:
        case KILN_C99_METHOD_GENERATOR_RETURN: {
            kiln_generator *frame;
            kiln_closure *closure;
            kiln_value produced = kiln_c99_undefined();
            int finished = 1;
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                KILN_C99_REFUSE("a generator method on a receiver that is not a generator");
            }
            frame = kiln_c99_generator_of(receiver);
            /* One frame, and it is on the C stack: resuming it from inside itself would overwrite
             * the locals the running body is standing on. The language says so by name. */
            if (frame->running) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Generator is already running");
            }
            if (frame->async_generator) {
                kiln_value step_promise;
                status = kiln_c99_new_promise(&step_promise);
                if (status != KILN_C99_OK) {
                    return status;
                }
                *result = step_promise;
                kiln_value sent_val = argc > 0 ? args[0] : kiln_c99_undefined();
                if (frame->done) {
                    return kiln_c99_async_generator_settle(
                        host, step_promise,
                        code == KILN_C99_METHOD_GENERATOR_RETURN ? sent_val : kiln_c99_undefined(),
                        1, depth, thrown);
                }
                if (code == KILN_C99_METHOD_GENERATOR_RETURN) {
                    if (frame->handler_top != 0 && !frame->done) {
                        frame->raising = 1;
                        frame->sent = kiln_c99_undefined();
                        closure = kiln_c99_closure_of(frame->function);
                        frame->running = 1;
                        kiln_val vproduced = KILN_VAL_UNDEFINED;
                        (void)kiln_c99_invoke_closure_body(
                            host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0, depth, thrown, frame, 0);
                        produced = kiln_val_to_boundary(vproduced);
                        frame->running = 0;
                    }
                    frame->done = 1;
                    return kiln_c99_async_generator_settle(host, step_promise, sent_val, 1, depth, thrown);
                }
                if (frame->queue_executing) {
                    if (frame->queue_count >= frame->queue_capacity) {
                        size_t new_cap = frame->queue_capacity == 0 ? 4 : frame->queue_capacity * 2;
                        frame->queue_promises = (kiln_value *)realloc(frame->queue_promises, new_cap * sizeof(kiln_value));
                        frame->queue_sent = (kiln_value *)realloc(frame->queue_sent, new_cap * sizeof(kiln_value));
                        frame->queue_capacity = new_cap;
                    }
                    frame->queue_promises[frame->queue_count] = step_promise;
                    frame->queue_sent[frame->queue_count] = sent_val;
                    frame->queue_count++;
                    return KILN_C99_OK;
                }
                frame->queue_executing = 1;
                frame->promise = step_promise;
                return kiln_c99_async_generator_step(
                    host, receiver, sent_val, 0, depth, thrown);
            }
            if (code == KILN_C99_METHOD_GENERATOR_RETURN) {
                if (frame->handler_top != 0 && !frame->done) {
                    frame->raising = 1;
                    frame->sent = kiln_c99_undefined();
                    closure = kiln_c99_closure_of(frame->function);
                    frame->running = 1;
                    kiln_val vproduced = KILN_VAL_UNDEFINED;
                    (void)kiln_c99_invoke_closure_body(
                        host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0, depth, thrown, frame, 0);
                    produced = kiln_val_to_boundary(vproduced);
                    frame->running = 0;
                }
                frame->done = 1;
                produced = argc > 0 ? args[0] : kiln_c99_undefined();
            } else if (!frame->done) {
                frame->sent = argc > 0 ? args[0] : kiln_c99_undefined();
                frame->raising = 0;
                closure = kiln_c99_closure_of(frame->function);
                frame->running = 1;
                kiln_val vproduced = KILN_VAL_UNDEFINED;
                status = kiln_c99_invoke_closure_body(
                    host, &vproduced, closure, kiln_val_from_boundary(frame->receiver), NULL, 0, depth, thrown, frame, 0);
                produced = kiln_val_to_boundary(vproduced);
                frame->running = 0;
                if (status == KILN_C99_YIELDED) {
                    frame->started = 1;
                    finished = 0;
                } else if (status != KILN_C99_OK) {
                    frame->done = 1;
                    return status;
                } else {
                    frame->done = 1;
                }
            }
            return kiln_c99_generator_step(host, produced, finished || frame->done, result, depth, thrown);
        }
        case KILN_C99_METHOD_JSON_STRINGIFY: {
            kiln_text text = {NULL, 0, 0};
            kiln_text indent = {NULL, 0, 0};
            int wrote;
            /* A replacer is a function or an array of keys, and honouring half of it would give
             * a document missing exactly what the program asked to control. */
            if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED &&
                args[1].tag != KILN_VALUE_NULL) {
                KILN_C99_REFUSE("JSON.stringify with a replacer");
            }
            if (argc > 2) {
                if (args[2].tag == KILN_VALUE_NUMBER) {
                    /* A number is that many spaces, capped at ten, which is what the
                     * specification says and what stops a silly argument from allocating. */
                    double count = kiln_c99_as_number(args[2]);
                    size_t spaces = !(count > 0.0) ? 0 : count > 10.0 ? 10 : (size_t)count;
                    while (spaces-- > 0) {
                        if (kiln_c99_text_append_c(&indent, " ") != KILN_C99_OK) {
                            free(indent.bytes);
                            return KILN_C99_OUT_OF_MEMORY;
                        }
                    }
                } else if (args[2].tag == KILN_VALUE_STRING) {
                    size_t len = args[2].len > 10 ? 10 : args[2].len;
                    if (kiln_c99_text_append(&indent, args[2].ptr, len) != KILN_C99_OK) {
                        free(indent.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
            }
            status = kiln_c99_json_value(host, &text, argc > 0 ? args[0] : kiln_c99_undefined(),
                                         indent.bytes, indent.len, "", 0, &wrote, depth, NULL,
                                         thrown);
            free(indent.bytes);
            if (status != KILN_C99_OK) {
                free(text.bytes);
                return status;
            }
            /* `JSON.stringify(undefined)` is `undefined`, not the string. */
            if (wrote) {
                *result = kiln_c99_text_value(&text);
            } else {
                free(text.bytes);
                *result = kiln_c99_undefined();
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_JSON_PARSE: {
            kiln_value source = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_undefined();
            kiln_val parsed;
            if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
                /* A reviver rewrites the tree bottom-up; ignoring it would hand back a document
                 * the program never asked for. */
                KILN_C99_REFUSE("JSON.parse with a reviver");
            }
            if (source.tag != KILN_VALUE_STRING) {
                KILN_C99_REFUSE("JSON.parse of a value with no string form");
            }
            status = kiln_c99_json_document(source.ptr, source.len, &parsed);
            /* Malformed input is a `SyntaxError` the program catches, not a fault that stops it:
             * `try { JSON.parse(input) } catch {}` around text that came from outside is the most
             * ordinary code there is, and it was unwritable while this ended the process. Only the
             * reader's own refusal becomes the throw -- running out of memory is still that. */
            if (status == KILN_C99_CONTROL_ERROR) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                            "Unexpected token in JSON");
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_val_to_boundary(parsed);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            if (subject.tag == KILN_VALUE_PROXY) {
                kiln_value trap;
                kiln_proxy *data = kiln_c99_proxy_of(subject);
                int trap_st = kiln_c99_proxy_trap(host, subject, "getPrototypeOf", &trap, depth, thrown);
                if (trap_st != KILN_C99_OK) return trap_st;
                if (trap.tag != KILN_VALUE_UNDEFINED) {
                    kiln_value trap_args[1] = { data->target };
                    return kiln_c99_call(host, result, trap, data->handler, trap_args, 1, depth, thrown);
                }
                return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                                              kiln_c99_undefined(), &data->target, 1, depth, thrown);
            }
            if (subject.tag == KILN_VALUE_OBJECT) {
                kiln_value linked = kiln_c99_object_of(subject)->prototype;
                /* Undefined and null are different answers: one means nothing was recorded, and
                 * an object with nothing recorded still inherits `Object.prototype`. The other
                 * means nothing was wanted, which is what `Object.create(null)` asks for. */
                *result = linked.tag == KILN_VALUE_UNDEFINED
                              ? kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE)
                              : linked;
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_ARRAY || subject.tag == KILN_VALUE_COLLECTION) {
                /* A subclass instance, or one a `setPrototypeOf` relinked, answers with the
                 * link its store carries. */
                kiln_value *slot = kiln_c99_exotic_store(subject);
                kiln_value linked = slot->tag == KILN_VALUE_OBJECT
                                        ? kiln_c99_object_of(*slot)->prototype
                                        : kiln_c99_undefined();
                if (linked.tag != KILN_VALUE_UNDEFINED) {
                    *result = linked;
                    return KILN_C99_OK;
                }
                if (subject.tag == KILN_VALUE_ARRAY) {
                    *result = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_PROTOTYPE);
                } else {
                    kiln_collection *collection = kiln_c99_collection_of(subject);
                    *result = kiln_c99_builtin(
                        collection->is_weak
                            ? (collection->is_map ? KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE
                                                  : KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE)
                            : (collection->is_map ? KILN_C99_BUILTIN_MAP_PROTOTYPE
                                                  : KILN_C99_BUILTIN_SET_PROTOTYPE));
                }
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_STRING) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_STRING_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_TYPED_ARRAY) {
                *result = kiln_c99_builtin(
                    kiln_c99_typed_prototype(kiln_c99_typed_array_of(subject)->kind));
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_DATA_VIEW) {
                /* One table for every view, because a DataView has no kind to pick one by. */
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_FUNCTION || subject.tag == KILN_VALUE_METHOD ||
                subject.tag == KILN_VALUE_BOUND) {
                /* A function keeps a relinked prototype on its store, which is where
                 * `Object.setPrototypeOf(f, o)` puts it and where the read walk looks for
                 * inherited statics -- so this has to report the same link or the two disagree
                 * about a chain they are both walking. */
                kiln_value linked =
                    subject.tag == KILN_VALUE_FUNCTION &&
                            kiln_c99_closure_of(subject)->properties.tag == KILN_VALUE_OBJECT
                        ? kiln_c99_object_of(kiln_c99_closure_of(subject)->properties)->prototype
                        : kiln_c99_undefined();
                *result = linked.tag == KILN_VALUE_UNDEFINED
                              ? kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE)
                              : linked;
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_NUMBER) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_NUMBER_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_REGEXP) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_REGEXP_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_DATE) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_DATE_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_BUILTIN) {
                kiln_typed_kind typed_kind;
                kiln_value store;
                int stored = kiln_c99_callable_store(subject, &store, 0);
                if (stored != KILN_C99_OK) return stored;
                if (store.tag == KILN_VALUE_OBJECT &&
                    kiln_c99_object_of(store)->prototype.tag != KILN_VALUE_UNDEFINED) {
                    *result = kiln_c99_object_of(store)->prototype;
                    return KILN_C99_OK;
                }
                if (kiln_c99_typed_prototype_kind(subject.bits, &typed_kind)) {
                    *result = kiln_c99_builtin(KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE);
                    return KILN_C99_OK;
                }
                if (subject.bits == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE) {
                    *result = kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
                    return KILN_C99_OK;
                }
                if (subject.bits == KILN_C99_BUILTIN_PROMISE_PROTOTYPE) {
                    *result = kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
                    return KILN_C99_OK;
                }
                /* A prototype table's own prototype is `Object.prototype`, which is where the
                 * chain from any of them ends. */
                *result = kiln_c99_is_namespace(subject.bits) &&
                                  subject.bits != KILN_C99_BUILTIN_OBJECT_PROTOTYPE
                              ? kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE)
                          : subject.bits == KILN_C99_BUILTIN_OBJECT_PROTOTYPE
                              ? kiln_c99_null()
                              : kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE);
                return KILN_C99_OK;
            }
            KILN_C99_REFUSE("Object.getPrototypeOf of a value whose chain this backend does not "
                            "model");
        }
        case KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            if (subject.tag != KILN_VALUE_OBJECT && subject.tag != KILN_VALUE_FUNCTION &&
                kiln_c99_exotic_store(subject) == NULL &&
                !(subject.tag == KILN_VALUE_BUILTIN && kiln_c99_is_namespace(subject.bits))) {
                KILN_C99_REFUSE("Object.setPrototypeOf on a value with no prototype slot");
            }
            {
                /* Where the literal's `__proto__` ignores a primitive, this one throws: a
                 * program that asked for a prototype in so many words is told the request could
                 * not be met rather than left to find out later. */
                kiln_value wanted = argc > 1 ? args[1] : kiln_c99_undefined();
                if (!kiln_c99_can_be_prototype(wanted)) {
                    return kiln_c99_throw_error(
                        thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                        "Object prototype may only be an Object or null");
                }
            }
            status = kiln_c99_set_prototype(subject, argc > 1 ? args[1] : kiln_c99_undefined());
            /* The object, so a `setPrototypeOf` can be inlined into the expression that built
             * it. */
            *result = subject;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_SYMBOLS: {
            /* The other half of `getOwnPropertyNames`, off the very same walk -- `ownKeys` is
             * both, and each of the two exists in order to be one of them. Its absence refused a
             * bundled `lodash` at the point it asked an object for its symbol keys, a question
             * the walk beneath it could already answer. */
            kiln_value listed;
            kiln_value symbols;
            status = kiln_c99_all_own_keys(host, argc > 0 ? args[0] : kiln_c99_undefined(),
                                           &listed, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_new_array(&symbols);
            for (index = 0; index < kiln_c99_array_of(listed)->count && status == KILN_C99_OK;
                 ++index) {
                kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(listed)->elements[index]);
                if (key.tag != KILN_VALUE_SYMBOL) {
                    continue;
                }
                status = kiln_c99_array_push(symbols, key);
            }
            *result = symbols;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES: {
            /* Names, so the symbol keys `Reflect.ownKeys` reports are dropped again here. The
             * two operations exist precisely to be different: `getOwnPropertySymbols` is the
             * other half, and `ownKeys` is both. */
            kiln_value listed;
            kiln_value named;
            status = kiln_c99_all_own_keys(host, argc > 0 ? args[0] : kiln_c99_undefined(),
                                           &listed, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_new_array(&named);
            for (index = 0; index < kiln_c99_array_of(listed)->count && status == KILN_C99_OK;
                 ++index) {
                kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(listed)->elements[index]);
                if (key.tag == KILN_VALUE_SYMBOL) {
                    continue;
                }
                status = kiln_c99_array_push(named, key);
            }
            *result = named;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY: {
            kiln_value name = argc > 1 ? kiln_c99_property_key(args[1]) : kiln_c99_undefined();
            if (argc < 3 || name.tag != KILN_VALUE_STRING) {
                KILN_C99_REFUSE("Object.defineProperty invalid key or arity");
            }
            status = kiln_c99_define_property(host, args[0], name.ptr, name.len, args[2], depth,
                                              thrown);
            if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_detail == NULL)
                kiln_c99_diagnostic_detail = "Object.defineProperty rejected descriptor or target";
            /* The target, so a definition can be inlined into the expression that built it. */
            *result = args[0];
            return status;
        }
        case KILN_C99_METHOD_OBJECT_DEFINE_PROPERTIES: {
            if (argc < 2) {
                KILN_C99_REFUSE("Object.defineProperties needs a target and a map of descriptors");
            }
            status = kiln_c99_define_properties(host, args[0], args[1], depth, thrown);
            *result = args[0];
            return status;
        }
        case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR: {
            kiln_value name = argc > 1 ? kiln_c99_property_key(args[1]) : kiln_c99_undefined();
            int found;
            if (argc < 2 || name.tag != KILN_VALUE_STRING) {
                KILN_C99_REFUSE("Object.getOwnPropertyDescriptor invalid key or arity");
            }
            return kiln_c99_describe_property(host, args[0], name.ptr, name.len, result, &found,
                                             depth, thrown);
        }
        case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTORS: {
            /* One object holding every own property's descriptor, which is what a structural
             * clone reaches for -- accessors and all, where a spread would have called them. */
            kiln_value built;
            kiln_object *object;
            if (argc == 0 || args[0].tag != KILN_VALUE_OBJECT) {
                KILN_C99_REFUSE("Object.getOwnPropertyDescriptors of something that is not an "
                                "object");
            }
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            object = kiln_c99_object_of(args[0]);
            if (object->shape != NULL) {
                kiln_c99_ensure_dictionary(object);
            }
            for (index = 0; index < object->count; ++index) {
                kiln_value described;
                int found;
                kiln_value symbol;
                const char *key = object->properties[index].key;
                size_t key_len = object->properties[index].key_len;
                /* A symbol-keyed property is described too -- this is how a faithful copy is
                 * made, and one that quietly leaves them out is the bug this family had. It
                 * lands in the result under the same reserved key it came from, so nothing here
                 * needs to know it is a symbol beyond deciding to keep it. */
                if (key_len >= 2 && key[0] == '@' && key[1] == '@' &&
                    !kiln_c99_symbol_of_key(key, key_len, &symbol)) {
                    continue;
                }
                status = kiln_c99_describe_property(host, args[0], key, key_len, &described, &found,
                                                    depth, thrown);
                if (status == KILN_C99_OK && found) {
                    status = kiln_c99_put(NULL, built, key, key_len, described, 0, NULL);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                object = kiln_c99_object_of(args[0]);
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_PROXY_REVOCABLE: {
            /* A pair: the proxy, and the one function that can retire it. Handing the second out
             * separately is the whole point -- a caller can be given the proxy without being
             * given the power to withdraw it, and the other way round. */
            kiln_value proxy;
            kiln_value revoke;
            kiln_value pair;
            status = kiln_c99_new_proxy(&proxy, argc > 0 ? args[0] : kiln_c99_undefined(),
                                        argc > 1 ? args[1] : kiln_c99_undefined());
            if (status == KILN_C99_OK) {
                status = kiln_c99_bind(&revoke, kiln_c99_method(KILN_C99_METHOD_PROXY_REVOKE),
                                       proxy);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_new_object(&pair);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, pair, "proxy", 5, proxy, 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, pair, "revoke", 6, revoke, 0, NULL);
            }
            *result = pair;
            return status;
        }
        case KILN_C99_METHOD_REFLECT_OWN_KEYS:
        case KILN_C99_METHOD_REFLECT_GET:
        case KILN_C99_METHOD_REFLECT_SET:
        case KILN_C99_METHOD_REFLECT_HAS:
        case KILN_C99_METHOD_REFLECT_DELETE_PROPERTY: {
            /* Every one of these refuses a primitive target rather than coercing it, which is
             * the difference from the operator forms: `Reflect.get(1, "x")` is a TypeError where
             * `(1).x` reads off the string prototype. A library that reaches for `Reflect`
             * usually does so precisely to get the strict form. */
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value key = argc > 1 ? args[1] : kiln_c99_undefined();
            int answer;
            if (!kiln_c99_is_reference(subject) && subject.tag != KILN_VALUE_DATE &&
                subject.tag != KILN_VALUE_REGEXP && subject.tag != KILN_VALUE_PROMISE) {
                return kiln_c99_throw_not_a(thrown, subject, "object");
            }
            switch (code) {
            case KILN_C99_METHOD_REFLECT_OWN_KEYS:
                return kiln_c99_all_own_keys(host, subject, result, depth, thrown);
            case KILN_C99_METHOD_REFLECT_GET:
                /* The third argument is what an accessor sees as `this` and what a `get` trap is
                 * handed; without one it is the target itself. */
                return kiln_c99_get_with_receiver(host, subject, key,
                                                  argc > 2 ? args[2] : subject, result, depth,
                                                  thrown);
            case KILN_C99_METHOD_REFLECT_SET:
                status = kiln_c99_reflect_set(host, subject, key,
                                               argc > 2 ? args[2] : kiln_c99_undefined(), &answer,
                                               depth, thrown);
                *result = kiln_c99_boolean(answer);
                return status;
            case KILN_C99_METHOD_REFLECT_HAS:
                status = kiln_c99_has_computed(host, subject, key, &answer, depth, thrown);
                *result = kiln_c99_boolean(answer);
                return status;
            default:
                status = kiln_c99_delete_computed(host, subject, key, &answer, depth, thrown);
                *result = kiln_c99_boolean(answer);
                return status;
            }
        }
        case KILN_C99_METHOD_REFLECT_GET_PROTOTYPE_OF: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            if (!kiln_c99_is_reference(subject) && subject.tag != KILN_VALUE_DATE &&
                subject.tag != KILN_VALUE_REGEXP && subject.tag != KILN_VALUE_PROMISE) {
                return kiln_c99_throw_not_a(thrown, subject, "object");
            }
            return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                                          kiln_c99_undefined(), &subject, 1, depth, thrown);
        }
        case KILN_C99_METHOD_REFLECT_APPLY: {
            /* `Reflect.apply(f, this, args)` is `f.apply(this, args)` without going through the
             * function's own `apply`, which a proxy or an override could have replaced. */
            kiln_array *list;
            if (argc < 3 || args[2].tag != KILN_VALUE_ARRAY) {
                KILN_C99_REFUSE("Reflect.apply needs a callee, a receiver and an array of "
                                "arguments");
            }
            list = kiln_c99_array_of(args[2]);
            kiln_val call_result_val = KILN_VAL_UNDEFINED;
            status = kiln_c99_call_val(host, &call_result_val, kiln_val_from_boundary(args[0]),
                                       kiln_val_from_boundary(args[1]),
                                       list->count == 0 ? NULL : list->elements, list->count, depth,
                                       thrown);
            if (status == KILN_C99_OK) *result = kiln_val_to_boundary(call_result_val);
            return status;
        }
        case KILN_C99_METHOD_REFLECT_CONSTRUCT: {
            kiln_array *list;
            if (argc < 2 || args[1].tag != KILN_VALUE_ARRAY) {
                KILN_C99_REFUSE("Reflect.construct needs a callee and an array of arguments");
            }
            if (argc > 2) {
                /* A third argument names a `new.target` other than the callee, which changes
                 * both what the body reads and which prototype the instance gets. */
                KILN_C99_REFUSE("Reflect.construct with a new.target other than the callee");
            }
            list = kiln_c99_array_of(args[1]);
            kiln_val built_val = KILN_VAL_UNDEFINED;
            status = kiln_c99_construct_val(host, &built_val, kiln_val_from_boundary(args[0]),
                                            list->count == 0 ? NULL : list->elements, list->count,
                                            depth, thrown);
            if (status == KILN_C99_OK) *result = kiln_val_to_boundary(built_val);
            return status;
        }
        case KILN_C99_METHOD_ERROR_CAPTURE_STACK_TRACE:
            /* A release build strips unwind tables, so there are no frames to read: the
             * property this would write is one this engine cannot fill in. Answering undefined
             * and leaving the error's own `stack` header alone is what the interpreter does. */
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        case KILN_C99_METHOD_OBJECT_IS: {
            /* SameValue: `===` except that it separates the two zeroes and finds NaN. The
             * collections already compare with SameValueZero, which differs from this in
             * exactly the zero case, so the shared half is reused and only that case is added. */
            kiln_value left = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value right = argc > 1 ? args[1] : kiln_c99_undefined();
            if (left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_NUMBER &&
                kiln_c99_as_number(left) == 0.0 && kiln_c99_as_number(right) == 0.0) {
                *result = kiln_c99_boolean(kiln_c99_signbit(kiln_c99_as_number(left)) ==
                                           kiln_c99_signbit(kiln_c99_as_number(right)));
                return KILN_C99_OK;
            }
            *result = kiln_c99_boolean(kiln_c99_same_value_zero(left, right));
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_STRING_FROM_CHAR_CODE: {
            /* Each argument is one UTF-16 *code unit*, so a surrogate pair spelled across two
             * arguments becomes the one character it stands for and a lone surrogate becomes
             * U+FFFD. `kiln_c99_from_units` already draws both of those lines, and draws them
             * where `String::from_utf16_lossy` does, which is what the interpreter builds the
             * string with.
             *
             * The narrowing is the interpreter's `as u16` rather than the specification's
             * `ToUint16`, and the difference is visible: the specification *wraps*, so
             * `fromCharCode(65601)` is "A" in node, where a saturating cast answers U+FFFF.
             * Matching the specification here would be a wrong answer against the oracle this
             * corpus measures against -- the two engines disagreeing is the failure being
             * fixed, so the deviation stays in one place and is shared rather than split. */
            uint16_t *units = NULL;
            size_t at;
            if (argc > 0) {
                units = (uint16_t *)malloc(argc * sizeof *units);
                if (units == NULL) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            for (at = 0; at < argc; ++at) {
                double number = kiln_c99_as_number(args[at]);
                units[at] = isnan(number)            ? (uint16_t)0
                            : number <= 0.0          ? (uint16_t)0
                            : number >= 65535.0      ? (uint16_t)65535
                                                     : (uint16_t)number;
            }
            *result = kiln_c99_from_units(units, argc);
            free(units);
            return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
        }
        case KILN_C99_METHOD_STRING_FROM_CODE_POINT: {
            uint16_t *units = NULL;
            size_t out_count = 0;
            size_t at;
            if (argc > 0) {
                units = (uint16_t *)malloc(argc * 2 * sizeof *units);
                if (units == NULL) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            for (at = 0; at < argc; ++at) {
                double number = kiln_c99_as_number(args[at]);
                if (isnan(number) || floor(number) != number || number < 0.0 || number > 1114111.0) {
                    free(units);
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                "Invalid code point");
                }
                uint32_t cp = (uint32_t)number;
                if (cp <= 0xFFFF) {
                    units[out_count++] = (uint16_t)cp;
                } else {
                    cp -= 0x10000;
                    units[out_count++] = (uint16_t)(0xD800 + (cp >> 10));
                    units[out_count++] = (uint16_t)(0xDC00 + (cp & 0x3FF));
                }
            }
            *result = kiln_c99_from_units(units, out_count);
            free(units);
            return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
        }
        case KILN_C99_METHOD_STRING_RAW: {
            /* `String.raw` reads the *raw* half of the template object -- the text as written,
             * before escapes were interpreted -- and rejoins it around the substitutions. The
             * substitutions arrive as the remaining arguments, one between each pair of
             * segments and none after the last, which is what makes a trailing `${}` add
             * nothing. */
            kiln_value strings = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value raw;
            kiln_array *segments;
            kiln_text built = {NULL, 0, 0};
            size_t at;
            status = kiln_c99_get(host, strings, "raw", 3, &raw, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (raw.tag != KILN_VALUE_ARRAY) {
                /* A hand-built array-like would walk here too; only the array a template site
                 * produces is understood so far. */
                KILN_C99_REFUSE("a tagged template whose raw strings are not an array");
            }
            segments = kiln_c99_array_of(raw);
            for (at = 0; at < segments->count; ++at) {
                kiln_value piece = segments->elements[at] == KILN_VAL_HOLE
                                       ? kiln_c99_undefined()
                                       : kiln_val_to_boundary(segments->elements[at]);
                kiln_value shown = kiln_c99_text(piece);
                if (shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_text_append(&built, shown.ptr, shown.len) != KILN_C99_OK) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                if (at + 1 < segments->count && at + 1 < argc) {
                    kiln_value between = kiln_c99_text(args[at + 1]);
                    if (between.tag != KILN_VALUE_STRING ||
                        kiln_c99_text_append(&built, between.ptr, between.len) != KILN_C99_OK) {
                        free(built.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
            }
            *result = kiln_c99_text_value(&built);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_FROM_ENTRIES: {
            kiln_value built;
            kiln_array *pairs;
            if (argc == 0 || args[0].tag != KILN_VALUE_ARRAY) {
                KILN_C99_REFUSE("Object.fromEntries of something that is not an array");
            }
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            pairs = kiln_c99_array_of(args[0]);
            for (index = 0; index < pairs->count; ++index) {
                kiln_array *pair;
                kiln_value entry = kiln_val_to_boundary(pairs->elements[index]);
                if (entry.tag != KILN_VALUE_ARRAY) {
                    KILN_C99_REFUSE("Object.fromEntries given an entry that is not an array");
                }
                pair = kiln_c99_array_of(entry);
                status = kiln_c99_set_computed(
                    host, built, pair->count > 0 && pair->elements[0] != KILN_VAL_HOLE ? kiln_val_to_boundary(pair->elements[0]) : kiln_c99_undefined(),
                    pair->count > 1 && pair->elements[1] != KILN_VAL_HOLE ? kiln_val_to_boundary(pair->elements[1]) : kiln_c99_undefined(), depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ARRAY_FROM: {
            /* Anything iterable, then the optional mapper -- which is one walk rather than a
             * `map` afterwards, because `Array.from({length: 3}, f)` has nothing to map. */
            kiln_value built;
            kiln_value mapper = argc > 1 ? args[1] : kiln_c99_undefined();
            kiln_array *collected;
            kiln_value source = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value iterator_method;
            if (mapper.tag != KILN_VALUE_UNDEFINED && !kiln_c99_is_callable(mapper)) {
                return kiln_c99_throw_not_a(thrown, mapper, "function");
            }
            if (source.tag == KILN_VALUE_UNDEFINED || source.tag == KILN_VALUE_NULL) {
                return kiln_c99_throw_not_a(thrown, source, "iterable or array-like object");
            }
            status = kiln_c99_new_array(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            iterator_method = kiln_c99_undefined();
            /* A generator is iterable and is not array-like: it has no `length`, so falling to
             * the array-like path below read `undefined`, coerced it to zero, and answered `[]`
             * for a generator that yields. That is the silent kind of wrong -- `Array.from(g())`
             * is how a program drains one, and an empty array is a plausible answer a caller
             * carries forward. It used to refuse here, so the answer got worse, not better. */
            if (source.tag == KILN_VALUE_ARRAY || source.tag == KILN_VALUE_STRING ||
                source.tag == KILN_VALUE_TYPED_ARRAY || source.tag == KILN_VALUE_COLLECTION ||
                source.tag == KILN_VALUE_GENERATOR) {
                status = kiln_c99_get(host, source, "@@iterator", 10, &iterator_method, depth,
                                      thrown);
                if (status != KILN_C99_OK) return status;
            } else if (source.tag == KILN_VALUE_OBJECT) {
                /* A plain object is array-like unless it explicitly supplies the iterator
                 * protocol. Unknown ordinary names on objects read as undefined here. */
                status = kiln_c99_get(host, source, "@@iterator", 10, &iterator_method, depth,
                                      thrown);
                if (status == KILN_C99_CONTROL_ERROR) {
                    /* Answered rather than refused, so the refusal's detail must go with it --
                     * see the `@@` conversion in `kiln_c99_get` for what a left-behind one does. */
                    kiln_c99_diagnostic_detail = NULL;
                    iterator_method = kiln_c99_undefined();
                    status = KILN_C99_OK;
                }
                if (status != KILN_C99_OK) return status;
            }
            if (iterator_method.tag == KILN_VALUE_UNDEFINED ||
                iterator_method.tag == KILN_VALUE_NULL) {
                /* `LengthOfArrayLike` is `ToLength(Get(items, "length"))`, so a length that is
                 * not already a number is coerced rather than read as zero, and one no array can
                 * have is the `ArrayCreate` step's `RangeError`. Clamping `Infinity` to
                 * `(size_t)-1` and then *running* was the other answer, and it reached 6 GB
                 * before anything stopped it. The interpreter refuses at the same bound. */
                kiln_value length_value;
                double wanted;
                size_t length = 0;
                status = kiln_c99_get(host, source, "length", 6, &length_value, depth, thrown);
                if (status != KILN_C99_OK) return status;
                status = kiln_c99_to_number_checked(host, length_value, &wanted, depth, thrown);
                if (status != KILN_C99_OK) return status;
                if (wanted > 0.0) {
                    wanted = floor(wanted);
                    if (wanted > 4294967295.0) {
                        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                    "Invalid array length");
                    }
                    length = (size_t)wanted;
                }
                for (index = 0; index < length; ++index) {
                    char key[32];
                    size_t key_len = kiln_c99_double_to_js((double)index, key);
                    kiln_value value;
                    kiln_value produced;
                    status = kiln_c99_get(host, source, key, key_len, &value, depth, thrown);
                    if (status != KILN_C99_OK) return status;
                    if (mapper.tag != KILN_VALUE_UNDEFINED) {
                        kiln_value visit[2] = {value, kiln_c99_number((double)index)};
                        kiln_value this_arg = argc > 2 ? args[2] : kiln_c99_undefined();
                        status = kiln_c99_call(host, &produced, mapper, this_arg, visit, 2, depth,
                                               thrown);
                        if (status != KILN_C99_OK) return status;
                        value = produced;
                    }
                    status = kiln_c99_array_push(built, value);
                    if (status != KILN_C99_OK) return status;
                }
                *result = built;
                return KILN_C99_OK;
            }
            if (!kiln_c99_is_callable(iterator_method)) {
                return kiln_c99_throw_not_a(thrown, iterator_method, "function");
            }
            status = kiln_c99_iterate(host, built, source, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (!kiln_c99_is_callable(mapper)) {
                *result = built;
                return KILN_C99_OK;
            }
            collected = kiln_c99_array_of(built);
            for (index = 0; index < collected->count; ++index) {
                kiln_value visit[2];
                kiln_value produced;
                visit[0] = kiln_val_to_boundary(collected->elements[index]);
                visit[1] = kiln_c99_number((double)index);
                status = kiln_c99_call(host, &produced, mapper,
                                       argc > 2 ? args[2] : kiln_c99_undefined(), visit, 2, depth,
                                       thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                collected->elements[index] = kiln_val_from_boundary(produced);
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE: {
            /* Own *and* enumerable, which is the pair `Object.keys` reports and `hasOwnProperty`
             * only half of. */
            kiln_value name = kiln_c99_property_key(argc > 0 ? args[0] : kiln_c99_undefined());
            kiln_object *object;
            size_t slot;
            *result = kiln_c99_boolean(0);
            if (receiver.tag == KILN_VALUE_TYPED_ARRAY && name.tag == KILN_VALUE_STRING) {
                int found;
                int enumerable;
                kiln_c99_typed_own(receiver, name.ptr, name.len, &found, &enumerable);
                *result = kiln_c99_boolean(found && enumerable);
                return KILN_C99_OK;
            }
            if (receiver.tag != KILN_VALUE_OBJECT || name.tag != KILN_VALUE_STRING) {
                return KILN_C99_OK;
            }
            object = kiln_c99_object_of(receiver);
            if (object->shape != NULL) {
                kiln_c99_ensure_dictionary(object);
            }
            for (slot = 0; slot < object->count; ++slot) {
                if (kiln_c99_same_key(&object->properties[slot], name.ptr, name.len)) {
                    *result = kiln_c99_boolean(object->properties[slot].enumerable);
                    break;
                }
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_IS_PROTOTYPE_OF: {
            /* Walks the chain rather than comparing one link, which is what makes it different
             * from `Object.getPrototypeOf(x) === this`. */
            kiln_value walked = argc > 0 ? args[0] : kiln_c99_undefined();
            size_t steps;
            *result = kiln_c99_boolean(0);
            for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
                if (walked.tag != KILN_VALUE_OBJECT) {
                    break;
                }
                walked = kiln_c99_object_of(walked)->prototype;
                if (kiln_c99_strict_equal(walked, receiver)) {
                    *result = kiln_c99_boolean(1);
                    break;
                }
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS:
        case KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE: {
            /* Weaker than freezing, and the difference is the point: a non-extensible object still
             * takes writes to what it already has. A library asks `isExtensible` before deciding
             * it may cache or wrap a value, so answering `isFrozen` for it would be a wrong answer
             * rather than a missing one.
             *
             * A primitive is not an error: the specification sends a non-object through unchanged
             * and reports it as non-extensible, which is what `Object.isExtensible(1)` is. */
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            int *flag = NULL;
            int frozen = 0;
            if (subject.tag == KILN_VALUE_OBJECT) {
                kiln_object *object = kiln_c99_object_of(subject);
                /* A module namespace is born non-extensible and cannot be made otherwise. */
                if (object->module_namespace) {
                    *result = code == KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE
                                  ? kiln_c99_boolean(0)
                                  : subject;
                    return KILN_C99_OK;
                }
                flag = &object->not_extensible;
                frozen = object->frozen;
            } else if (subject.tag == KILN_VALUE_ARRAY) {
                flag = &kiln_c99_array_of(subject)->not_extensible;
                frozen = kiln_c99_array_of(subject)->frozen;
            } else if (subject.tag == KILN_VALUE_FUNCTION) {
                kiln_closure *closure = kiln_c99_closure_of(subject);
                if (closure->properties.tag != KILN_VALUE_OBJECT) {
                    status = kiln_c99_new_object(&closure->properties);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                flag = &kiln_c99_object_of(closure->properties)->not_extensible;
                frozen = kiln_c99_object_of(closure->properties)->frozen;
            }
            if (flag == NULL) {
                /* A primitive keeps the specification's answer: it is not extensible, and
                 * `preventExtensions` hands it back unchanged. */
                if (!kiln_c99_is_reference(subject) && subject.tag != KILN_VALUE_DATE) {
                    *result = code == KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE ? kiln_c99_boolean(0)
                                                                          : subject;
                    return KILN_C99_OK;
                }
                /* An object-like value this backend keeps no extensibility slot for -- a
                 * collection, a date, a promise, a regexp, a typed array, a bound function.
                 * Answering `false` for one was a wrong answer rather than a missing feature:
                 * nothing has prevented its extensions, so the language says true. It cost the
                 * whole of vue's collection reactivity, because `getTargetType` asks
                 * `isExtensible` before it will wrap a Map and reads false as "do not wrap": a
                 * reactive Map and Set came back unwrapped, no trap ever ran, and every effect
                 * over one ran exactly once.
                 *
                 * `preventExtensions` is the half that stays a refusal -- there is nowhere to
                 * record it, and one that quietly did nothing would make the answer above lie in
                 * the other direction. A proxy refuses both: either is a trap call. */
                if (subject.tag == KILN_VALUE_PROXY ||
                    code == KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS) {
                    KILN_C99_REFUSE("Object.preventExtensions, or an extensibility question about "
                                    "a Proxy");
                }
                *result = kiln_c99_boolean(1);
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE) {
                /* Freezing implies non-extensible, so it has to be read here too: an object
                 * frozen without ever being told to prevent extensions is still not extensible. */
                *result = kiln_c99_boolean(!*flag && !frozen);
                return KILN_C99_OK;
            }
            *flag = 1;
            /* The target, so a call can be inlined into the expression that built it. */
            *result = subject;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_FREEZE:
        case KILN_C99_METHOD_OBJECT_IS_FROZEN: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            int *flag = NULL;
            if (subject.tag == KILN_VALUE_OBJECT) {
                if (kiln_c99_object_of(subject)->module_namespace) {
                    kiln_object *namespace_object = kiln_c99_object_of(subject);
                    size_t namespace_index;
                    int has_writable_export = 0;
                    /* A namespace is already non-extensible. A non-empty one cannot make its
                     * export descriptors non-writable without violating the exotic contract;
                     * an empty namespace has only the fixed toStringTag slot and is frozen. */
                    for (namespace_index = 0; namespace_index < namespace_object->count;
                         ++namespace_index) {
                        if (namespace_object->properties[namespace_index].writable) {
                            has_writable_export = 1;
                            break;
                        }
                    }
                    if (code == KILN_C99_METHOD_OBJECT_FREEZE) {
                        if (has_writable_export)
                            return kiln_c99_throw_module_namespace_freeze(thrown);
                        namespace_object->frozen = 1;
                        *result = subject;
                        return KILN_C99_OK;
                    }
                    *result = kiln_c99_boolean(!has_writable_export);
                    return KILN_C99_OK;
                }
                flag = &kiln_c99_object_of(subject)->frozen;
            } else if (subject.tag == KILN_VALUE_ARRAY) {
                /* An array is not a `kiln_object` here, so freezing one has to reach the flag on
                 * the array itself -- and its elements are not properties of a store, which is
                 * why they need their own check rather than inheriting the object one. */
                flag = &kiln_c99_array_of(subject)->frozen;
            } else if (subject.tag == KILN_VALUE_FUNCTION) {
                /* A function's own properties live in an object it makes on first use. Freezing
                 * the function is freezing that store, so it has to exist first. */
                kiln_closure *closure = kiln_c99_closure_of(subject);
                if (closure->properties.tag != KILN_VALUE_OBJECT) {
                    int status = kiln_c99_new_object(&closure->properties);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                flag = &kiln_c99_object_of(closure->properties)->frozen;
            }
            if (subject.tag == KILN_VALUE_PROXY) {
                kiln_value inner = kiln_c99_proxy_inner(subject);
                return kiln_c99_invoke_method(host, result, code, kiln_c99_undefined(), &inner, 1,
                                              depth, thrown);
            }
            if (flag == NULL) {
                /* A built-in -- `Object`, `Math`, `JSON`, `Array.prototype.push` -- is an object
                 * whose properties are writable and configurable, so it is not frozen and there
                 * is nowhere to record that it became so. It shares this arm with the primitives
                 * only because neither keeps a store; answering `true` for it said the realm's
                 * own namespaces could not be changed, which every one of them can. */
                int is_object = subject.tag == KILN_VALUE_BUILTIN ||
                                subject.tag == KILN_VALUE_METHOD ||
                                subject.tag == KILN_VALUE_GENERATOR ||
                                subject.tag == KILN_VALUE_SETTLER ||
                                subject.tag == KILN_VALUE_TEMPORAL ||
                                subject.tag == KILN_VALUE_COMMONJS_REQUIRE ||
                                subject.tag == KILN_VALUE_COLLECTION ||
                                subject.tag == KILN_VALUE_PROMISE;
                /* A primitive has nothing to freeze and is already as frozen as it gets. */
                *result = code == KILN_C99_METHOD_OBJECT_FREEZE ? subject
                                                                : kiln_c99_boolean(!is_object);
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_OBJECT_FREEZE) {
                *flag = 1;
                if (subject.tag == KILN_VALUE_OBJECT) {
                    kiln_object *obj = kiln_c99_object_of(subject);
                    size_t i;
                    if (obj->shape != NULL) {
                        kiln_c99_ensure_dictionary(obj);
                        obj->shape = NULL;
                    }
                    for (i = 0; i < obj->count; ++i) {
                        if (!obj->properties[i].accessor) {
                            obj->properties[i].writable = 0;
                        }
                        obj->properties[i].configurable = 0;
                    }
                } else if (subject.tag == KILN_VALUE_FUNCTION) {
                    kiln_closure *closure = kiln_c99_closure_of(subject);
                    if (closure->properties.tag == KILN_VALUE_OBJECT) {
                        kiln_object *obj = kiln_c99_object_of(closure->properties);
                        size_t i;
                        if (obj->shape != NULL) {
                            kiln_c99_ensure_dictionary(obj);
                            obj->shape = NULL;
                        }
                        for (i = 0; i < obj->count; ++i) {
                            if (!obj->properties[i].accessor) {
                                obj->properties[i].writable = 0;
                            }
                            obj->properties[i].configurable = 0;
                        }
                    }
                } else if (subject.tag == KILN_VALUE_ARRAY) {
                    kiln_array *arr = kiln_c99_array_of(subject);
                    if (arr->properties.tag == KILN_VALUE_OBJECT) {
                        kiln_object *obj = kiln_c99_object_of(arr->properties);
                        size_t i;
                        if (obj->shape != NULL) {
                            kiln_c99_ensure_dictionary(obj);
                            obj->shape = NULL;
                        }
                        for (i = 0; i < obj->count; ++i) {
                            if (!obj->properties[i].accessor) {
                                obj->properties[i].writable = 0;
                            }
                            obj->properties[i].configurable = 0;
                        }
                    }
                }
                /* The object itself, so `Object.freeze(o) === o` and a freeze can be inlined into
                 * the expression that built it. */
                *result = subject;
            } else {
                *result = kiln_c99_boolean(*flag);
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_CREATE: {
            kiln_value built;
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            /* `Object.create(null)` records the absence, which is not the same as recording
             * nothing: an object with no link at all inherits `Object.prototype`. */
            if (argc > 0 &&
                (args[0].tag == KILN_VALUE_NULL || kiln_c99_can_be_prototype(args[0]))) {
                kiln_c99_object_of(built)->prototype = args[0];
            } else if (argc > 0) {
                KILN_C99_REFUSE("Object.create given a prototype that cannot be one");
            }
            /* The second argument is the same map of descriptors `defineProperties` takes, and
             * it goes through the same code -- a definition, not an assignment, so an accessor
             * arrives as an accessor and the prototype's setters are not consulted. */
            if (argc > 1) {
                status = kiln_c99_define_properties(host, built, args[1], depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_HAS_OWN: {
            /* The static spelling of `hasOwnProperty`, and the same helper answers both. */
            int found = 0;
            /* Both arguments, for the reason `hasOwnProperty` gives above. */
            if (argc < 2) {
                KILN_C99_REFUSE("Object.hasOwn needs both a target and a key");
            }
            status = kiln_c99_own_property_present(host, args[0], args[1], &found, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_boolean(found);
            return KILN_C99_OK;
        }
        default:
            KILN_C99_REFUSE("an Object or Reflect static this backend has not implemented");
        }
    }
    if ((groups & KILN_C99_GROUP_STRING_DISPATCH)) {
        if (receiver.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a String.prototype method on a receiver that is not a string");
        }
        return kiln_c99_invoke_string_method(result, code, receiver, args, argc);
    }
    if (receiver.tag == KILN_VALUE_TYPED_ARRAY) {
        kiln_typed_array *view = kiln_c99_typed_array_of(receiver);
        if (code == KILN_C99_METHOD_ARRAY_GENERIC_MAP ||
            code == KILN_C99_METHOD_ARRAY_PARALLEL_MAP ||
            code == KILN_C99_METHOD_ARRAY_GENERIC_FILTER) {
            /* A method borrowed from Array.prototype is generic: its result is an ordinary
             * Array, even when the receiver is a typed view. Keep the original receiver in the
             * callback's third argument as well; materialising a snapshot here would make
             * `Array.prototype.map.call(view, (value, index, seen) => seen === view)` observe
             * the implementation detail instead of the language value. */
            kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
            kiln_value built;
            if (!kiln_c99_is_callable(callback)) {
                return kiln_c99_throw_not_a(thrown, callback, "function");
            }
            status = kiln_c99_new_array(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            for (index = 0; index < view->length; ++index) {
                kiln_value visit[3];
                kiln_value produced;
                visit[0] = kiln_c99_number(kiln_c99_typed_get(view, index));
                visit[1] = kiln_c99_number((double)index);
                visit[2] = receiver;
                status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth,
                                       thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (code != KILN_C99_METHOD_ARRAY_GENERIC_FILTER) {
                    status = kiln_c99_array_push(built, produced);
                } else if (kiln_c99_truthy(produced)) {
                    status = kiln_c99_array_push(built, visit[0]);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = built;
            return KILN_C99_OK;
        }
        switch (code) {
        case KILN_C99_METHOD_JOIN: {
            kiln_text joined = {NULL, 0, 0};
            kiln_value separator = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value shown_separator;
            const char *separator_bytes = ",";
            size_t separator_len = 1;
            if (separator.tag != KILN_VALUE_UNDEFINED) {
                shown_separator = kiln_c99_text(separator);
                if (shown_separator.tag != KILN_VALUE_STRING) {
                    KILN_C99_REFUSE("Array.prototype.join given a separator with no text form");
                }
                separator_bytes = shown_separator.ptr;
                separator_len = shown_separator.len;
            }
            for (index = 0; index < view->length; ++index) {
                kiln_value shown;
                if (index != 0 && kiln_c99_text_append(&joined, separator_bytes, separator_len) !=
                                      KILN_C99_OK) {
                    free(joined.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                shown = kiln_c99_text(kiln_c99_number(kiln_c99_typed_get(view, index)));
                if (shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_text_append(&joined, shown.ptr, shown.len) != KILN_C99_OK) {
                    free(joined.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            *result = kiln_c99_text_value(&joined);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ARRAY_VALUES:
            return kiln_c99_new_typed_iterator(result, receiver, 0);
        case KILN_C99_METHOD_ARRAY_ENTRIES:
            return kiln_c99_new_typed_iterator(result, receiver, 1);
        case KILN_C99_METHOD_ARRAY_KEYS:
            return kiln_c99_new_typed_iterator(result, receiver, 2);
        case KILN_C99_METHOD_TYPED_SUBARRAY: {
            size_t start = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                                   view->length, 0);
            size_t end = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                                 view->length, view->length);
            kiln_typed_array *sub =
                (kiln_typed_array *)kiln_c99_gc_alloc(KILN_GC_TYPED_ARRAY, sizeof *sub);
            if (sub == NULL) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            sub->buffer = view->buffer;
            sub->byte_offset = view->byte_offset + start * kiln_c99_typed_width(view->kind);
            sub->length = end >= start ? end - start : 0;
            sub->kind = view->kind;
            /* A slice of a Buffer is a Buffer. The field was left unwritten while the view came
             * from malloc, which read uninitialised memory; a collected block is no more zeroed
             * than malloc was, so the answer has to be written down. */
            sub->is_buffer = view->is_buffer;
            *result = kiln_c99_value(KILN_VALUE_TYPED_ARRAY, (uint64_t)(uintptr_t)sub, NULL, 0);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_SLICE: {
            size_t width = kiln_c99_typed_width(view->kind);
            size_t start = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                                   view->length, 0);
            size_t end = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                                 view->length, view->length);
            size_t count = end >= start ? end - start : 0;
            kiln_typed_array *copy;
            status = kiln_c99_typed_alloc(result, view->kind, count);
            if (status != KILN_C99_OK) {
                return status;
            }
            copy = kiln_c99_typed_array_of(*result);
            if (count != 0) {
                /* The bytes rather than the elements: a copy of the same kind reinterprets them
                 * identically, so there is nothing for a per-element conversion to decide. */
                memcpy(copy->buffer->bytes,
                       view->buffer->bytes + view->byte_offset + start * width,
                       count * width);
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_MAP:
        case KILN_C99_METHOD_FILTER: {
            /* Both answer with a view of the receiver's kind. `filter`'s length is not known
             * until the predicate has run, so what it keeps is gathered first and the view is
             * sized from that; `map` fills one of the receiver's own length.
             *
             * Gathering as `double` rather than into the view directly is what keeps a callback
             * that reads the receiver seeing the values it started with. */
            kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
            double *kept;
            size_t count = 0;
            if (!kiln_c99_is_callable(callback)) {
                return kiln_c99_throw_not_a(thrown, callback, "function");
            }
            kept = (double *)malloc((view->length == 0 ? 1 : view->length) * sizeof *kept);
            if (kept == NULL) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            for (index = 0; index < view->length; ++index) {
                kiln_value visit[3];
                kiln_value produced;
                visit[0] = kiln_c99_number(kiln_c99_typed_get(view, index));
                visit[1] = kiln_c99_number((double)index);
                visit[2] = receiver;
                status =
                    kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
                if (status != KILN_C99_OK) {
                    free(kept);
                    return status;
                }
                if (code == KILN_C99_METHOD_FILTER) {
                    if (kiln_c99_truthy(produced)) {
                        kept[count++] = kiln_c99_as_number(visit[0]);
                    }
                    continue;
                }
                {
                    /* `map` converts what the callback answered, so a callback returning "5"
                     * stores 5 -- the element write is the conversion, exactly as `view[i] = x`
                     * is. */
                    double converted;
                    status = kiln_c99_to_number_checked(host, produced, &converted, depth, thrown);
                    if (status != KILN_C99_OK) {
                        free(kept);
                        return status;
                    }
                    kept[count++] = converted;
                }
            }
            status = kiln_c99_typed_alloc(result, view->kind, count);
            if (status != KILN_C99_OK) {
                free(kept);
                return status;
            }
            for (index = 0; index < count; ++index) {
                kiln_c99_typed_set(kiln_c99_typed_array_of(*result), index, kept[index]);
            }
            free(kept);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ARRAY_REVERSE: {
            /* In place, and the receiver is the answer -- `view.reverse()` is the view it just
             * changed, not a copy, which is what makes `a.reverse() === a` true. */
            size_t low = 0;
            size_t high = view->length == 0 ? 0 : view->length - 1;
            while (low < high) {
                double kept = kiln_c99_typed_get(view, low);
                kiln_c99_typed_set(view, low, kiln_c99_typed_get(view, high));
                kiln_c99_typed_set(view, high, kept);
                ++low;
                --high;
            }
            *result = receiver;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ARRAY_SORT: {
            /* Insertion sort, because a comparator is program code: it can throw, and it can
             * read the view it is sorting. Sorting a copy of the values and writing them back at
             * the end is what keeps a throw from leaving the view half-rearranged. */
            kiln_value comparator = argc > 0 ? args[0] : kiln_c99_undefined();
            double *items;
            if (comparator.tag != KILN_VALUE_UNDEFINED && !kiln_c99_is_callable(comparator)) {
                return kiln_c99_throw_not_a(thrown, comparator, "function");
            }
            items = (double *)malloc((view->length == 0 ? 1 : view->length) * sizeof *items);
            if (items == NULL) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            for (index = 0; index < view->length; ++index) {
                items[index] = kiln_c99_typed_get(view, index);
            }
            if (view->length > 1) {
                kiln_c99_typed_sort_context ctx;
                ctx.host = host;
                ctx.comparator = comparator;
                ctx.depth = depth;
                ctx.thrown = thrown;
                status = kiln_c99_timsort_typed(&ctx, items, view->length);
                if (status != KILN_C99_OK) {
                    free(items);
                    return status;
                }
            }
            for (index = 0; index < view->length; ++index) {
                kiln_c99_typed_set(view, index, items[index]);
            }
            free(items);
            *result = receiver;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_TYPED_SET: {
            size_t offset = 0;
            size_t count;
            double *items;
            if (argc == 0 || (args[0].tag != KILN_VALUE_ARRAY &&
                              args[0].tag != KILN_VALUE_TYPED_ARRAY)) {
                KILN_C99_REFUSE("TypedArray.prototype.set needs an array or a typed array to copy "
                                "from");
            }
            if (argc > 1) {
                status = kiln_c99_to_index(args[1], &offset, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            count = args[0].tag == KILN_VALUE_ARRAY
                        ? kiln_c99_array_of(args[0])->count
                        : kiln_c99_typed_array_of(args[0])->length;
            if (offset > view->length || count > view->length - offset) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "offset is out of bounds");
            }
            if (count > ((size_t)-1) / sizeof *items) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            items = (double *)malloc((count == 0 ? 1 : count) * sizeof *items);
            if (items == NULL) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            for (index = 0; index < count; ++index) {
                items[index] = args[0].tag == KILN_VALUE_ARRAY
                                   ? kiln_c99_as_number(kiln_val_to_boundary(kiln_c99_array_of(args[0])->elements[index]))
                                   : kiln_c99_typed_get(kiln_c99_typed_array_of(args[0]), index);
            }
            for (index = 0; index < count; ++index) {
                kiln_c99_typed_set(view, offset + index, items[index]);
            }
            free(items);
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_TYPED_FILL: {
            double filler = argc > 0 ? kiln_c99_as_number(args[0]) : NAN;
            size_t start = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                                   view->length, 0);
            size_t end = kiln_c99_relative_index(argc > 2 ? args[2] : kiln_c99_undefined(),
                                                 view->length, view->length);
            for (index = start; index < end; ++index) {
                kiln_c99_typed_set(view, index, filler);
            }
            *result = receiver;
            return KILN_C99_OK;
        }
        default:
            /* Everything else a view shares with `Array.prototype` -- `indexOf`, `includes`,
             * `reduce`, `some`, `at`, `toString` and the rest -- is a *reader*, and the generic
             * array-like walk below already implements those against anything with a `length`
             * and indices. Falling through rather than refusing is what gives a view all of them
             * without a second copy of each; only the four that must answer with a view of their
             * own, or rearrange one in place, are written out above. */
            break;
        }
    }
    if (receiver.tag != KILN_VALUE_ARRAY) {
        /* Redirected onto something that is not an array. The array methods are generic: they
         * walk whatever has a `length` and indexed properties, which is what makes
         * `Array.prototype.slice.call(arguments)` and `[].map.call("abc", f)` work. A snapshot
         * of that walk is taken here and the method runs against it.
         *
         * Only the readers come this way. A mutator would have to write its result back through
         * the array-like, and a snapshot cannot carry that, so those still refuse -- an honest
         * "not implemented" rather than a silent no-op on the caller's object. */
        kiln_value length;
        kiln_value snapshot;
        double count;
        size_t at;
        if (!kiln_c99_reads_only(code)) {
            KILN_C99_REFUSE("an array method that writes back, reached through an array-like");
        }
        /* A number or a boolean has no `length` and no indices, so the walk is empty -- which is
         * why `[].slice.call(7).length` is 0. Asking for the property instead would be answered
         * with "this backend has no such member", and that is a different sentence: the language
         * says the property is absent, not unknown. */
        if (receiver.tag == KILN_VALUE_NUMBER || receiver.tag == KILN_VALUE_BOOLEAN ||
            receiver.tag == KILN_VALUE_SYMBOL || receiver.tag == KILN_VALUE_BIGINT) {
            length = kiln_c99_undefined();
        } else {
            status = kiln_c99_get(host, receiver, "length", 6, &length, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        /* No `length` is an empty walk rather than an error, which is why
         * `[].slice.call(7).length` is 0 and not a throw. */
        count = kiln_c99_as_number(length);
        if (!(count >= 1.0)) {
            count = 0.0;
        } else if (count > 4294967295.0) {
            count = 4294967295.0;
        } else {
            count = floor(count);
        }
        status = kiln_c99_new_array(&snapshot);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (at = 0; at < (size_t)count; ++at) {
            kiln_value element;
            status = kiln_c99_get_computed(host, receiver, kiln_c99_number((double)at), &element,
                                           depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_array_push(snapshot, element);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_invoke_method(host, result, code, snapshot, args, argc, depth, thrown);
    }
    array = kiln_c99_array_of(receiver);
    if (array->frozen && kiln_c99_changes_length(code)) {
        /* A TypeError rather than a silent no-op: an element write has an answer to give without
         * doing anything -- the value it was handed -- and `push` does not. Its answer is the new
         * length, and there is no honest one. */
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot add or remove an element of a frozen array");
    }
    switch (code) {
    case KILN_C99_METHOD_PUSH:
        for (index = 0; index < argc; ++index) {
            status = kiln_c99_array_push(receiver, args[index]);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        *result = kiln_c99_number((double)array->count);
        return KILN_C99_OK;
    case KILN_C99_METHOD_POP:
        if (array->count == 0) {
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        array->count -= 1;
        *result = kiln_val_to_boundary(array->elements[array->count]);
        return KILN_C99_OK;
    case KILN_C99_METHOD_JOIN: {
        kiln_text joined = {NULL, 0, 0};
        kiln_value separator = argc > 0 ? args[0] : kiln_c99_undefined();
        const char *bytes = ",";
        size_t len = 1;
        if (separator.tag != KILN_VALUE_UNDEFINED) {
            kiln_value shown = kiln_c99_text(separator);
            if (shown.tag != KILN_VALUE_STRING) {
                KILN_C99_REFUSE("join given a separator with no text form");
            }
            bytes = shown.ptr;
            len = shown.len;
        }
        for (index = 0; index < array->count; ++index) {
            kiln_val element = array->elements[index];
            if (index != 0 && kiln_c99_text_append(&joined, bytes, len) != KILN_C99_OK) {
                free(joined.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            /* A nullish element, and a hole, contribute nothing -- which is what makes
             * `[1, , 3].join()` produce "1,,3" rather than "1,undefined,3". */
            if (element == KILN_VAL_UNDEFINED || element == KILN_VAL_NULL ||
                element == KILN_VAL_HOLE) {
                continue;
            }
            if (KILN_VAL_IS_STRING(element)) {
                const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(element);
                if (s != NULL && s->byte_len > 0) {
                    if (kiln_c99_text_append(&joined, s->bytes, s->byte_len) != KILN_C99_OK) {
                        free(joined.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
                continue;
            }
            {
                kiln_value shown = kiln_c99_text(kiln_val_to_boundary(element));
                if (shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_text_append(&joined, shown.ptr, shown.len) != KILN_C99_OK) {
                    free(joined.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
        }
        *result = kiln_c99_text_value(&joined);
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_INDEX_OF:
    case KILN_C99_METHOD_INCLUDES: {
        kiln_value needle = argc > 0 ? args[0] : kiln_c99_undefined();
        /* Where the walk starts, counting from the end when negative. Ignoring it answered
         * `[1,2,3,4].indexOf(3, 3)` with 2 -- a position before the one the caller asked to
         * start from, which reads as a find rather than as a miss. */
        size_t start = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                               array->count, 0);
        int found = 0;
        for (index = start; index < array->count; ++index) {
            if (array->elements[index] == KILN_VAL_HOLE) {
                /* The only place `indexOf` and `includes` disagree, and the reason `includes`
                 * exists: one skips a hole and the other reads it as undefined. */
                found = code == KILN_C99_METHOD_INCLUDES && needle.tag == KILN_VALUE_UNDEFINED;
                if (found) {
                    break;
                }
                continue;
            }
            kiln_value elem_val = kiln_val_to_boundary(array->elements[index]);
            found = code == KILN_C99_METHOD_INCLUDES
                        ? kiln_c99_same_value_zero(elem_val, needle)
                        : kiln_c99_strict_equal(elem_val, needle);
            if (found) {
                break;
            }
        }
        if (code == KILN_C99_METHOD_INCLUDES) {
            *result = kiln_c99_boolean(found);
        } else {
            *result = kiln_c99_number(found ? (double)index : -1.0);
        }
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_SLICE: {
        size_t start = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t end = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                             array->count, array->count);
        kiln_value built;
        size_t slice_len = (end > start && start < array->count) ? (end < array->count ? end - start : array->count - start) : 0;
        status = kiln_c99_new_array_with_capacity(&built, slice_len, array->is_numeric);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (slice_len > 0) {
            kiln_array *b_arr = kiln_c99_array_of(built);
            memcpy(b_arr->elements, array->elements + start, slice_len * sizeof(kiln_val));
            b_arr->count = slice_len;
        }
        *result = built;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_CONCAT: {
        kiln_value built;
        size_t argument;
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < array->count; ++index) {
            status = kiln_c99_array_push(built, kiln_val_to_boundary(array->elements[index]));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        for (argument = 0; argument < argc; ++argument) {
            /* An array argument contributes its elements, one level deep; anything else
             * contributes itself. */
            if (args[argument].tag == KILN_VALUE_ARRAY) {
                kiln_array *other = kiln_c99_array_of(args[argument]);
                for (index = 0; index < other->count; ++index) {
                    status = kiln_c99_array_push(built, kiln_val_to_boundary(other->elements[index]));
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
            } else {
                status = kiln_c99_array_push(built, args[argument]);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
        }
        *result = built;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_TO_STRING:
        /* `join` with the separator fixed rather than defaulted -- a program can pass one to
         * `join` and cannot to `toString`, which is why `String([1, 2])` is `"1,2"`. */
        return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_JOIN, receiver, NULL, 0, depth,
                                      thrown);
    case KILN_C99_METHOD_ARRAY_FIND_LAST:
    case KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
        size_t at = array->count;
        *result = code == KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX ? kiln_c99_number(-1.0)
                                                                : kiln_c99_undefined();
        while (at > 0) {
            kiln_value visit[3];
            kiln_value produced;
            at -= 1;
            if (at >= array->count) {
                /* The callback shortened the array out from under the walk. */
                continue;
            }
            /* A hole is visited as undefined here, unlike `find`: the language added the
             * last-first pair after it stopped skipping them. */
            visit[0] = array->elements[at] == KILN_VAL_HOLE ? kiln_c99_undefined()
                                                            : kiln_val_to_boundary(array->elements[at]);
            visit[1] = kiln_c99_number((double)at);
            visit[2] = receiver;
            status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (kiln_c99_truthy(produced)) {
                *result = code == KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX
                              ? kiln_c99_number((double)at)
                              : visit[0];
                break;
            }
        }
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_SPLICE: {
        kiln_value removed;
        size_t start = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t removing;
        size_t inserting = argc > 2 ? argc - 2 : 0;
        size_t tail;
        /* A missing count removes everything from `start`, where an explicit `undefined` removes
         * nothing: `splice(1)` and `splice(1, undefined)` are different calls. */
        if (argc < 2) {
            removing = array->count - start;
        } else {
            double wanted = kiln_c99_as_number(args[1]);
            removing = isnan(wanted) || wanted < 0.0        ? 0
                       : wanted > (double)(array->count - start) ? array->count - start
                                                                 : (size_t)wanted;
        }
        status = kiln_c99_new_array(&removed);
        for (index = 0; index < removing && status == KILN_C99_OK; ++index) {
            status = kiln_c99_array_push(removed, kiln_val_to_boundary(array->elements[start + index]));
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        status = kiln_c99_array_reserve(array, array->count - removing + inserting);
        if (status != KILN_C99_OK) {
            return status;
        }
        tail = array->count - start - removing;
        /* An array that has never grown has `elements == NULL`, and then `start`, `removing` and
         * `tail` are all zero -- so this moved nothing, from `NULL + 0` to `NULL + 0`. Computing
         * that offset is undefined even though nothing is read, and `memmove` declares both
         * pointers non-null however small the count. gcc emits code that happens to work, which
         * is why the corpus passed; `zig cc` traps, and UBSan calls it "applying zero offset to
         * null pointer". Guarding on the count says what the code already meant. */
        if (tail != 0) {
            memmove(array->elements + start + inserting, array->elements + start + removing,
                    tail * sizeof *array->elements);
        }
        for (index = 0; index < inserting; ++index) {
            array->elements[start + index] = kiln_val_from_boundary(args[index + 2]);
        }
        array->count = array->count - removing + inserting;
        /* The removed elements, which is what makes `splice` usable as a queue read. */
        *result = removed;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_COPY_WITHIN: {
        size_t target = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                                array->count, 0);
        size_t start = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t end = kiln_c99_relative_index(argc > 2 ? args[2] : kiln_c99_undefined(),
                                             array->count, array->count);
        size_t moving = end > start ? end - start : 0;
        if (moving > array->count - target) {
            moving = array->count - target;
        }
        /* `memmove` rather than a loop, because the source and the destination overlap by design
         * -- that overlap is the whole reason the method exists. */
        memmove(array->elements + target, array->elements + start,
                moving * sizeof *array->elements);
        *result = receiver;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_REDUCE_RIGHT: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value accumulator;
        size_t at = array->count;
        if (argc > 1) {
            accumulator = args[1];
        } else {
            /* The seed is the last element that is *there*: an array of nothing but holes has no
             * answer to give, exactly as an empty one does not. */
            while (at > 0 && array->elements[at - 1] == KILN_VAL_HOLE) {
                at -= 1;
            }
            if (at == 0) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Reduce of empty array with no initial value");
            }
            at -= 1;
            accumulator = kiln_val_to_boundary(array->elements[at]);
        }
        while (at > 0) {
            kiln_value visit[4];
            at -= 1;
            if (at >= array->count || array->elements[at] == KILN_VAL_HOLE) {
                continue;
            }
            visit[0] = accumulator;
            visit[1] = kiln_val_to_boundary(array->elements[at]);
            visit[2] = kiln_c99_number((double)at);
            visit[3] = receiver;
            status = kiln_c99_call(host, &accumulator, callback, kiln_c99_undefined(), visit, 4,
                                   depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        *result = accumulator;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FLAT_MAP:
    case KILN_C99_METHOD_MAP:
    case KILN_C99_METHOD_ARRAY_GENERIC_MAP:
    case KILN_C99_METHOD_ARRAY_PARALLEL_MAP:
    case KILN_C99_METHOD_FILTER:
    case KILN_C99_METHOD_ARRAY_GENERIC_FILTER:
    case KILN_C99_METHOD_FOR_EACH: {
        kiln_value built = kiln_c99_undefined();
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_val vcallee = kiln_val_from_boundary(callback);
        if ((code == KILN_C99_METHOD_MAP || code == KILN_C99_METHOD_ARRAY_PARALLEL_MAP) &&
            KILN_VAL_IS_CLOSURE(vcallee) && self_value.tag == KILN_VALUE_UNDEFINED) {
            kiln_value built_val;
            status = kiln_c99_new_array_with_capacity(&built_val, array->count, array->is_numeric);
            if (status != KILN_C99_OK) return status;
            kiln_array *built_arr = kiln_c99_array_of(built_val);
            kiln_val vrecv = kiln_val_from_boundary(receiver);
            size_t length = array->count;
            for (index = 0; index < length && index < array->count; ++index) {
                if (array->elements[index] == KILN_VAL_HOLE) {
                    built_arr->elements[built_arr->count++] = KILN_VAL_HOLE;
                    built_arr->is_numeric = 0;
                    continue;
                }
                kiln_val vvisit[3];
                vvisit[0] = array->elements[index];
                vvisit[1] = kiln_c99_val_from_double((double)index);
                vvisit[2] = vrecv;
                kiln_val prod = KILN_VAL_UNDEFINED;
                status = kiln_c99_val_call(host, vcallee, vvisit, 3, depth, thrown, NULL, 0, NULL, &prod);
                if (status != KILN_C99_OK) return status;
                if (built_arr->is_numeric && !KILN_VAL_IS_NUMBER(prod)) {
                    built_arr->is_numeric = 0;
                }
                built_arr->elements[built_arr->count++] = prod;
            }
            *result = built_val;
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_FILTER && KILN_VAL_IS_CLOSURE(vcallee) && self_value.tag == KILN_VALUE_UNDEFINED) {
            kiln_value built_val;
            status = kiln_c99_new_array_with_capacity(&built_val, array->count, array->is_numeric);
            if (status != KILN_C99_OK) return status;
            kiln_array *built_arr = kiln_c99_array_of(built_val);
            kiln_val vrecv = kiln_val_from_boundary(receiver);
            size_t length = array->count;
            for (index = 0; index < length && index < array->count; ++index) {
                if (array->elements[index] == KILN_VAL_HOLE) {
                    continue;
                }
                kiln_val elem = array->elements[index];
                kiln_val vvisit[3];
                vvisit[0] = elem;
                vvisit[1] = kiln_c99_val_from_double((double)index);
                vvisit[2] = vrecv;
                kiln_val prod = KILN_VAL_UNDEFINED;
                status = kiln_c99_val_call(host, vcallee, vvisit, 3, depth, thrown, NULL, 0, NULL, &prod);
                if (status != KILN_C99_OK) return status;
                if (kiln_c99_val_truthy(prod)) {
                    built_arr->elements[built_arr->count++] = elem;
                }
            }
            *result = built_val;
            return KILN_C99_OK;
        }
        if (code != KILN_C99_METHOD_FOR_EACH) {
            status = kiln_c99_new_array(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        /* The length is read once, so an element the callback appends is not visited -- which is
         * what the specification says and what stops an appending callback from looping. */
        {
            size_t length = array->count;
            for (index = 0; index < length && index < array->count; ++index) {
                kiln_value visit[3];
                kiln_value produced;
                /* Every element walk skips a hole and visits an undefined. `map` still has to
                 * *leave* one: its result has an entry per input position, and skipping would
                 * shift everything after it. */
                if (array->elements[index] == KILN_VAL_HOLE) {
                    if (code == KILN_C99_METHOD_MAP ||
                        code == KILN_C99_METHOD_ARRAY_GENERIC_MAP ||
                        code == KILN_C99_METHOD_ARRAY_PARALLEL_MAP) {
                        status = kiln_c99_array_push(built, kiln_c99_hole());
                        if (status != KILN_C99_OK) {
                            return status;
                        }
                    }
                    continue;
                }
                visit[0] = kiln_val_to_boundary(array->elements[index]);
                visit[1] = kiln_c99_number((double)index);
                visit[2] = receiver;
                status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (code == KILN_C99_METHOD_MAP ||
                    code == KILN_C99_METHOD_ARRAY_GENERIC_MAP ||
                    code == KILN_C99_METHOD_ARRAY_PARALLEL_MAP ||
                    code == KILN_C99_METHOD_ARRAY_FLAT_MAP) {
                    status = kiln_c99_array_push(built, produced);
                } else if ((code == KILN_C99_METHOD_FILTER ||
                            code == KILN_C99_METHOD_ARRAY_GENERIC_FILTER) &&
                           kiln_c99_truthy(produced)) {
                    status = kiln_c99_array_push(built, visit[0]);
                } else {
                    status = KILN_C99_OK;
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
        }
        if (code == KILN_C99_METHOD_ARRAY_FLAT_MAP) {
            /* One level, and only of what the callback produced -- `flatMap` does not flatten an
             * array that was already an element. */
            kiln_value one = kiln_c99_number(1.0);
            return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_ARRAY_FLAT, built, &one, 1,
                                          depth, thrown);
        }
        *result = built;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_AT: {
        /* A negative index counts from the end, which is the whole reason `at` exists alongside
         * indexing; out of range is undefined rather than clamped. */
        double relative = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        double position;
        /* `ToIntegerOrInfinity` first, so a NaN index is zero and not out of range: `[1].at(NaN)`
         * is `[1].at(0)`. Without it every comparison below is false for a NaN and the answer
         * came back undefined -- the spelling of "there is nothing there", for an index that
         * names the first element. The string `at` beside this one already had the check. */
        if (relative != relative) {
            relative = 0.0;
        }
        relative = trunc(relative);
        position = relative < 0.0 ? (double)array->count + relative : relative;
        *result = position >= 0.0 && position < (double)array->count &&
                          array->elements[(size_t)position] != KILN_VAL_HOLE
                      ? kiln_val_to_boundary(array->elements[(size_t)position])
                      : kiln_c99_undefined();
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_LAST_INDEX_OF: {
        kiln_value needle = argc > 0 ? args[0] : kiln_c99_undefined();
        double found = -1.0;
        /* `fromIndex` is where the *backwards* walk starts, counting from the end when
         * negative -- and a negative that runs off the front leaves nowhere to start, which is
         * -1 rather than the whole array. That last case is why this cannot use
         * `kiln_c99_relative_index`, which clamps such a position to zero and would go on to
         * report a match at the very element the caller excluded. */
        size_t limit = array->count;
        if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
            double from = kiln_c99_as_number(args[1]);
            if (from != from) {
                from = 0.0;
            }
            from = trunc(from);
            if (from < 0.0) {
                from += (double)array->count;
            }
            if (from < 0.0) {
                *result = kiln_c99_number(-1.0);
                return KILN_C99_OK;
            }
            limit = from >= (double)array->count ? array->count : (size_t)from + 1;
        }
        for (index = limit; index > 0; --index) {
            if (array->elements[index - 1] == KILN_VAL_HOLE) {
                continue;
            }
            if (kiln_c99_strict_equal(kiln_val_to_boundary(array->elements[index - 1]), needle)) {
                found = (double)(index - 1);
                break;
            }
        }
        *result = kiln_c99_number(found);
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FIND:
    case KILN_C99_METHOD_ARRAY_FIND_INDEX:
    case KILN_C99_METHOD_ARRAY_SOME:
    case KILN_C99_METHOD_ARRAY_EVERY: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
        size_t length = array->count;
        int matched = 0;
        /* `every` is the only one that answers true for an empty array, and the only one that
         * stops on a *falsy* result. Everything else about the four is the same walk. */
        for (index = 0; index < length && index < array->count; ++index) {
            kiln_value visit[3];
            kiln_value produced;
            /* `find` and `findIndex` visit a hole as undefined, where `some` and `every` skip
             * it -- the same split `includes` and `indexOf` make. */
            if (array->elements[index] == KILN_VAL_HOLE) {
                if (code == KILN_C99_METHOD_ARRAY_SOME || code == KILN_C99_METHOD_ARRAY_EVERY) {
                    continue;
                }
                visit[0] = kiln_c99_undefined();
            } else {
                visit[0] = kiln_val_to_boundary(array->elements[index]);
            }
            visit[1] = kiln_c99_number((double)index);
            visit[2] = receiver;
            status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            matched = kiln_c99_truthy(produced);
            if (code == KILN_C99_METHOD_ARRAY_EVERY ? !matched : matched) {
                break;
            }
        }
        switch (code) {
        case KILN_C99_METHOD_ARRAY_FIND:
            *result = matched && index < array->count ? kiln_val_to_boundary(array->elements[index])
                                                      : kiln_c99_undefined();
            if (result->tag == KILN_VALUE_HOLE) {
                *result = kiln_c99_undefined();
            }
            break;
        case KILN_C99_METHOD_ARRAY_FIND_INDEX:
            *result = kiln_c99_number(matched ? (double)index : -1.0);
            break;
        case KILN_C99_METHOD_ARRAY_SOME: *result = kiln_c99_boolean(matched); break;
        default: *result = kiln_c99_boolean(index >= length || matched); break;
        }
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_SORT: {
        /* An insertion sort, which is stable -- and stability is program-visible, so it is part
         * of the answer rather than an implementation detail. The comparator can call back into
         * the program, so every comparison has to be able to fail. */
        kiln_value comparator = argc > 0 ? args[0] : kiln_c99_undefined();
        size_t sorted;
        /* Holes and undefined sort to the end, ahead of the comparator ever seeing them. */
        size_t live = 0;
        /* `undefined` means the default ordering; anything else has to be callable. The check
         * runs before a single element moves, so a refused sort leaves the array exactly as it
         * was -- and it lives here rather than in `toSorted` because both need it and only one
         * of them should own it. Falling through to the text comparison instead was a silent
         * wrong answer: `a.sort("nope")` reordered the array where every other engine throws. */
        if (comparator.tag != KILN_VALUE_UNDEFINED && !kiln_c99_is_callable(comparator)) {
            return kiln_c99_throw_error(
                thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                "The comparison function must be either a function or undefined");
        }
        for (index = 0; index < array->count; ++index) {
            if (array->elements[index] != KILN_VAL_HOLE &&
                array->elements[index] != KILN_VAL_UNDEFINED) {
                array->elements[live++] = array->elements[index];
            }
        }
        for (index = live; index < array->count; ++index) {
            array->elements[index] = KILN_VAL_UNDEFINED;
        }
        if (live > 1) {
            kiln_c99_sort_context ctx;
            ctx.host = host;
            ctx.comparator = comparator;
            ctx.depth = depth;
            ctx.thrown = thrown;
            status = kiln_c99_timsort_val(&ctx, array->elements, live);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        /* The array itself, so `a.sort()` can be chained and `a.sort() === a` holds. */
        *result = receiver;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_REVERSE: {
        size_t half;
        for (half = 0; half * 2 + 1 < array->count; ++half) {
            kiln_val swap = array->elements[half];
            array->elements[half] = array->elements[array->count - 1 - half];
            array->elements[array->count - 1 - half] = swap;
        }
        *result = receiver;
        return KILN_C99_OK;
    }
    /* The four ES2023 change-by-copy methods, each the counterpart of one that mutates: `with`
     * to an index assignment, `toReversed` to `reverse`, `toSorted` to `sort`, `toSpliced` to
     * `splice`. They share a case because they share a shape -- read the receiver into a fresh
     * array, then let the counterpart rearrange *that* -- and sharing it is what keeps them
     * honest: one ordering and one splice, so a copy and its mutating twin can never disagree
     * about where a comparator's ties land or how a count is clamped.
     *
     * Every hole becomes `undefined` on the way in, which is the other half of what these
     * methods promise: the result is dense however sparse the receiver was.
     *
     * The receiver is read *before* the counterpart runs, so a comparator that reaches back and
     * changes it cannot change the answer -- which is the property that made the language add
     * these in the first place. */
    case KILN_C99_METHOD_ARRAY_TO_SORTED:
    case KILN_C99_METHOD_ARRAY_TO_REVERSED:
    case KILN_C99_METHOD_ARRAY_TO_SPLICED:
    case KILN_C99_METHOD_ARRAY_WITH: {
        kiln_value copy;
        kiln_value discarded;
        size_t replacing = 0;
        if (code == KILN_C99_METHOD_ARRAY_WITH) {
            /* `ToIntegerOrInfinity`, and then a negative index counts from the end. An index
             * that lands outside the array is a `RangeError` rather than an appended element:
             * `with` answers a copy of the *same* length, and there is nowhere to put the value.
             *
             * `kiln_c99_relative_index` cannot be borrowed for this. It clamps, which is the
             * answer `slice` and `at` want and is precisely the one this has to refuse -- a
             * clamp would quietly write over the first or last element instead of throwing. */
            double relative = kiln_c99_as_number(argc > 0 ? args[0] : kiln_c99_undefined());
            double at;
            relative = isnan(relative) ? 0.0 : trunc(relative);
            at = relative >= 0.0 ? relative : (double)array->count + relative;
            if (!(at >= 0.0) || at >= (double)array->count) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "Invalid index");
            }
            replacing = (size_t)at;
        }
        status = kiln_c99_new_array(&copy);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < array->count; ++index) {
            kiln_value element = array->elements[index] == KILN_VAL_HOLE
                                     ? kiln_c99_undefined()
                                     : kiln_val_to_boundary(array->elements[index]);
            if (code == KILN_C99_METHOD_ARRAY_WITH && index == replacing) {
                /* Stored as it was handed over: the replacement is the one argument of these
                 * four that must not be converted to anything. */
                element = argc > 1 ? args[1] : kiln_c99_undefined();
            }
            status = kiln_c99_array_push(copy, element);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        /* The copy is the answer in every case, including the three that hand it on: `sort`,
         * `reverse` and `splice` all rearrange it in place, and only `splice` has a return value
         * of its own -- the elements it removed, which is exactly what `toSpliced` must *not*
         * answer. */
        *result = copy;
        if (code == KILN_C99_METHOD_ARRAY_WITH) {
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_ARRAY_TO_SPLICED) {
            /* `toSpliced()` with no arguments at all is a plain copy -- an absent `start` skips
             * nothing -- where `toSpliced(1)` has a start and no count and so runs to the end.
             * `splice` reads a missing second argument as "to the end" for both, which is right
             * for the one-argument form and wrong for the empty call it shares that branch with,
             * so the empty call is answered here rather than handed down. */
            if (argc == 0) {
                return KILN_C99_OK;
            }
            return kiln_c99_invoke_method(host, &discarded, KILN_C99_METHOD_ARRAY_SPLICE, copy,
                                          args, argc, depth, thrown);
        }
        return kiln_c99_invoke_method(host, &discarded,
                                      code == KILN_C99_METHOD_ARRAY_TO_SORTED
                                          ? KILN_C99_METHOD_ARRAY_SORT
                                          : KILN_C99_METHOD_ARRAY_REVERSE,
                                      copy, args, argc, depth, thrown);
    }
    case KILN_C99_METHOD_ARRAY_SHIFT: {
        if (array->count == 0) {
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        *result = array->elements[0] == KILN_VAL_HOLE ? kiln_c99_undefined()
                                                       : kiln_val_to_boundary(array->elements[0]);
        memmove(array->elements, array->elements + 1,
                (array->count - 1) * sizeof *array->elements);
        array->count -= 1;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_UNSHIFT: {
        size_t added;
        status = kiln_c99_array_reserve(array, array->count + argc);
        if (status != KILN_C99_OK) {
            return status;
        }
        memmove(array->elements + argc, array->elements, array->count * sizeof *array->elements);
        for (added = 0; added < argc; ++added) {
            array->elements[added] = kiln_val_from_boundary(args[added]);
        }
        array->count += argc;
        *result = kiln_c99_number((double)array->count);
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FILL: {
        kiln_value filler = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_val filler_val = kiln_val_from_boundary(filler);
        size_t start = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t end = kiln_c99_relative_index(argc > 2 ? args[2] : kiln_c99_undefined(),
                                             array->count, array->count);
        for (index = start; index < end; ++index) {
            array->elements[index] = filler_val;
        }
        *result = receiver;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FLAT: {
        /* One level unless told otherwise, and a hole disappears rather than flattening to
         * undefined. */
        double depth_wanted = argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED
                                  ? kiln_c99_as_number(args[0])
                                  : 1.0;
        kiln_value built;
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
            kiln_val elem_v = array->elements[index];
            if (elem_v == KILN_VAL_HOLE) {
                continue;
            }
            kiln_value element = kiln_val_to_boundary(elem_v);
            if (element.tag == KILN_VALUE_ARRAY && depth_wanted >= 1.0) {
                kiln_value flattened;
                kiln_value inner_depth = kiln_c99_number(depth_wanted - 1.0);
                kiln_array *nested;
                size_t inner;
                status = kiln_c99_invoke_method(host, &flattened, KILN_C99_METHOD_ARRAY_FLAT,
                                                element, &inner_depth, 1, depth + 1, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                nested = kiln_c99_array_of(flattened);
                for (inner = 0; inner < nested->count && status == KILN_C99_OK; ++inner) {
                    status = kiln_c99_array_push(built, kiln_val_to_boundary(nested->elements[inner]));
                }
                continue;
            }
            status = kiln_c99_array_push(built, element);
        }
        *result = built;
        return status;
    }
    case KILN_C99_METHOD_ARRAY_ENTRIES:
    case KILN_C99_METHOD_ARRAY_KEYS: {
        kiln_value snapshot;
        status = kiln_c99_new_array(&snapshot);
        for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
            kiln_value entry = kiln_c99_number((double)index);
            if (code == KILN_C99_METHOD_ARRAY_ENTRIES) {
                kiln_value pair;
                status = kiln_c99_new_array(&pair);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(pair, entry);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(pair, array->elements[index] == KILN_VAL_HOLE
                                                           ? kiln_c99_undefined()
                                                           : kiln_val_to_boundary(array->elements[index]));
                }
                entry = pair;
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_array_push(snapshot, entry);
            }
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_new_iterator(result, snapshot);
    }
    case KILN_C99_METHOD_REDUCE: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value accumulator;
        size_t length = array->count;
        index = 0;
        if (argc > 1) {
            accumulator = args[1];
        } else {
            /* Without a seed the accumulator is the first element that is *there*: folding a
             * hole in would turn every sum over a sparse array into NaN. An array of nothing but
             * holes has no answer to give, exactly as an empty one does not. */
            while (index < length && array->elements[index] == KILN_VAL_HOLE) {
                index += 1;
            }
            if (index >= length) {
                /* A TypeError the program can catch, not a refusal: `[].reduce(f)` is ordinary
                 * code with an ordinary error, and a fatal status would make it unwritable. */
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Reduce of empty array with no initial value");
            }
            accumulator = kiln_val_to_boundary(array->elements[index]);
            index += 1;
        }
        kiln_val vcallee = kiln_val_from_boundary(callback);
        if (KILN_VAL_IS_CLOSURE(vcallee)) {
            kiln_val vacc = kiln_val_from_boundary(accumulator);
            kiln_val vrecv = kiln_val_from_boundary(receiver);
            for (; index < length && index < array->count; ++index) {
                if (array->elements[index] == KILN_VAL_HOLE) {
                    continue;
                }
                kiln_val vvisit[4];
                vvisit[0] = vacc;
                vvisit[1] = array->elements[index];
                vvisit[2] = kiln_c99_val_from_double((double)index);
                vvisit[3] = vrecv;
                status = kiln_c99_val_call(host, vcallee, vvisit, 4, depth, thrown, NULL, 0, NULL, &vacc);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = kiln_val_to_boundary(vacc);
            return KILN_C99_OK;
        }
        for (; index < length && index < array->count; ++index) {
            kiln_value visit[4];
            if (array->elements[index] == KILN_VAL_HOLE) {
                continue;
            }
            visit[0] = accumulator;
            visit[1] = kiln_val_to_boundary(array->elements[index]);
            visit[2] = kiln_c99_number((double)index);
            visit[3] = receiver;
            status = kiln_c99_call(host, &accumulator, callback, kiln_c99_undefined(), visit, 4,
                                   depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        *result = accumulator;
        return KILN_C99_OK;
    }
    default:
        KILN_C99_REFUSE("an array method this backend has not implemented");
    }
}
