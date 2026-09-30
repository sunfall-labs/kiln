/* Date string grammar belongs to kiln-date-core, the semantic owner shared by interpreter and
 * provider. This adapter only preserves the runtime string representation at that boundary. */
static double kiln_c99_parse_date(kiln_value source) {
    if (source.tag != KILN_VALUE_STRING || source.ptr == NULL) return 0.0 / 0.0;
    const kiln_string *text = (const kiln_string *)source.ptr;
    kiln_string view = { .unit_len = text->unit_len,
                         .flags = text->flags & (KILN_STRING_LATIN1 | KILN_STRING_UTF16),
                         .data = text->data };
    return kiln_date_parse(&view);
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
            kiln_c99_heap_store_val(array, &array->elements[index], KILN_VAL_HOLE);
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
