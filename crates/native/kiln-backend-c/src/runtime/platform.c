
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

typedef struct {
    const void *brand;
    void *decoder;
} kiln_c99_text_decoder_owner_context;

static const char kiln_c99_text_decoder_owner_brand;

static int kiln_c99_text_decoder_visit(void *context, kiln_gc_visit_root_fn visit,
                                       void *visit_context) {
    (void)context;
    (void)visit;
    (void)visit_context;
    return 0;
}

static void kiln_c99_text_decoder_release(void *context) {
    kiln_c99_text_decoder_owner_context *owner =
        (kiln_c99_text_decoder_owner_context *)context;
    if (owner == NULL) return;
    kiln_text_decoder_state_free(owner->decoder);
    free(owner);
}

static int kiln_c99_text_decoder_state_provider(void **out_state, const uint8_t *label,
                                                size_t label_len, uint32_t flags,
                                                kiln_value *thrown) {
    uint64_t capacity = (uint64_t)label_len + 128;
    uint64_t len = 0;
    uint8_t *buffer = (uint8_t *)malloc((size_t)capacity + 1);
    int status;
    if (buffer == NULL) return KILN_C99_OUT_OF_MEMORY;
    status = kiln_text_decoder_state_create(label, (uint64_t)label_len, flags, out_state, buffer,
                                            capacity, &len);
    if (status == KILN_C99_INVALID_ARGUMENT && len > capacity) {
        uint8_t *bigger = (uint8_t *)realloc(buffer, (size_t)len + 1);
        if (bigger == NULL) { free(buffer); return KILN_C99_OUT_OF_MEMORY; }
        buffer = bigger;
        capacity = len;
        status = kiln_text_decoder_state_create(label, (uint64_t)label_len, flags, out_state,
                                               buffer, capacity, &len);
    }
    if (status == 3) {
        buffer[len] = 0;
        status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR, (const char *)buffer);
    }
    free(buffer);
    return status;
}

static int kiln_c99_text_decoder_state_decode_provider(kiln_value *out, void *state,
                                                       const uint8_t *bytes, size_t bytes_len,
                                                       int stream, kiln_value *thrown) {
    uint64_t capacity = (uint64_t)bytes_len * 3 + 128;
    uint64_t len = 0;
    uint8_t *buffer = (uint8_t *)malloc((size_t)capacity + 1);
    int status;
    if (buffer == NULL) return KILN_C99_OUT_OF_MEMORY;
    status = kiln_text_decoder_state_decode(state, bytes, (uint64_t)bytes_len, (uint32_t)stream,
                                            buffer, capacity, &len);
    if (status == KILN_C99_INVALID_ARGUMENT && len > capacity) {
        uint8_t *bigger = (uint8_t *)realloc(buffer, (size_t)len + 1);
        if (bigger == NULL) { free(buffer); return KILN_C99_OUT_OF_MEMORY; }
        buffer = bigger;
        capacity = len;
        status = kiln_text_decoder_state_decode(state, bytes, (uint64_t)bytes_len,
                                                (uint32_t)stream, buffer, capacity, &len);
    }
    if (status == 4) {
        buffer[len] = 0;
        status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, (const char *)buffer);
        free(buffer);
        return status;
    }
    if (status != KILN_C99_OK) { free(buffer); return status; }
    *out = kiln_c99_copy_text((const char *)buffer, (size_t)len);
    free(buffer);
    return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
}

