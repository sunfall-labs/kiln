
/* Cancellation is ordinary object state, not a new value kind. That keeps the hot value layout
 * and every host boundary unchanged: only the controller owns the method that can transition its
 * signal, while consumers receive the signal object and its read/listen surface. */
static int kiln_c99_new_abort_signal(kiln_value *out) {
    kiln_value listeners;
    int status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK) status = kiln_c99_new_array(&listeners);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "aborted", 7, kiln_c99_boolean(0), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "reason", 6, kiln_c99_undefined(), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "onabort", 7, kiln_c99_null(), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "addEventListener", 16,
                                        kiln_c99_method(KILN_C99_METHOD_ABORT_SIGNAL_LISTEN));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "removeEventListener", 19,
                                        kiln_c99_method(KILN_C99_METHOD_ABORT_SIGNAL_UNLISTEN));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "throwIfAborted", 14,
                                        kiln_c99_method(KILN_C99_METHOD_ABORT_SIGNAL_THROW));
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "@@abort-listeners", 17, listeners, 0, NULL);
    if (status == KILN_C99_OK) {
        kiln_object *object = kiln_c99_object_of(*out);
        size_t index;
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
            object->shape = NULL;
        }
        for (index = 0; index < object->count; ++index) object->properties[index].enumerable = 0;
    }
    return status;
}

static int kiln_c99_new_abort_controller(kiln_value *out) {
    kiln_value signal;
    int status = kiln_c99_new_abort_signal(&signal);
    if (status == KILN_C99_OK) status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "signal", 6, signal, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "abort", 5,
                                        kiln_c99_method(KILN_C99_METHOD_ABORT_CONTROLLER_ABORT));
    if (status == KILN_C99_OK) {
        kiln_object *object = kiln_c99_object_of(*out);
        size_t index;
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
            object->shape = NULL;
        }
        for (index = 0; index < object->count; ++index) object->properties[index].enumerable = 0;
    }
    return status;
}

static int kiln_c99_new_text_encoder(kiln_value *out) {
    int status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "encoding", 8, kiln_c99_copy_text("utf-8", 5), 0,
                              NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "encode", 6,
                                        kiln_c99_method(KILN_C99_METHOD_TEXT_ENCODER_ENCODE));
    if (status == KILN_C99_OK) {
        kiln_object *object = kiln_c99_object_of(*out);
        size_t index;
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
            object->shape = NULL;
        }
        for (index = 0; index < object->count; ++index) object->properties[index].enumerable = 0;
    }
    return status;
}

static int kiln_c99_ascii_iequals(const char *a, const char *b, size_t len) {
    size_t i;
    for (i = 0; i < len; ++i) {
        char ca = a[i];
        char cb = b[i];
        if (ca >= 'A' && ca <= 'Z') ca = (char)(ca + ('a' - 'A'));
        if (cb >= 'A' && cb <= 'Z') cb = (char)(cb + ('a' - 'A'));
        if (ca != cb) return 0;
    }
    return 1;
}

/* `TextDecoder`'s label resolution and decoding are `kiln-encoding`'s, behind the provider's
 * `kiln_text_decoder_label` and `kiln_text_decode` -- the same Rust the interpreter calls
 * (ADR 0060). `decode` selects the operation. A success is the answer as UTF-8 text; the two
 * error statuses spill the message to throw (3: a label that names no encoding, the
 * `RangeError`; 4: `fatal` and bytes that are not valid, the `TypeError`). */
