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

static uint8_t kiln_c99_primitive_wrapper_kind(kiln_value primitive) {
    switch (primitive.tag) {
    case KILN_VALUE_STRING: return KILN_C99_WRAPPER_STRING;
    case KILN_VALUE_NUMBER: return KILN_C99_WRAPPER_NUMBER;
    case KILN_VALUE_BOOLEAN: return KILN_C99_WRAPPER_BOOLEAN;
    case KILN_VALUE_SYMBOL: return KILN_C99_WRAPPER_SYMBOL;
    case KILN_VALUE_BIGINT: return KILN_C99_WRAPPER_BIGINT;
    default: return KILN_C99_WRAPPER_NONE;
    }
}

static int kiln_c99_unwrap_primitive_wrapper(kiln_value receiver, uint8_t kind,
                                             kiln_value *primitive) {
    if (receiver.tag != KILN_VALUE_OBJECT) return 0;
    kiln_object *object = kiln_c99_object_of(receiver);
    if (object->primitive_wrapper_kind != kind) return 0;
    *primitive = object->boxed_primitive;
    return 1;
}

/* Primitive wrappers are ordinary realm-owned objects with one extra internal slot. String's
 * indexed characters and length are materialized as ordinary own properties so existing property
 * reflection, Object.assign and spread share the normal store machinery. */
static int kiln_c99_new_primitive_wrapper(kiln_value *out, kiln_value primitive) {
    static const uint64_t prototype_ids[] = {
        0, KILN_C99_BUILTIN_STRING_PROTOTYPE, KILN_C99_BUILTIN_NUMBER_PROTOTYPE,
        KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE, KILN_C99_BUILTIN_SYMBOL_PROTOTYPE,
        KILN_C99_BUILTIN_BIGINT_PROTOTYPE
    };
    uint8_t kind = kiln_c99_primitive_wrapper_kind(primitive);
    kiln_value built;
    kiln_object *object;
    int status;
    if (kind == KILN_C99_WRAPPER_NONE) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_new_object(&built);
    if (status != KILN_C99_OK) return status;
    object = kiln_c99_object_of(built);
    /* Wrapper-owned descriptors are exotic: making this one a dictionary lets the ordinary
     * storage helpers carry their exact attributes without a wrapper-specific shape family. */
    object->shape = NULL;
    kiln_c99_heap_store_pointer(object, &object->slots, NULL);
    object->slot_capacity = 0;
    object->primitive_wrapper_kind = kind;
    kiln_c99_heap_store_value(object, &object->boxed_primitive, primitive);
    kiln_c99_heap_store_value(object, &object->prototype,
                              kiln_c99_builtin(prototype_ids[kind]));
    if (kind == KILN_C99_WRAPPER_STRING) {
        const kiln_string *text = primitive.ptr == NULL ? &kiln_c99_empty_string
                                                         : (const kiln_string *)primitive.ptr;
        size_t index;
        char key[32];
        for (index = 0; index < text->unit_len; ++index) {
            int length = snprintf(key, sizeof key, "%zu", index);
            kiln_value character = kiln_val_to_boundary(
                kiln_c99_val_from_unit(kiln_string_unit_at(text, index)));
            if (length < 0 || (size_t)length >= sizeof key) return KILN_C99_INVALID_ARGUMENT;
            status = kiln_c99_put(NULL, built, key, (size_t)length, character, 0, NULL);
            if (status != KILN_C99_OK) return status;
            object = kiln_c99_object_of(built);
            object->properties[object->count - 1].writable = 0;
            object->properties[object->count - 1].configurable = 0;
        }
        status = kiln_c99_put(NULL, built, "length", 6,
                              kiln_c99_number((double)text->unit_len), 0, NULL);
        if (status != KILN_C99_OK) return status;
        object = kiln_c99_object_of(built);
        object->properties[object->count - 1].enumerable = 0;
        object->properties[object->count - 1].writable = 0;
        object->properties[object->count - 1].configurable = 0;
    }
    *out = built;
    return KILN_C99_OK;
}

/* `Object(x)`, which is `new Object(x)`: nothing becomes an empty object and a reference is
 * handed straight back -- the identity `Object(o) === o` is what makes it usable as a coercion.
 * Primitives become ordinary wrapper objects with their matching prototype and internal slot. */
static int kiln_c99_object_constructor(kiln_value *out, const kiln_value *args, size_t argc) {
    kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
    if (subject.tag == KILN_VALUE_UNDEFINED || subject.tag == KILN_VALUE_NULL) {
        return kiln_c99_new_object(out);
    }
    if (kiln_c99_primitive_wrapper_kind(subject) != KILN_C99_WRAPPER_NONE)
        return kiln_c99_new_primitive_wrapper(out, subject);
    *out = subject;
    return KILN_C99_OK;
}

static kiln_value kiln_c99_binary(uint16_t operation, kiln_value left, kiln_value right);
static kiln_value kiln_c99_unary(uint16_t operation, kiln_value value);
static kiln_value kiln_c99_typeof(kiln_value value);