void *kiln_c99_text_decoder_state(kiln_value receiver) {
    kiln_object *object;
    kiln_gc_external_owner *owner;
    kiln_c99_text_decoder_owner_context *context;
    if (receiver.tag != KILN_VALUE_OBJECT || receiver.bits == 0) return NULL;
    object = kiln_c99_object_of(receiver);
    if (object == NULL || object->external_owner == NULL) return NULL;
    owner = (kiln_gc_external_owner *)object->external_owner;
    if (kiln_c99_gc_payload_kind(owner) != KILN_GC_EXTERNAL_OWNER || owner->context == NULL ||
        owner->release_context != kiln_c99_text_decoder_release ||
        owner->visit_roots != kiln_c99_text_decoder_visit)
        return NULL;
    context = (kiln_c99_text_decoder_owner_context *)owner->context;
    return context->brand == &kiln_c99_text_decoder_owner_brand ? context->decoder : NULL;
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
    void *decoder_state = NULL;
    kiln_c99_text_decoder_owner_context *owner_context = NULL;
    void *external_owner;
    int status;
    if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED) {
        status = kiln_c99_text_decoder_utf8(args[0], &label, &label_len);
        if (status != KILN_C99_OK) return status;
    }
    status = kiln_c99_text_decoder_provider(&encoding, 0,
                                            label != NULL ? label : (const uint8_t *)"utf-8",
                                            label != NULL ? label_len : 5, NULL, 0, 0, thrown);
    if (status != KILN_C99_OK) { free(label); return status; }
    if (argc > 1) {
        status = kiln_c99_text_decoder_flags(host, args[1], depth, thrown, &flags);
        if (status != KILN_C99_OK) { free(label); return status; }
    }
    status = kiln_c99_text_decoder_state_provider(
        &decoder_state, label != NULL ? label : (const uint8_t *)"utf-8",
        label != NULL ? label_len : 5, flags, thrown);
    free(label);
    if (status != KILN_C99_OK) return status;
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
    if (status != KILN_C99_OK) {
        kiln_text_decoder_state_free(decoder_state);
        return status;
    }
    owner_context = (kiln_c99_text_decoder_owner_context *)malloc(sizeof *owner_context);
    if (owner_context == NULL) {
        kiln_text_decoder_state_free(decoder_state);
        return KILN_C99_OUT_OF_MEMORY;
    }
    owner_context->brand = &kiln_c99_text_decoder_owner_brand;
    owner_context->decoder = decoder_state;
    external_owner = kiln_c99_external_owner_create(owner_context, kiln_c99_text_decoder_visit,
                                                    kiln_c99_text_decoder_release);
    if (external_owner == NULL) {
        kiln_c99_text_decoder_release(owner_context);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_object_set_external_owner(*out, external_owner);
    return status;
}

/* `TextDecoder.prototype.decode` over the bytes of `source`, which the caller has already
 * checked is a buffer or a view: the decoder's own `encoding`, `fatal` and `ignoreBOM` are what
 * the provider decodes with, exactly as the interpreter reads them off the receiver. */
static int kiln_c99_text_decoder_decode(kiln_host *host, kiln_value *result, kiln_value receiver,
                                        const uint8_t *bytes, size_t bytes_len, int depth,
                                        const kiln_value *args, size_t argc, kiln_value *thrown) {
    void *decoder_state = kiln_c99_text_decoder_state(receiver);
    kiln_value options = argc > 1 ? args[1] : kiln_c99_undefined();
    kiln_value stream_value = kiln_c99_undefined();
    int stream = 0;
    int status = KILN_C99_OK;
    if (decoder_state == NULL) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "TextDecoder.decode called on an incompatible receiver");
    }
    if (options.tag == KILN_VALUE_OBJECT || options.tag == KILN_VALUE_FUNCTION ||
        options.tag == KILN_VALUE_ARRAY) {
        status = kiln_c99_get(host, options, "stream", 6, &stream_value, depth, thrown);
        if (status != KILN_C99_OK) return status;
        stream = kiln_c99_truthy(stream_value);
    }
    return kiln_c99_text_decoder_state_decode_provider(result, decoder_state, bytes, bytes_len,
                                                       stream, thrown);
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

/* What `setTimeout`, `setInterval` and `setImmediate` hand back for the timer `entry`, armed
 * under `id` (`code` says which): the record itself as a `Timeout` or `Immediate`
 * (`KILN_VALUE_TIMER`), or the bare identity where the realm's bootstrap never adopted the
 * prototypes (`kiln_c99_timer_op`, op 5) or the timer is an internal one (`__kilnQueueIo`). No
 * script runs and nothing else is allocated, which is what keeps arming a timer cheap. */