static int kiln_c99_text_decoder_provider(kiln_value *out, int decode, const uint8_t *name,
                                          size_t name_len, const uint8_t *bytes, size_t bytes_len,
                                          uint32_t flags, kiln_value *thrown) {
    uint64_t capacity = decode ? (uint64_t)bytes_len * 3 + 64 : (uint64_t)name_len + 128;
    uint64_t len = 0;
    uint8_t *buffer = (uint8_t *)malloc((size_t)capacity + 1);
    int status;
    if (buffer == NULL) return KILN_C99_OUT_OF_MEMORY;
    status = decode ? kiln_text_decode(name, (uint64_t)name_len, bytes, (uint64_t)bytes_len, flags,
                                       buffer, capacity, &len)
                    : kiln_text_decoder_label(name, (uint64_t)name_len, buffer, capacity, &len);
    if (status == KILN_C99_INVALID_ARGUMENT && len > capacity) {
        uint8_t *bigger = (uint8_t *)realloc(buffer, (size_t)len + 1);
        if (bigger == NULL) { free(buffer); return KILN_C99_OUT_OF_MEMORY; }
        buffer = bigger;
        capacity = len;
        status = decode ? kiln_text_decode(name, (uint64_t)name_len, bytes, (uint64_t)bytes_len,
                                           flags, buffer, capacity, &len)
                        : kiln_text_decoder_label(name, (uint64_t)name_len, buffer, capacity, &len);
    }
    if (status == 3 || status == 4) {
        buffer[len] = 0;
        status = kiln_c99_throw_error(thrown,
                                      status == 3 ? KILN_C99_BUILTIN_RANGE_ERROR
                                                  : KILN_C99_BUILTIN_TYPE_ERROR,
                                      (const char *)buffer);
        free(buffer);
        return status;
    }
    if (status != KILN_C99_OK) { free(buffer); return status; }
    *out = kiln_c99_copy_text((const char *)buffer, (size_t)len);
    free(buffer);
    return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
}

/* A string value as owned UTF-8 (NULL and 0 for the empty string). */
static int kiln_c99_text_decoder_utf8(kiln_value value, uint8_t **owned, size_t *len) {
    kiln_value text = kiln_c99_text(value);
    *owned = NULL;
    *len = 0;
    if (text.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    return kiln_c99_string_to_utf8_lossy(text, owned, len);
}

/* `TextDecoderOptions.fatal` / `.ignoreBOM` of an options object, as `kiln_text_decode` flags. */
static int kiln_c99_text_decoder_flags(kiln_host *host, kiln_value options, int depth,
                                       kiln_value *thrown, uint32_t *flags) {
    static const struct { const char *key; size_t len; uint32_t flag; } options_table[2] = {
        {"fatal", 5, 1u}, {"ignoreBOM", 9, 2u}};
    size_t index;
    *flags = 0;
    if (options.tag != KILN_VALUE_OBJECT && options.tag != KILN_VALUE_FUNCTION &&
        options.tag != KILN_VALUE_ARRAY) return KILN_C99_OK;
    for (index = 0; index < 2; ++index) {
        kiln_value member;
        int status = kiln_c99_get(host, options, options_table[index].key, options_table[index].len,
                                  &member, depth, thrown);
        if (status != KILN_C99_OK) return status;
        if (kiln_c99_truthy(member)) *flags |= options_table[index].flag;
    }
    return KILN_C99_OK;
}

static int kiln_c99_new_text_decoder(kiln_host *host, kiln_value *out, const kiln_value *args,
                                     size_t argc, int depth, kiln_value *thrown) {
    kiln_value encoding;
    uint32_t flags = 0;
    uint8_t *label = NULL;
    size_t label_len = 5;
    int status;
    if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED) {
        status = kiln_c99_text_decoder_utf8(args[0], &label, &label_len);
        if (status != KILN_C99_OK) return status;
    }
    status = kiln_c99_text_decoder_provider(&encoding, 0,
                                            label != NULL ? label : (const uint8_t *)"utf-8",
                                            label != NULL ? label_len : 5, NULL, 0, 0, thrown);
    free(label);
    if (status != KILN_C99_OK) return status;
    if (argc > 1) {
        status = kiln_c99_text_decoder_flags(host, args[1], depth, thrown, &flags);
        if (status != KILN_C99_OK) return status;
    }
    status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "encoding", 8, encoding, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "fatal", 5, kiln_c99_boolean((flags & 1u) != 0), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "ignoreBOM", 9, kiln_c99_boolean((flags & 2u) != 0), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "decode", 6,
                                        kiln_c99_method(KILN_C99_METHOD_TEXT_DECODER_DECODE));
    if (status == KILN_C99_OK) {
        kiln_object *object = kiln_c99_object_of(*out);
        size_t index;
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
            object->shape = NULL;
        }
        for (index = 0; index < object->count; ++index) object->properties[index].enumerable = 0;
    }
    return status;
}

/* `TextDecoder.prototype.decode` over the bytes of `source`, which the caller has already
 * checked is a buffer or a view: the decoder's own `encoding`, `fatal` and `ignoreBOM` are what
 * the provider decodes with, exactly as the interpreter reads them off the receiver. */
