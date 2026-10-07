/* The WebCrypto JavaScript policy is shared TypeScript. This private bridge forwards byte spans
 * to the same Rust implementation linked directly by the interpreter; no algorithm lives here. */

static int kiln_c99_webcrypto_bytes(kiln_value value, const uint8_t **bytes, size_t *length,
                                    kiln_value *thrown) {
    kiln_typed_array *view;
    if (value.tag == KILN_VALUE_UNDEFINED) {
        *bytes = NULL;
        *length = 0;
        return KILN_C99_OK;
    }
    if (value.tag != KILN_VALUE_TYPED_ARRAY)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "WebCrypto data must be a Uint8Array");
    view = kiln_c99_typed_array_of(value);
    if (view == NULL || view->kind != KILN_TYPED_UINT8 || view->buffer == NULL ||
        view->buffer->detached)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "WebCrypto data must be a live Uint8Array");
    *length = view->length;
    *bytes = view->length == 0 ? NULL : view->buffer->bytes + view->byte_offset;
    return KILN_C99_OK;
}

int kiln_c99_webcrypto_primitive(kiln_value *out, const kiln_value *args, size_t argc,
                                 kiln_value *thrown) {
    const uint8_t *key = NULL, *data = NULL, *iv = NULL, *additional_data = NULL;
    size_t key_length = 0, data_length = 0, iv_length = 0, additional_data_length = 0;
    size_t capacity = 0, required = 0, output_length = 0;
    uint32_t operation;
    double operation_number, requested_length;
    kiln_typed_array *result_view;
    int status;
    if (args == NULL || argc < 6 || args[0].tag != KILN_VALUE_NUMBER ||
        args[5].tag != KILN_VALUE_NUMBER)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Invalid internal WebCrypto arguments");
    operation_number = kiln_c99_as_number(args[0]);
    requested_length = kiln_c99_as_number(args[5]);
    if (!isfinite(operation_number) || floor(operation_number) != operation_number ||
        operation_number < 1 || operation_number > 12 || !isfinite(requested_length) ||
        floor(requested_length) != requested_length || requested_length < 0 ||
        requested_length > (operation_number == 12 ? 65536 : 8160))
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Invalid internal WebCrypto operation");
    operation = (uint32_t)operation_number;
    output_length = (size_t)requested_length;
#define WEBCRYPTO_READ(index, target, size)                                      \
    do {                                                                         \
        status = kiln_c99_webcrypto_bytes(args[(index)], &(target), &(size), thrown); \
        if (status != KILN_C99_OK) return status;                                \
    } while (0)
    WEBCRYPTO_READ(1, key, key_length);
    WEBCRYPTO_READ(2, data, data_length);
    WEBCRYPTO_READ(3, iv, iv_length);
    WEBCRYPTO_READ(4, additional_data, additional_data_length);
#undef WEBCRYPTO_READ
    switch (operation) {
    case 1: case 10: capacity = 32; break;
    case 11: capacity = 1; break;
    case 12: capacity = output_length; break;
    case 2: capacity = output_length; break;
    case 3:
        if (data_length > SIZE_MAX - 16)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "WebCrypto input is too large");
        capacity = data_length + 16;
        break;
    case 4: capacity = data_length; break;
    case 5: capacity = 128; break;
    case 6: capacity = 32; break;
    case 7: capacity = 64; break;
    case 8: capacity = 1; break;
    case 9: capacity = 0; break;
    default:
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Unsupported internal WebCrypto operation");
    }
    status = kiln_c99_webcrypto_output_array(out, capacity);
    if (status != KILN_C99_OK) return status;
    result_view = kiln_c99_typed_array_of(*out);
    if (result_view == NULL || result_view->buffer == NULL)
        return KILN_C99_OUT_OF_MEMORY;
    status = kiln_webcrypto_execute(operation, key, key_length, data, data_length, iv, iv_length,
                                    additional_data, additional_data_length, output_length,
                                    capacity == 0 ? NULL : result_view->buffer->bytes +
                                                              result_view->byte_offset,
                                    capacity, &required);
    if (status == -2 || status == -3)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "WebCrypto primitive rejected its input");
    if (status == -5)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR,
                                    "WebCrypto authentication failed");
    if (status == -6)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Unsupported WebCrypto primitive");
    if (status == -7)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR,
                                    "Secure random generation failed");
    if (status != 0 || required > capacity)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR,
                                    "WebCrypto provider failed");
    result_view->length = required;
    return KILN_C99_OK;
}
