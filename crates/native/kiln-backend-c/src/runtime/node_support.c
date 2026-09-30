
/* Materialize a borrowed byte span directly as a Buffer. The caller owns the span only for the
 * current callback, so the copy into GC-managed typed-array storage completes before returning.
 * This avoids the old string intermediate and its per-byte conversion. */
static int kiln_c99_buffer_from_bytes(kiln_value *out, const uint8_t *bytes, uint64_t length) {
    size_t size;
    int status;
    if (length > SIZE_MAX) return KILN_C99_INVALID_ARGUMENT;
    size = (size_t)length;
    if (size != 0 && bytes == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_typed_alloc_ex(out, KILN_TYPED_UINT8, size, 0);
    if (status != KILN_C99_OK) return status;
    if (size != 0) memcpy(kiln_c99_typed_array_of(*out)->buffer->bytes, bytes, size);
    kiln_c99_typed_array_of(*out)->is_buffer = 1;
    return KILN_C99_OK;
}

static int kiln_c99_buffer_encoding_is(const char *name, size_t length, const char *wanted) {
    size_t index;
    if (strlen(wanted) != length) return 0;
    for (index = 0; index < length; ++index)
        if (tolower((unsigned char)name[index]) != wanted[index]) return 0;
    return 1;
}

/* Encode the string forms accepted by Buffer.from and Buffer.alloc's fill argument. The Node
 * bootstrap has a richer TypeScript prototype surface, but the static Buffer methods are
 * immutable backend intrinsics, so their byte-producing paths need the same encodings here. */
static int kiln_c99_buffer_encode_string(kiln_value *out, kiln_value source,
                                         kiln_value encoding, kiln_value *thrown) {
    const kiln_string *text;
    const char *name = "utf8";
    size_t name_len = 4;
    uint8_t *bytes = NULL;
    size_t count = 0;
    size_t index;
    if (source.tag != KILN_VALUE_STRING || source.ptr == NULL) return KILN_C99_INVALID_ARGUMENT;
    text = (const kiln_string *)source.ptr;
    if (encoding.tag != KILN_VALUE_UNDEFINED && encoding.tag != KILN_VALUE_NULL) {
        if (encoding.tag != KILN_VALUE_STRING || encoding.ptr == NULL)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Unknown encoding");
        name = kiln_c99_string_ascii_bytes(encoding, &name_len);
        if (name == NULL) return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                      "Unknown encoding");
    }
    if ((kiln_c99_buffer_encoding_is(name, name_len, "utf8")) ||
        (kiln_c99_buffer_encoding_is(name, name_len, "utf-8"))) {
        uint8_t *utf8 = NULL;
        size_t utf8_len = 0;
        int status = kiln_c99_string_to_utf8_lossy(source, &utf8, &utf8_len);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_buffer_from_bytes(out, utf8, utf8_len);
        free(utf8);
        return status;
    }
    if ((kiln_c99_buffer_encoding_is(name, name_len, "utf16le")) ||
        (kiln_c99_buffer_encoding_is(name, name_len, "utf-16le")) ||
        (kiln_c99_buffer_encoding_is(name, name_len, "ucs2")) ||
        (kiln_c99_buffer_encoding_is(name, name_len, "ucs-2"))) {
        if (text->unit_len > SIZE_MAX / 2) return KILN_C99_OUT_OF_MEMORY;
        count = text->unit_len * 2;
        bytes = count == 0 ? NULL : malloc(count);
        if (count != 0 && bytes == NULL) return KILN_C99_OUT_OF_MEMORY;
        for (index = 0; index < text->unit_len; ++index) {
            uint16_t unit = kiln_string_unit_at(text, index);
            bytes[index * 2] = (uint8_t)unit;
            bytes[index * 2 + 1] = (uint8_t)(unit >> 8);
        }
    } else if ((kiln_c99_buffer_encoding_is(name, name_len, "hex"))) {
        size_t limit = text->unit_len / 2;
        bytes = limit == 0 ? NULL : malloc(limit);
        if (limit != 0 && bytes == NULL) return KILN_C99_OUT_OF_MEMORY;
        for (index = 0; index < limit; ++index) {
            uint32_t a = kiln_string_unit_at(text, index * 2);
            uint32_t b = kiln_string_unit_at(text, index * 2 + 1);
            int high = a >= '0' && a <= '9' ? (int)(a - '0')
                     : a >= 'a' && a <= 'f' ? (int)(a - 'a' + 10)
                     : a >= 'A' && a <= 'F' ? (int)(a - 'A' + 10) : -1;
            int low = b >= '0' && b <= '9' ? (int)(b - '0')
                    : b >= 'a' && b <= 'f' ? (int)(b - 'a' + 10)
                    : b >= 'A' && b <= 'F' ? (int)(b - 'A' + 10) : -1;
            if (high < 0 || low < 0) break;
            bytes[count++] = (uint8_t)(high * 16 + low);
        }
    } else if (kiln_c99_buffer_encoding_is(name, name_len, "base64") ||
               kiln_c99_buffer_encoding_is(name, name_len, "base64url")) {
        size_t capacity = text->unit_len / 4 * 3 + 3;
        uint32_t bits = 0;
        uint32_t accumulator = 0;
        bytes = capacity == 0 ? NULL : malloc(capacity);
        if (capacity != 0 && bytes == NULL) return KILN_C99_OUT_OF_MEMORY;
        for (index = 0; index < text->unit_len; ++index) {
            uint32_t unit = kiln_string_unit_at(text, index);
            int value = unit >= 'A' && unit <= 'Z' ? (int)(unit - 'A')
                      : unit >= 'a' && unit <= 'z' ? (int)(unit - 'a' + 26)
                      : unit >= '0' && unit <= '9' ? (int)(unit - '0' + 52)
                      : unit == '+' || unit == '-' ? 62
                      : unit == '/' || unit == '_' ? 63 : -1;
            if (unit == '=') break;
            if (value < 0) continue;
            accumulator = (accumulator << 6) | (uint32_t)value;
            bits += 6;
            if (bits >= 8) {
                bits -= 8;
                bytes[count++] = (uint8_t)(accumulator >> bits);
                accumulator &= bits == 0 ? 0 : ((UINT32_C(1) << bits) - 1);
            }
        }
    } else if ((kiln_c99_buffer_encoding_is(name, name_len, "latin1")) ||
               (kiln_c99_buffer_encoding_is(name, name_len, "binary")) ||
               (kiln_c99_buffer_encoding_is(name, name_len, "ascii"))) {
        count = text->unit_len;
        bytes = count == 0 ? NULL : malloc(count);
        if (count != 0 && bytes == NULL) return KILN_C99_OUT_OF_MEMORY;
        for (index = 0; index < count; ++index)
            bytes[index] = (uint8_t)kiln_string_unit_at(text, index);
    } else {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Unknown encoding");
    }
    {
        int status = kiln_c99_buffer_from_bytes(out, bytes, count);
        free(bytes);
        return status;
    }
}