static int kiln_c99_text_decoder_decode(kiln_host *host, kiln_value *result, kiln_value receiver,
                                        const uint8_t *bytes, size_t bytes_len, int depth,
                                        kiln_value *thrown) {
    kiln_value encoding;
    uint32_t flags = 0;
    uint8_t *name = NULL;
    size_t name_len = 0;
    int status = KILN_C99_OK;
    /* A receiver that is not an object has no attributes: the defaults, as in the interpreter. */
    if (receiver.tag == KILN_VALUE_OBJECT) {
        status = kiln_c99_get(host, receiver, "encoding", 8, &encoding, depth, thrown);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_text_decoder_utf8(encoding, &name, &name_len);
        if (status != KILN_C99_OK) return status;
    }
    status = kiln_c99_text_decoder_flags(host, receiver, depth, thrown, &flags);
    if (status == KILN_C99_OK)
        status = kiln_c99_text_decoder_provider(result, 1,
                                                name != NULL ? name : (const uint8_t *)"utf-8",
                                                name != NULL ? name_len : 5, bytes, bytes_len,
                                                flags, thrown);
    free(name);
    return status;
}

/* A platform exception, as node raises one: an `Error` named `name` whose prototype is the
 * realm's `DOMException.prototype`, so `instanceof DOMException` and its `code` hold. A realm
 * without `DOMException` gets the named `Error` alone. */
static int kiln_c99_dom_exception(kiln_host *host, kiln_value *out, const char *message,
                                  size_t message_len, const char *name, size_t name_len,
                                  int depth, kiln_value *thrown) {
    kiln_value constructor = kiln_c99_undefined();
    kiln_value prototype = kiln_c99_undefined();
    int found = 0;
    int status = kiln_c99_new_error(out, KILN_C99_BUILTIN_ERROR,
                                    kiln_c99_copy_text(message, message_len));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "name", 4, kiln_c99_copy_text(name, name_len));
    if (status == KILN_C99_OK)
        status = kiln_c99_global_binding(host, &constructor, "DOMException", 12, &found, depth,
                                         thrown);
    if (status != KILN_C99_OK || !found || constructor.tag == KILN_VALUE_UNDEFINED) return status;
    status = kiln_c99_get(host, constructor, "prototype", 9, &prototype, depth, thrown);
    if (status == KILN_C99_OK && prototype.tag == KILN_VALUE_OBJECT)
        status = kiln_c99_set_prototype(*out, prototype);
    return status;
}

static int kiln_c99_abort(kiln_host *host, kiln_value signal, kiln_value reason, int depth,
                          kiln_value *thrown) {
    kiln_value aborted;
    kiln_value listeners;
    kiln_value handler;
    kiln_value event;
    size_t index;
    int status;
    if (signal.tag != KILN_VALUE_OBJECT) return KILN_C99_OK;
    status = kiln_c99_get(host, signal, "aborted", 7, &aborted, depth, thrown);
    if (status != KILN_C99_OK || kiln_c99_truthy(aborted)) return status;
    if (reason.tag == KILN_VALUE_UNDEFINED) {
        status = kiln_c99_dom_exception(host, &reason, "This operation was aborted", 26,
                                        "AbortError", 10, depth, thrown);
        if (status != KILN_C99_OK) return status;
    }
    status = kiln_c99_put(NULL, signal, "aborted", 7, kiln_c99_boolean(1), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, signal, "reason", 6, reason, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_get(host, signal, "@@abort-listeners", 17, &listeners, depth, thrown);
    if (status == KILN_C99_OK)
        status = kiln_c99_get(host, signal, "onabort", 7, &handler, depth, thrown);
    if (status == KILN_C99_OK) status = kiln_c99_new_object(&event);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, event, "type", 4, kiln_c99_copy_text("abort", 5), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, event, "target", 6, signal, 0, NULL);
    if (status != KILN_C99_OK) return status;
    if (kiln_c99_is_callable(handler)) {
        kiln_value ignored;
        status = kiln_c99_call(host, &ignored, handler, signal, &event, 1, depth, thrown);
        if (status != KILN_C99_OK) return status;
    }
    if (listeners.tag != KILN_VALUE_ARRAY) return KILN_C99_OK;
    for (index = 0; index < kiln_c99_array_of(listeners)->count; ++index) {
        kiln_value ignored;
        kiln_value listener = kiln_val_to_boundary(kiln_c99_array_of(listeners)->elements[index]);
        status = kiln_c99_call(host, &ignored, listener, signal, &event, 1, depth, thrown);
        if (status != KILN_C99_OK) return status;
    }
    return KILN_C99_OK;
}