static int kiln_c99_timer_handle(kiln_timer *entry, uint64_t id, uint64_t code,
                                 kiln_value *result) {
    *result = kiln_c99_number((double)id);
    if (entry == NULL || code == KILN_C99_METHOD_QUEUE_IO_RAW ||
        !kiln_c99_realm()->timer_handles)
        return KILN_C99_OK;
    *result = kiln_c99_value(KILN_VALUE_TIMER, (uint64_t)(uintptr_t)entry, NULL, 0);
    return KILN_C99_OK;
}

#define KILN_C99_REALM_SLOT_TIMEOUT_PROTOTYPE UINT32_C(14)
#define KILN_C99_REALM_SLOT_IMMEDIATE_PROTOTYPE UINT32_C(15)

/* The store a timer handle keeps its own properties in, made on first use: an ordinary object
 * whose prototype is `Timeout.prototype` (or `Immediate.prototype`), so every member read, write
 * and `in` goes through the machinery arrays and promises already use for theirs. A timer nothing
 * touches never makes one. */
static int kiln_c99_timer_materialize(kiln_value target) {
    kiln_timer *timer = kiln_c99_timer_of(target);
    kiln_value *slot;
    int status;
    if (timer->properties.tag == KILN_VALUE_OBJECT) return KILN_C99_OK;
    status = kiln_c99_new_object_for_owner(&timer->properties, timer);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_realm_value_slot(timer->kind == KILN_C99_TIMER_IMMEDIATE
                                           ? KILN_C99_REALM_SLOT_IMMEDIATE_PROTOTYPE
                                           : KILN_C99_REALM_SLOT_TIMEOUT_PROTOTYPE,
                                       &slot);
    if (status == KILN_C99_OK && slot->tag == KILN_VALUE_OBJECT)
        status = kiln_c99_set_prototype(timer->properties, *slot);
    return status;
}

/* The identity a timer handle holds, when `handle` is one. */
static int kiln_c99_timer_handle_id(kiln_value handle, double *id, int *found) {
    *found = 0;
    if (handle.tag != KILN_VALUE_TIMER) return KILN_C99_OK;
    *id = (double)kiln_c99_timer_of(handle)->id;
    *found = 1;
    return KILN_C99_OK;
}

/* A timeout that already ran, armed again under its own identity: `refresh()` after the fact, or
 * from inside its own callback. The realm answers 1 when it did not take the work. */
static int kiln_c99_timer_rearm(kiln_timer *timer) {
    int status;
    if (timer->cleared || timer->ran == 0 || timer->kind != KILN_C99_TIMER_TIMEOUT)
        return KILN_C99_OK;
    status = kiln_realm_rearm_timeout(timer->id, timer->delay, kiln_c99_run_timer,
                                      kiln_c99_drop_timer, timer);
    if (status != 0) return status == 1 ? KILN_C99_OK : status;
    timer->owners += 1;
    timer->ran = 0;
    if (timer->unreferenced) return kiln_realm_set_timer_ref(timer->id, 0);
    return KILN_C99_OK;
}

/* What a timer handle's methods ask of the engine, `__kilnTimerRef(handle, op)`: `op` 0 its
 * identity, 1 whether it is referenced, 2 and 3 to reference or unreference it, 4 to restart it
 * from now, and 5 (`__kilnTimerRef(Timeout.prototype, 5, Immediate.prototype)`, once, from the
 * bootstrap) to adopt the prototypes handles are made with. The referenced flag the program sees
 * is on the record, so it survives the timer's end as node's does; the one the loop reads lives
 * in the realm's timer queue. */
