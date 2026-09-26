/* An ISO-8601 instant, or NaN. Deliberately narrow: the shapes a program writes down and the
 * shapes `toISOString` produces, which is what a round trip needs. Anything looser would accept
 * text the interpreter rejects, and the two have to agree. */
static double kiln_c99_parse_date(const char *bytes, size_t len) {
    int64_t fields[7] = {0, 1, 1, 0, 0, 0, 0};
    size_t at = 0;
    size_t index;
    static const size_t WIDTHS[7] = {4, 2, 2, 2, 2, 2, 3};
    while (at < len && (bytes[at] == ' ' || bytes[at] == '\t')) {
        at += 1;
    }
    while (len > at && (bytes[len - 1] == ' ' || bytes[len - 1] == '\t')) {
        len -= 1;
    }
    for (index = 0; index < 7; ++index) {
        size_t width = WIDTHS[index];
        size_t digit;
        if (index > 0) {
            /* The separator each field is introduced by, and the point past which the rest is
             * optional: a date alone is a valid instant. */
            char expected = index == 1 || index == 2 ? '-' : index == 3 ? 'T' : index == 6 ? '.' : ':';
            if (at >= len || bytes[at] != expected) {
                break;
            }
            at += 1;
        }
        if (len - at < width) {
            return 0.0 / 0.0;
        }
        fields[index] = 0;
        for (digit = 0; digit < width; ++digit) {
            char byte = bytes[at + digit];
            if (byte < '0' || byte > '9') {
                return 0.0 / 0.0;
            }
            fields[index] = fields[index] * 10 + (byte - '0');
        }
        at += width;
    }
    /* A trailing `Z` is the only zone accepted, because the realm has no other. */
    if (at < len && bytes[at] == 'Z') {
        at += 1;
    }
    if (at != len || fields[1] < 1 || fields[1] > 12 || fields[2] < 1 || fields[2] > 31) {
        return 0.0 / 0.0;
    }
    return (double)kiln_c99_days_from_civil(fields[0], fields[1], fields[2]) * 86400000.0 +
           (double)fields[3] * 3600000.0 + (double)fields[4] * 60000.0 +
           (double)fields[5] * 1000.0 + (double)fields[6];
}

/* `Array(...)`, which is `new Array(...)`: the two are the same operation, and the ambiguity in
 * it belongs to the language rather than to this backend. Exactly one *numeric* argument is a
 * length and produces that many holes; anything else is the elements, so `new Array('3')` is one
 * string and `new Array(3)` is three empty slots. */
static int kiln_c99_array_constructor(kiln_value *out, const kiln_value *args, size_t argc,
                                      kiln_value *thrown) {
    kiln_value built;
    size_t index;
    int status;
    if (argc == 1 && args[0].tag == KILN_VALUE_NUMBER) {
        double length = kiln_c99_as_number(args[0]);
        kiln_array *array;
        if (!(length >= 0.0) || length != (double)(uint32_t)length) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "Invalid array length");
        }
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        array = kiln_c99_array_of(built);
        status = kiln_c99_array_reserve(array, (size_t)length);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < (size_t)length; ++index) {
            array->elements[index] = KILN_VAL_HOLE;
        }
        array->count = (size_t)length;
        *out = built;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_array(&built);
    if (status != KILN_C99_OK) {
        return status;
    }
    for (index = 0; index < argc; ++index) {
        status = kiln_c99_array_push(built, args[index]);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    *out = built;
    return KILN_C99_OK;
}

/* `Object(x)`, which is `new Object(x)`: nothing becomes an empty object and a reference is
 * handed straight back -- the identity `Object(o) === o` is what makes it usable as a coercion.
 * A primitive would become a wrapper object, which this backend does not have, so it refuses
 * rather than handing back the primitive and calling it an object. */
static int kiln_c99_object_constructor(kiln_value *out, const kiln_value *args, size_t argc) {
    kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
    if (subject.tag == KILN_VALUE_UNDEFINED || subject.tag == KILN_VALUE_NULL) {
        return kiln_c99_new_object(out);
    }
    if (!kiln_c99_is_reference(subject) && subject.tag != KILN_VALUE_DATE &&
        subject.tag != KILN_VALUE_REGEXP && subject.tag != KILN_VALUE_PROMISE) {
        KILN_C99_REFUSE("Object() of a primitive this backend has no wrapper for");
    }
    *out = subject;
    return KILN_C99_OK;
}

static kiln_value kiln_c99_binary(uint16_t operation, kiln_value left, kiln_value right);
static kiln_value kiln_c99_unary(uint16_t operation, kiln_value value);
static kiln_value kiln_c99_typeof(kiln_value value);
