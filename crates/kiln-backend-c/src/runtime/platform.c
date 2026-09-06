
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

static int kiln_c99_new_text_decoder(kiln_value *out, const kiln_value *args, size_t argc,
                                     kiln_value *thrown) {
    const char *label = "utf-8";
    size_t label_len = 5;
    if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED) {
        kiln_value s = kiln_c99_text(args[0]);
        if (s.tag == KILN_VALUE_STRING && s.ptr != NULL) {
            int ok = 0;
            if (s.len == 5 && kiln_c99_ascii_iequals(s.ptr, "utf-8", 5)) ok = 1;
            else if (s.len == 4 && kiln_c99_ascii_iequals(s.ptr, "utf8", 4)) ok = 1;
            else if (s.len == 16 && kiln_c99_ascii_iequals(s.ptr, "unicode-1-1-utf-8", 16)) ok = 1;
            if (!ok) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "The encoding label provided is invalid");
            }
        }
    }
    int status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "encoding", 8, kiln_c99_copy_text(label, label_len), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "fatal", 5, kiln_c99_boolean(0), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "ignoreBOM", 9, kiln_c99_boolean(0), 0, NULL);
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

static int kiln_c99_new_async_local_storage(kiln_value *out) {
    int status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "*store", 6, kiln_c99_undefined(), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "getStore", 8,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_ALS_GET_STORE));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "run", 3,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_ALS_RUN));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "enterWith", 9,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_ALS_ENTER_WITH));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "exit", 4,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_ALS_EXIT));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "disable", 7,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_ALS_DISABLE));
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
        status = kiln_c99_new_error(&reason, KILN_C99_BUILTIN_ERROR,
                                    kiln_c99_copy_text("This operation was aborted", 26));
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, reason, "name", 4,
                                  kiln_c99_copy_text("AbortError", 10), 0, NULL);
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