static int kiln_c99_timer_op(kiln_value *result, const kiln_value *args, size_t argc) {
    kiln_value handle = argc > 0 ? args[0] : kiln_c99_undefined();
    double op = argc > 1 && args[1].tag == KILN_VALUE_NUMBER ? kiln_c99_as_number(args[1])
                                                             : 0.0 / 0.0;
    kiln_timer *timer;
    int status = KILN_C99_OK;
    *result = kiln_c99_undefined();
    if (op == 5.0) {
        kiln_value *slot;
        if (argc < 3 || args[0].tag != KILN_VALUE_OBJECT || args[2].tag != KILN_VALUE_OBJECT)
            return KILN_C99_INVALID_ARGUMENT;
        status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_TIMEOUT_PROTOTYPE, &slot);
        if (status != KILN_C99_OK) return status;
        *slot = args[0];
        status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_IMMEDIATE_PROTOTYPE, &slot);
        if (status != KILN_C99_OK) return status;
        *slot = args[2];
        kiln_c99_realm()->timer_handles = 1;
        return KILN_C99_OK;
    }
    if (handle.tag != KILN_VALUE_TIMER) {
        if (op != 0.0) *result = kiln_c99_boolean(1);
        return KILN_C99_OK;
    }
    timer = kiln_c99_timer_of(handle);
    if (op == 0.0) {
        *result = kiln_c99_number((double)timer->id);
    } else if (op == 2.0 || op == 3.0) {
        int unref = op == 3.0;
        if (unref != (timer->unreferenced != 0)) {
            timer->unreferenced = (uint8_t)unref;
            status = kiln_realm_set_timer_ref(timer->id, !unref);
        }
        *result = kiln_c99_boolean(!unref);
    } else if (op == 4.0) {
        if (!timer->cleared) {
            /* 1 when a pending timer was restarted in place; otherwise it already ran. */
            status = kiln_realm_refresh_timer(timer->id);
            if (status == 0) status = kiln_c99_timer_rearm(timer);
            else if (status == 1) status = KILN_C99_OK;
        }
    } else {
        *result = kiln_c99_boolean(!timer->unreferenced);
    }
    return status;
}

/* `clearTimeout(handle)`: the realm drops its entry, and the record drops its callback and
 * arguments, which is what makes a later `refresh()` do nothing. */
static int kiln_c99_timer_clear(kiln_value handle) {
    kiln_timer *timer = kiln_c99_timer_of(handle);
    int status = kiln_realm_clear_timer(timer->id);
    if (status == KILN_C99_OK) kiln_c99_timer_drop_state(timer);
    return status;
}