static int kiln_c99_buffer_from(kiln_value *out, kiln_value source, const kiln_value *args,
                                size_t argc, kiln_value *thrown) {
    kiln_value built;
    int status;
    if (source.tag == KILN_VALUE_ARRAY_BUFFER) {
        /* `Buffer.from(arrayBuffer, byteOffset, length)` shares the bytes, which is what makes
         * the shared prototype's `subarray` and `slice` windows rather than copies. */
        size_t offset = 0;
        size_t length = 0;
        int has_length = argc > 2 && args[2].tag != KILN_VALUE_UNDEFINED;
        if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
            status = kiln_c99_to_index(args[1], &offset, thrown);
            if (status != KILN_C99_OK) return status;
        }
        if (has_length) {
            status = kiln_c99_to_index(args[2], &length, thrown);
            if (status != KILN_C99_OK) return status;
        }
        status = kiln_c99_new_typed_array(&built, KILN_TYPED_UINT8, source, offset, length,
                                          has_length, thrown);
        if (status != KILN_C99_OK) return status;
        kiln_c99_typed_array_of(built)->is_buffer = 1;
        *out = built;
        return KILN_C99_OK;
    }
    if (source.tag == KILN_VALUE_STRING) {
        return kiln_c99_buffer_encode_string(out, source,
                                             argc > 1 ? args[1] : kiln_c99_undefined(), thrown);
    } else if (source.tag == KILN_VALUE_TYPED_ARRAY || source.tag == KILN_VALUE_ARRAY) {
        status = kiln_c99_new_typed_array(&built, KILN_TYPED_UINT8, source, 0, 0, 0, thrown);
        if (status != KILN_C99_OK) return status;
    } else {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Buffer.from needs a string, array, or typed array");
    }
    kiln_c99_typed_array_of(built)->is_buffer = 1;
    *out = built;
    return KILN_C99_OK;
}

/* `Buffer.prototype`: the object the runtime bootstrap assigned to the `Buffer` built-in's
 * store, or undefined before it has (and in a realm without the Node bootstrap). Reads on a
 * Buffer consult it first and `Object.getPrototypeOf` answers it. */