static int kiln_c99_abort(kiln_host *host, kiln_value signal, kiln_value reason, int depth,
                          kiln_value *thrown) {
    kiln_value aborted;
    kiln_value listeners;
    kiln_value snapshot;
    kiln_value handler;
    kiln_value event;
    size_t index;
    int single_listener;
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
    if (status == KILN_C99_OK) status = kiln_c99_new_array(&snapshot);
    if (status == KILN_C99_OK && listeners.tag == KILN_VALUE_ARRAY) {
        kiln_array *registered = kiln_c99_array_of(listeners);
        for (index = 0; status == KILN_C99_OK && index < registered->count; ++index) {
            status = kiln_c99_array_push(
                snapshot, kiln_val_to_boundary(registered->elements[index]));
        }
    }
    single_listener = status == KILN_C99_OK &&
                      (listeners.tag != KILN_VALUE_ARRAY ||
                       kiln_c99_array_of(listeners)->count <= 1);
    if (status == KILN_C99_OK) status = kiln_c99_new_object(&event);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, event, "type", 4, kiln_c99_copy_text("abort", 5), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, event, "target", 6, signal, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, event, "currentTarget", 12, signal, 0, NULL);
    if (status != KILN_C99_OK) return status;
    if (kiln_c99_is_callable(handler)) {
        kiln_value ignored;
        status = kiln_c99_call(host, &ignored, handler, signal, &event, 1, depth, thrown);
        if (status != KILN_C99_OK) return status;
    }
    if (listeners.tag != KILN_VALUE_ARRAY) return KILN_C99_OK;
    /* Node's multi-listener path dispatches registrations appended while a callback is running.
     * Preserve the single-listener fast path's fixed callback behavior, and retain stable records
     * so removal cannot shift the dispatch cursor or suppress an unrelated listener. */
    {
        kiln_array *registered = kiln_c99_array_of(listeners);
        kiln_array *pending = kiln_c99_array_of(snapshot);
        for (size_t at = 0; !single_listener && at < registered->count; ++at) {
            kiln_value record = kiln_val_to_boundary(registered->elements[at]);
            size_t seen;
            for (seen = 0; seen < pending->count; ++seen)
                if (kiln_c99_strict_equal(kiln_val_to_boundary(pending->elements[seen]), record))
                    break;
            if (seen == pending->count) {
                status = kiln_c99_array_push(snapshot, record);
                if (status != KILN_C99_OK) return status;
            }
        }
    }
    for (index = 0; index < kiln_c99_array_of(snapshot)->count; ++index) {
        kiln_value ignored;
        kiln_value record = kiln_val_to_boundary(kiln_c99_array_of(snapshot)->elements[index]);
        kiln_value removed, listener, once;
        status = kiln_c99_get(host, record, "removed", 7, &removed, depth, thrown);
        if (status == KILN_C99_OK)
            status = kiln_c99_get(host, record, "listener", 8, &listener, depth, thrown);
        if (status == KILN_C99_OK)
            status = kiln_c99_get(host, record, "once", 4, &once, depth, thrown);
        if (status != KILN_C99_OK) return status;
        if (kiln_c99_truthy(removed)) continue;
        if (kiln_c99_truthy(once)) {
            status = kiln_c99_put(NULL, record, "removed", 7, kiln_c99_boolean(1), 0, NULL);
            if (status != KILN_C99_OK) return status;
            kiln_array *registered = kiln_c99_array_of(listeners);
            size_t write = 0;
            for (size_t at = 0; at < registered->count; ++at) {
                if (kiln_c99_strict_equal(kiln_val_to_boundary(registered->elements[at]), record))
                    continue;
                if (write != at)
                    kiln_c99_heap_store_val(registered, &registered->elements[write],
                                            registered->elements[at]);
                ++write;
            }
            registered->count = write;
        }
        status = kiln_c99_call(host, &ignored, listener, signal, &event, 1, depth, thrown);
        if (status != KILN_C99_OK) return status;
        {
            kiln_array *registered = kiln_c99_array_of(listeners);
            kiln_array *pending = kiln_c99_array_of(snapshot);
            for (size_t at = 0; !single_listener && at < registered->count; ++at) {
                kiln_value candidate = kiln_val_to_boundary(registered->elements[at]);
                size_t seen;
                for (seen = 0; seen < pending->count; ++seen)
                    if (kiln_c99_strict_equal(
                            kiln_val_to_boundary(pending->elements[seen]), candidate))
                        break;
                if (seen == pending->count) {
                    status = kiln_c99_array_push(snapshot, candidate);
                    if (status != KILN_C99_OK) return status;
                }
            }
        }
    }
    return KILN_C99_OK;
}

/* CompressionStream owns one incremental flate2 codec. The opaque state is attached to the
 * public compressor object through the collector's external-owner slot, so a stream's codec dies
 * with the stream and remains alive during a collection whenever the JS object is reachable. */
typedef struct {
    const void *brand;
    void *codec;
} kiln_c99_compression_owner_context;

static const char kiln_c99_compression_owner_brand;

static int kiln_c99_compression_visit(void *context, kiln_gc_visit_root_fn visit,
                                      void *visit_context) {
    (void)context;
    (void)visit;
    (void)visit_context;
    return 0;
}

static void kiln_c99_compression_release(void *context) {
    kiln_c99_compression_owner_context *owner = (kiln_c99_compression_owner_context *)context;
    if (owner == NULL) return;
    kiln_compression_state_free(owner->codec);
    free(owner);
}

static void *kiln_c99_compression_codec(kiln_value receiver) {
    kiln_object *object;
    kiln_gc_external_owner *external;
    kiln_c99_compression_owner_context *context;
    if (receiver.tag != KILN_VALUE_OBJECT || (object = kiln_c99_object_of(receiver)) == NULL ||
        object->external_owner == NULL) return NULL;
    external = (kiln_gc_external_owner *)object->external_owner;
    if (kiln_c99_gc_payload_kind(external) != KILN_GC_EXTERNAL_OWNER ||
        external->release_context != kiln_c99_compression_release ||
        external->visit_roots != kiln_c99_compression_visit) return NULL;
    context = (kiln_c99_compression_owner_context *)external->context;
    return context->brand == &kiln_c99_compression_owner_brand ? context->codec : NULL;
}