int kiln_c99_buffer_prototype(kiln_host *host, kiln_value *out, int depth, kiln_value *thrown) {
    kiln_value store;
    int status = kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_BUFFER), &store, 0);
    *out = kiln_c99_undefined();
    if (status != KILN_C99_OK) return status;
    if (store.tag != KILN_VALUE_OBJECT) return KILN_C99_OK;
    return kiln_c99_get(host, store, "prototype", 9, out, depth, thrown);
}

/* The typed array a boxed value names, or NULL when it names something else. */
static kiln_typed_array *kiln_c99_val_typed_array(kiln_val value) {
    void *ptr;
    if ((value & KILN_TAG_MASK) != KILN_TAG_BUFFER_PREFIX) return NULL;
    ptr = KILN_VAL_PTR(value);
    if (ptr == NULL || kiln_c99_gc_payload_kind(ptr) != KILN_GC_TYPED_ARRAY) return NULL;
    return (kiln_typed_array *)ptr;
}

static int kiln_c99_buffer_method(kiln_value *out, uint64_t code, kiln_value receiver,
                                  const kiln_value *args, size_t argc, kiln_value *thrown) {
    kiln_value first = argc > 0 ? args[0] : kiln_c99_undefined();
    if (code == KILN_C99_METHOD_BUFFER_FROM) return kiln_c99_buffer_from(out, first, args, argc, thrown);
    if (code == KILN_C99_METHOD_BUFFER_ALLOC) {
        size_t length;
        size_t index;
        int status = kiln_c99_to_index(first, &length, thrown);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_typed_alloc(out, KILN_TYPED_UINT8, length);
        if (status != KILN_C99_OK) return status;
        kiln_c99_typed_array_of(*out)->is_buffer = 1;
        if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED && length != 0) {
            kiln_value fill = args[1];
            if (fill.tag == KILN_VALUE_NUMBER) {
                double number = kiln_c99_as_number(fill);
                uint8_t byte = 0;
                if (isfinite(number) && number != 0.0) {
                    double reduced = fmod(trunc(number), 256.0);
                    if (reduced < 0.0) reduced += 256.0;
                    byte = (uint8_t)reduced;
                }
                memset(kiln_c99_typed_array_of(*out)->buffer->bytes, byte, length);
            } else {
                kiln_value pattern;
                if (fill.tag == KILN_VALUE_STRING) {
                    kiln_value encoding = argc > 2 ? args[2] : kiln_c99_undefined();
                    status = kiln_c99_buffer_encode_string(&pattern, fill, encoding, thrown);
                } else if (fill.tag == KILN_VALUE_TYPED_ARRAY || fill.tag == KILN_VALUE_ARRAY) {
                    status = kiln_c99_new_typed_array(&pattern, KILN_TYPED_UINT8, fill, 0, 0,
                                                      0, thrown);
                    if (status == KILN_C99_OK) kiln_c99_typed_array_of(pattern)->is_buffer = 1;
                } else {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "The \"value\" argument is invalid");
                }
                if (status != KILN_C99_OK) return status;
                kiln_typed_array *source = kiln_c99_typed_array_of(pattern);
                if (source->length == 0)
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                "The argument 'value' is invalid");
                for (index = 0; index < length; ++index)
                    kiln_c99_typed_set(kiln_c99_typed_array_of(*out), index,
                                       kiln_c99_typed_get(source, index % source->length));
            }
        }
        return status;
    }
    if (code == KILN_C99_METHOD_BUFFER_IS_BUFFER) {
        *out = kiln_c99_boolean(first.tag == KILN_VALUE_TYPED_ARRAY &&
                                kiln_c99_typed_array_of(first)->is_buffer);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_BUFFER_BYTE_LENGTH) {
        if (first.tag == KILN_VALUE_STRING) {
            /* UTF-8 bytes, not code units: `Buffer.byteLength("é")` is 2. */
            uint8_t *encoded = NULL;
            size_t encoded_len = 0;
            int status = kiln_c99_string_to_utf8_lossy(first, &encoded, &encoded_len);
            if (status != KILN_C99_OK) return status;
            free(encoded);
            *out = kiln_c99_number((double)encoded_len);
            return KILN_C99_OK;
        }
        *out = kiln_c99_number(first.tag == KILN_VALUE_STRING ? (double)first.len :
            first.tag == KILN_VALUE_TYPED_ARRAY ?
                (double)(kiln_c99_typed_array_of(first)->length *
                         kiln_c99_typed_width(kiln_c99_typed_array_of(first)->kind)) :
                (double)kiln_c99_text(first).len);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_BUFFER_CONCAT) {
        kiln_array *items;
        size_t total = 0;
        size_t at;
        size_t written = 0;
        int status;
        if (first.tag != KILN_VALUE_ARRAY)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Buffer.concat needs an array");
        items = kiln_c99_array_of(first);
        /* The entries are read as the boxed values they are: a typed array is the buffer prefix
         * over a payload whose header says so, one load each, where the boundary conversion of
         * every entry -- twice, once to measure and once to copy -- was a third of `Buffer.concat`
         * over a thousand chunks. */
        for (at = 0; at < items->count; ++at) {
            const kiln_typed_array *view = kiln_c99_val_typed_array(items->elements[at]);
            if (view == NULL)
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Buffer.concat entries must be typed arrays");
            total += view->length;
        }
        /* Every byte of the result is written below. */
        status = kiln_c99_typed_alloc_ex(out, KILN_TYPED_UINT8, total, 0);
        if (status != KILN_C99_OK) return status;
        for (at = 0; at < items->count; ++at) {
            kiln_typed_array *view = kiln_c99_val_typed_array(items->elements[at]);
            kiln_typed_array *target = kiln_c99_typed_array_of(*out);
            size_t index;
            /* A Buffer is a `Uint8Array`, and so is what `Buffer.concat` allocates, so the
             * overwhelmingly common case is bytes into bytes -- which is a `memcpy` and was
             * written as a loop over `typed_get`/`typed_set`. Those two are generic: each element
             * dispatched on the view's kind, bounds-checked, widened to a double and narrowed
             * back. Copying 76 MiB that way took 207ms where node took 2.9ms; it is about 370
             * MiB/s against memcpy's tens of gigabytes.
             *
             * The element loop stays for the case the fast path cannot serve -- an entry that is
             * some other typed array -- because `Buffer.concat` is only documented to take
             * Buffers and refusing a `Uint16Array` outright would be a new error where there was
             * an answer. */
            if (view->kind == KILN_TYPED_UINT8 && target->kind == KILN_TYPED_UINT8) {
                if (view->length != 0)
                    memcpy(target->buffer->bytes + target->byte_offset + written,
                           view->buffer->bytes + view->byte_offset, view->length);
                written += view->length;
                continue;
            }
            for (index = 0; index < view->length; ++index)
                kiln_c99_typed_set(target, written++, kiln_c99_typed_get(view, index));
        }
        kiln_c99_typed_array_of(*out)->is_buffer = 1;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_BUFFER_TO_STRING || code == KILN_C99_METHOD_BUFFER_TO_STRING_RAW) {
        kiln_typed_array *view;
        kiln_value encoding = (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED)
            ? kiln_c99_text(args[0])
            : kiln_c99_copy_text("utf8", 4);
        char norm[32];
        size_t nlen;
        size_t i;
        if (receiver.tag != KILN_VALUE_TYPED_ARRAY || encoding.tag != KILN_VALUE_STRING)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "unsupported Buffer encoding");
        size_t encoding_len = 0;
        const char *encoding_bytes = kiln_c99_string_ascii_bytes(encoding, &encoding_len);
        if (encoding_bytes == NULL) return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                                "unsupported Buffer encoding");
        nlen = encoding_len < 31 ? encoding_len : 31;
        for (i = 0; i < nlen; ++i)
            norm[i] = (char)tolower((unsigned char)encoding_bytes[i]);
        norm[nlen] = '\0';
        if ((nlen == 4 && memcmp(norm, "utf8", 4) == 0) ||
            (nlen == 5 && memcmp(norm, "utf-8", 5) == 0)) {
            view = kiln_c99_typed_array_of(receiver);
            *out = kiln_c99_copy_text((const char *)view->buffer->bytes + view->byte_offset,
                                      view->length);
            return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
        }
        if (nlen == 3 && memcmp(norm, "hex", 3) == 0) {
            kiln_value hex_args[1] = { receiver };
            return kiln_c99_uint8_to_hex_raw(out, hex_args, 1, thrown);
        }
        if (nlen == 6 && memcmp(norm, "base64", 6) == 0) {
            kiln_value b64_args[3] = { receiver, kiln_c99_boolean(0), kiln_c99_boolean(0) };
            return kiln_c99_uint8_to_base64_raw(out, b64_args, 3, thrown);
        }
        if (nlen == 9 && memcmp(norm, "base64url", 9) == 0) {
            kiln_value b64_args[3] = { receiver, kiln_c99_boolean(1), kiln_c99_boolean(1) };
            return kiln_c99_uint8_to_base64_raw(out, b64_args, 3, thrown);
        }
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "unsupported Buffer encoding");
    }
    KILN_C99_REFUSE("a Buffer method this backend has not implemented");
}