static int kiln_c99_compression_bytes(kiln_value value, const uint8_t **bytes,
                                      size_t *length, kiln_value *thrown) {
    kiln_typed_array *view;
    if (value.tag != KILN_VALUE_TYPED_ARRAY)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "CompressionStream chunks must be Uint8Array values");
    view = kiln_c99_typed_array_of(value);
    if (view == NULL || view->kind != KILN_TYPED_UINT8 || view->buffer == NULL ||
        view->buffer->detached)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "CompressionStream chunks must be live Uint8Array values");
    *length = view->length;
    *bytes = view->length == 0 ? NULL : view->buffer->bytes + view->byte_offset;
    return KILN_C99_OK;
}

int kiln_c99_compression_primitive(kiln_value *out, const kiln_value *args, size_t argc,
                                   kiln_value *thrown) {
    double operation, format, direction;
    void *codec;
    if (out == NULL || args == NULL || argc < 2 || args[1].tag != KILN_VALUE_NUMBER)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Invalid CompressionStream operation");
    operation = kiln_c99_as_number(args[1]);
    if (operation == 0) {
        kiln_c99_compression_owner_context *context;
        void *owner;
        int status;
        if (args[0].tag != KILN_VALUE_OBJECT || argc < 4 || args[2].tag != KILN_VALUE_NUMBER ||
            args[3].tag != KILN_VALUE_NUMBER)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Invalid CompressionStream constructor");
        format = kiln_c99_as_number(args[2]);
        direction = kiln_c99_as_number(args[3]);
        if (format < 1 || format > 3 || floor(format) != format || direction < 1 || direction > 2 ||
            floor(direction) != direction)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Unsupported compression format");
        codec = kiln_compression_state_create((uint32_t)format, (uint32_t)direction);
        if (codec == NULL) return KILN_C99_OUT_OF_MEMORY;
        context = (kiln_c99_compression_owner_context *)malloc(sizeof *context);
        if (context == NULL) { kiln_compression_state_free(codec); return KILN_C99_OUT_OF_MEMORY; }
        context->brand = &kiln_c99_compression_owner_brand;
        context->codec = codec;
        owner = kiln_c99_external_owner_create(context, kiln_c99_compression_visit,
                                               kiln_c99_compression_release);
        if (owner == NULL) { kiln_c99_compression_release(context); return KILN_C99_OUT_OF_MEMORY; }
        status = kiln_c99_object_set_external_owner(args[0], owner);
        if (status != KILN_C99_OK) { kiln_c99_external_owner_detach(owner); return status; }
        *out = (kiln_value){.tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0};
        return KILN_C99_OK;
    }
    if (operation != 1 || argc < 4 || args[3].tag != KILN_VALUE_BOOLEAN)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Invalid CompressionStream operation");
    codec = kiln_c99_compression_codec(args[0]);
    if (codec == NULL)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Invalid CompressionStream receiver");
    {
        const uint8_t *input;
        size_t input_length, output_length = 0;
        uint8_t *output = NULL;
        kiln_typed_array *view;
        int status = kiln_c99_compression_bytes(args[2], &input, &input_length, thrown);
        if (status != KILN_C99_OK) return status;
        status = kiln_compression_state_push(codec, input, input_length,
                                             args[3].bits ? 1u : 0u, &output,
                                             &output_length);
        if (status != 0) return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                     "Invalid compressed data");
        status = kiln_c99_webcrypto_output_array(out, output_length);
        if (status == KILN_C99_OK) {
            view = kiln_c99_typed_array_of(*out);
            if (view == NULL || view->buffer == NULL) status = KILN_C99_OUT_OF_MEMORY;
            else if (output_length != 0) memcpy(view->buffer->bytes + view->byte_offset, output,
                                                output_length);
        }
        kiln_compression_output_free(output, output_length);
        return status;
    }
}
