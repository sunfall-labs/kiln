
/* A compact native EventEmitter core shared by node:events and the socket/server facades.
 *
 * Listener state is an unspellable, non-enumerable own property. Keeping the state in ordinary
 * backend values makes emit synchronous, preserves registration order, and lets every future
 * I/O facade use the same callback semantics without exposing backend object layout across the
 * generated-module boundary. */
static int kiln_c99_install_event_emitter_methods(kiln_value target) {
    static const struct { const char *name; size_t length; uint64_t code; } methods[] = {
        {"on", 2, KILN_C99_METHOD_NODE_EVENT_ON},
        {"addListener", 11, KILN_C99_METHOD_NODE_EVENT_ON},
        {"once", 4, KILN_C99_METHOD_NODE_EVENT_ONCE},
        {"emit", 4, KILN_C99_METHOD_NODE_EVENT_EMIT},
        {"listeners", 9, KILN_C99_METHOD_NODE_EVENT_LISTENERS},
        {"listenerCount", 13, KILN_C99_METHOD_NODE_EVENT_LISTENER_COUNT},
        {"removeListener", 14, KILN_C99_METHOD_NODE_EVENT_REMOVE_LISTENER},
        {"off", 3, KILN_C99_METHOD_NODE_EVENT_REMOVE_LISTENER},
        {"removeAllListeners", 18, KILN_C99_METHOD_NODE_EVENT_REMOVE_ALL_LISTENERS},
        {"setMaxListeners", 15, KILN_C99_METHOD_NODE_EVENT_SET_MAX_LISTENERS},
        {"eventNames", 10, KILN_C99_METHOD_NODE_EVENT_EVENT_NAMES},
    };
    size_t index;
    int status = KILN_C99_OK;
    for (index = 0; status == KILN_C99_OK && index < sizeof methods / sizeof *methods; ++index) {
        status = kiln_c99_define_method(target, methods[index].name, methods[index].length,
                                        kiln_c99_method(methods[index].code));
    }
    return status;
}

static int kiln_c99_install_readable_methods(kiln_value target) {
    static const struct { const char *name; size_t length; uint64_t code; } methods[] = {
        {"push", 4, KILN_C99_METHOD_NODE_READABLE_PUSH},
        {"read", 4, KILN_C99_METHOD_NODE_READABLE_READ},
        {"destroy", 7, KILN_C99_METHOD_NODE_READABLE_DESTROY},
        {"pause", 5, KILN_C99_METHOD_NODE_READABLE_PAUSE},
        {"resume", 6, KILN_C99_METHOD_NODE_READABLE_RESUME},
        {"setEncoding", 11, KILN_C99_METHOD_NODE_READABLE_SET_ENCODING},
        {"pipe", 4, KILN_C99_METHOD_NODE_READABLE_PIPE},
        {"unpipe", 6, KILN_C99_METHOD_NODE_READABLE_UNPIPE},
    };
    size_t index;
    int status = KILN_C99_OK;
    for (index = 0; status == KILN_C99_OK && index < sizeof methods / sizeof *methods; ++index) {
        status = kiln_c99_define_method(target, methods[index].name, methods[index].length,
                                        kiln_c99_method(methods[index].code));
    }
    return status;
}

static int kiln_c99_install_writable_methods(kiln_value target) {
    static const struct { const char *name; size_t length; uint64_t code; } methods[] = {
        {"write", 5, KILN_C99_METHOD_NODE_WRITABLE_WRITE},
        {"end", 3, KILN_C99_METHOD_NODE_WRITABLE_END},
        {"destroy", 7, KILN_C99_METHOD_NODE_WRITABLE_DESTROY},
    };
    size_t index;
    int status = KILN_C99_OK;
    for (index = 0; status == KILN_C99_OK && index < sizeof methods / sizeof *methods; ++index) {
        status = kiln_c99_define_method(target, methods[index].name, methods[index].length,
                                        kiln_c99_method(methods[index].code));
    }
    return status;
}

static int kiln_c99_install_server_response_methods(kiln_value target) {
    static const struct { const char *name; size_t length; uint64_t code; } methods[] = {
        {"setHeader", 9, KILN_C99_METHOD_NODE_RESPONSE_SET_HEADER},
        {"getHeader", 9, KILN_C99_METHOD_NODE_RESPONSE_GET_HEADER},
        {"getHeaders", 10, KILN_C99_METHOD_NODE_RESPONSE_GET_HEADERS},
        {"hasHeader", 9, KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER},
        {"removeHeader", 12, KILN_C99_METHOD_NODE_RESPONSE_REMOVE_HEADER},
        {"writeHead", 9, KILN_C99_METHOD_NODE_RESPONSE_WRITE_HEAD},
        {"write", 5, KILN_C99_METHOD_NODE_RESPONSE_WRITE},
        {"end", 3, KILN_C99_METHOD_NODE_RESPONSE_END},
    };
    size_t index;
    int status = KILN_C99_OK;
    for (index = 0; status == KILN_C99_OK && index < sizeof methods / sizeof *methods; ++index) {
        status = kiln_c99_define_method(target, methods[index].name, methods[index].length,
                                        kiln_c99_method(methods[index].code));
    }
    if (status == KILN_C99_OK)
        status = kiln_c99_own_data_hot_key(target, KILN_C99_HOT_KEY_STATUS_CODE, kiln_c99_number(200.0));
    if (status == KILN_C99_OK)
        status = kiln_c99_own_data(target, "headersSent", 11, kiln_c99_boolean(0));
    if (status == KILN_C99_OK)
        status = kiln_c99_own_data(target, "writableEnded", 13, kiln_c99_boolean(0));
    if (status == KILN_C99_OK)
        status = kiln_c99_own_data_hot_key(target, KILN_C99_HOT_KEY_DESTROYED, kiln_c99_boolean(0));
    return status;
}

static const char kiln_c99_http_version_11[] = "1.1";
static const char kiln_c99_http_version_10[] = "1.0";

/* The prototype a `node:http` request links to.
 *
 * `req` and `res` are built from nothing on every request, and installing their methods on each
 * *instance* was the largest single cost in the object model on a keep-alive server: seventeen
 * or more `kiln_c99_define_method` calls, each a chain walk, a linear scan and an insert, all
 * producing the same method values every time. One shared prototype per realm replaces them, and
 * the lookup that finds them was already a chain walk.
 *
 * Nothing about listeners moves: `kiln_c99_node_event` keeps its state in an *own*
 * `@@node:events` property of whichever receiver the method was called on, so buckets land on
 * the instance and two requests never share listeners. Nor does shadowing: a method on a
 * prototype is writable, so `req.on = f` still creates an own property that wins.
 *
 * There is no registered `IncomingMessage` constructor to hang this on, so it is a realm value
 * slot. The response has one -- see `kiln_c99_callable_prototype` -- and uses it. */
static int kiln_c99_node_incoming_message_prototype(kiln_value *out) {
    kiln_value *slot = NULL;
    int status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_INCOMING_MESSAGE, &slot);
    if (status != KILN_C99_OK) return status;
    if (slot == NULL) KILN_C99_REFUSE("the realm has no IncomingMessage prototype slot");
    if (slot->tag == KILN_VALUE_OBJECT) {
        *out = *slot;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_object(slot);
    if (status == KILN_C99_OK) status = kiln_c99_install_event_emitter_methods(*slot);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(
            *slot, "setEncoding", 11,
            kiln_c99_method(KILN_C99_METHOD_NODE_READABLE_SET_ENCODING));
    if (status == KILN_C99_OK)
        status = kiln_c99_own_data_hot_key(*slot, KILN_C99_HOT_KEY_HTTP_VERSION,
                                           kiln_c99_static_text(kiln_c99_http_version_11, 3));
    if (status == KILN_C99_OK)
        status = kiln_c99_own_data_hot_key(*slot, KILN_C99_HOT_KEY_HTTP_VERSION_MAJOR, kiln_c99_number(1.0));
    if (status == KILN_C99_OK)
        status = kiln_c99_own_data_hot_key(*slot, KILN_C99_HOT_KEY_HTTP_VERSION_MINOR, kiln_c99_number(1.0));
    if (status == KILN_C99_OK)
        status = kiln_c99_own_data_hot_key(*slot, KILN_C99_HOT_KEY_ABORTED, kiln_c99_boolean(0));
    if (status == KILN_C99_OK)
        status = kiln_c99_own_data_hot_key(*slot, KILN_C99_HOT_KEY_COMPLETE, kiln_c99_boolean(0));
    if (status == KILN_C99_OK)
        status = kiln_c99_own_data_hot_key(*slot, KILN_C99_HOT_KEY_DESTROYED, kiln_c99_boolean(0));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_accessor(
            *slot, "headers", 7,
            kiln_c99_method(KILN_C99_METHOD_NODE_REQUEST_HEADERS_GET),
            kiln_c99_method(KILN_C99_METHOD_NODE_REQUEST_HEADERS_SET), 1);
    if (status != KILN_C99_OK) {
        /* A half-built prototype must not be handed out or cached: leave the slot empty so the
         * next request builds it again rather than inheriting a surface with holes in it. */
        *slot = kiln_c99_undefined();
        return status;
    }
    *out = *slot;
    return KILN_C99_OK;
}

static int kiln_c99_node_event(kiln_host *host, kiln_value *result, uint64_t code,
                               kiln_value receiver, const kiln_value *args, size_t argc,
                               int depth, kiln_value *thrown) {
    kiln_value table;
    kiln_value name;
    kiln_value bucket;
    kiln_array *entries;
    size_t index;
    int status;
    if (code == KILN_C99_METHOD_NODE_EVENT_EMITTER) {
        if (receiver.tag == KILN_VALUE_OBJECT) {
            *result = receiver;
        } else {
            status = kiln_c99_new_object(result);
            if (status != KILN_C99_OK) return status;
        }
        return kiln_c99_install_event_emitter_methods(*result);
    }
    if (receiver.tag != KILN_VALUE_OBJECT) {
        if (code == KILN_C99_METHOD_NODE_EVENT_LISTENER_COUNT && argc > 0 && args[0].tag == KILN_VALUE_OBJECT) {
            kiln_value emitter = args[0];
            const kiln_value *rest_args = argc > 1 ? args + 1 : NULL;
            size_t rest_argc = argc > 1 ? argc - 1 : 0;
            return kiln_c99_node_event(host, result, code, emitter, rest_args, rest_argc, depth, thrown);
        }
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "EventEmitter method called on an incompatible receiver");
    }
    {
        kiln_property *state = kiln_c99_own_private(receiver, "@@node:events", 13);
        /* No listener has ever been registered on this receiver -- the common case for `finish`
         * and `close` on a response nothing is listening for. `on`/`once` are the only codes
         * that need somewhere to put a listener; every other code has a fixed answer for "no
         * listeners", the same one the bucket-not-found path below gives once a private array
         * does exist and simply has nothing under this name. Answering it here skips building
         * that array -- and the private property that would otherwise hold it -- for a call that
         * was never going to find anything in it. */
        if ((state == NULL || state->value.tag != KILN_VALUE_ARRAY) &&
            code != KILN_C99_METHOD_NODE_EVENT_ON && code != KILN_C99_METHOD_NODE_EVENT_ONCE) {
            if (code == KILN_C99_METHOD_NODE_EVENT_EMIT) *result = kiln_c99_boolean(0);
            else if (code == KILN_C99_METHOD_NODE_EVENT_LISTENERS ||
                     code == KILN_C99_METHOD_NODE_EVENT_EVENT_NAMES)
                return kiln_c99_new_array(result);
            else if (code == KILN_C99_METHOD_NODE_EVENT_LISTENER_COUNT) {
                *result = kiln_c99_number(0.0);
                return KILN_C99_OK;
            }
            else *result = receiver;
            return KILN_C99_OK;
        }
        if (state == NULL) {
            kiln_object *object = kiln_c99_object_of(receiver);
            status = kiln_c99_put(NULL, receiver, "@@node:events", 13,
                                  kiln_c99_undefined(), 0, NULL);
            if (status != KILN_C99_OK) return status;
            state = kiln_c99_own_private(receiver, "@@node:events", 13);
            if (state == NULL)
                KILN_C99_REFUSE("an event bucket store that could not be created on this receiver");
            state->enumerable = 0;
            (void)object;
        }
        table = state->value;
        if (table.tag != KILN_VALUE_ARRAY) {
            status = kiln_c99_new_array(&table);
            if (status != KILN_C99_OK) return status;
            state->value = table;
        }
    }
    entries = kiln_c99_array_of(table);
    /* `eventNames()`: every bucket that still holds a listener, in registration order. */
    if (code == KILN_C99_METHOD_NODE_EVENT_EVENT_NAMES) {
        status = kiln_c99_new_array(result);
        for (index = 0; status == KILN_C99_OK && index < entries->count; ++index) {
            kiln_value entry = kiln_val_to_boundary(entries->elements[index]);
            kiln_value listeners;
            kiln_value found;
            if (entry.tag != KILN_VALUE_OBJECT) continue;
            status = kiln_c99_get(NULL, entry, "listeners", 9, &listeners, 0, NULL);
            if (status != KILN_C99_OK) return status;
            if (listeners.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(listeners)->count == 0) continue;
            status = kiln_c99_get(NULL, entry, "name", 4, &found, 0, NULL);
            if (status == KILN_C99_OK) status = kiln_c99_array_push(*result, found);
        }
        return status;
    }
    name = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
    bucket = kiln_c99_undefined();
    for (index = 0; index < entries->count; ++index) {
        kiln_value found;
        kiln_value entry = kiln_val_to_boundary(entries->elements[index]);
        if (entry.tag != KILN_VALUE_OBJECT) continue;
        status = kiln_c99_get(NULL, entry, "name", 4, &found, 0, NULL);
        if (status < KILN_C99_OK) return status;
        if (kiln_c99_strict_equal(found, name)) {
            bucket = entry;
            break;
        }
    }
    if (code == KILN_C99_METHOD_NODE_EVENT_ON || code == KILN_C99_METHOD_NODE_EVENT_ONCE) {
        kiln_value listener = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_value listeners;
        kiln_value record;
        if (!kiln_c99_is_callable(listener))
            return kiln_c99_throw_not_a(thrown, listener, "function");
        if (bucket.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&bucket);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, bucket, "name", 4, name, 0, NULL);
            if (status == KILN_C99_OK) status = kiln_c99_new_array(&listeners);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, bucket, "listeners", 9, listeners, 0, NULL);
            if (status == KILN_C99_OK) status = kiln_c99_array_push(table, bucket);
            if (status != KILN_C99_OK) return status;
        } else {
            status = kiln_c99_get(NULL, bucket, "listeners", 9, &listeners, 0, NULL);
            if (status != KILN_C99_OK || listeners.tag != KILN_VALUE_ARRAY)
                KILN_C99_REFUSE("an event bucket whose listeners are not an array");
        }
        status = kiln_c99_new_object(&record);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, record, "callback", 8, listener, 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, record, "once", 4,
                                  kiln_c99_boolean(code == KILN_C99_METHOD_NODE_EVENT_ONCE), 0,
                                  NULL);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(listeners, record);
        if (status != KILN_C99_OK) return status;
        {
            kiln_value fd_val;
            if (kiln_c99_get(NULL, receiver, "fd", 2, &fd_val, 0, NULL) == KILN_C99_OK &&
                fd_val.tag == KILN_VALUE_NUMBER && kiln_c99_as_number(fd_val) == 0.0) {
                if (kiln_c99_strict_equal(name, kiln_c99_copy_text("data", 4))) {
                    (void)kiln_c99_node_stdin_resume(host, receiver);
                }
            }
        }
        *result = receiver;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_EVENT_SET_MAX_LISTENERS) {
        *result = receiver;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_EVENT_REMOVE_ALL_LISTENERS) {
        if (argc == 0) {
            kiln_c99_array_of(table)->count = 0;
        } else if (bucket.tag == KILN_VALUE_OBJECT) {
            kiln_value listeners;
            status = kiln_c99_get(NULL, bucket, "listeners", 9, &listeners, 0, NULL);
            if (status != KILN_C99_OK || listeners.tag != KILN_VALUE_ARRAY)
                KILN_C99_REFUSE("an event bucket whose listeners are not an array");
            kiln_c99_array_of(listeners)->count = 0;
        }
        *result = receiver;
        return KILN_C99_OK;
    }
    if (bucket.tag != KILN_VALUE_OBJECT) {
        if (code == KILN_C99_METHOD_NODE_EVENT_EMIT) *result = kiln_c99_boolean(0);
        else if (code == KILN_C99_METHOD_NODE_EVENT_LISTENERS) return kiln_c99_new_array(result);
        else if (code == KILN_C99_METHOD_NODE_EVENT_LISTENER_COUNT) {
            *result = kiln_c99_number(0.0);
            return KILN_C99_OK;
        }
        else *result = receiver;
        return KILN_C99_OK;
    }
    {
        kiln_value listeners;
        kiln_array *stored;
        status = kiln_c99_get(NULL, bucket, "listeners", 9, &listeners, 0, NULL);
        if (status != KILN_C99_OK || listeners.tag != KILN_VALUE_ARRAY)
            KILN_C99_REFUSE("an event bucket whose listeners are not an array");
        stored = kiln_c99_array_of(listeners);
        if (code == KILN_C99_METHOD_NODE_EVENT_LISTENERS) {
            status = kiln_c99_new_array(result);
            for (index = 0; status == KILN_C99_OK && index < stored->count; ++index) {
                kiln_value callback;
                status = kiln_c99_get(NULL, kiln_val_to_boundary(stored->elements[index]), "callback", 8, &callback, 0,
                                      NULL);
                if (status == KILN_C99_OK) status = kiln_c99_array_push(*result, callback);
            }
            return status;
        }
        if (code == KILN_C99_METHOD_NODE_EVENT_LISTENER_COUNT) {
            *result = kiln_c99_number((double)stored->count);
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_NODE_EVENT_REMOVE_LISTENER) {
            kiln_value wanted = argc > 1 ? args[1] : kiln_c99_undefined();
            if (!kiln_c99_is_callable(wanted))
                return kiln_c99_throw_not_a(thrown, wanted, "function");
            for (index = stored->count; index > 0; --index) {
                kiln_value callback;
                status = kiln_c99_get(NULL, kiln_val_to_boundary(stored->elements[index - 1]), "callback", 8, &callback,
                                      0, NULL);
                if (status != KILN_C99_OK) return status;
                if (kiln_c99_strict_equal(callback, wanted)) {
                    memmove(stored->elements + index - 1, stored->elements + index,
                            (stored->count - index) * sizeof *stored->elements);
                    --stored->count;
                    break;
                }
            }
            *result = receiver;
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_NODE_EVENT_EMIT) {
            kiln_value snapshot;
            kiln_array *copy;
            status = kiln_c99_new_array(&snapshot);
            for (index = 0; status == KILN_C99_OK && index < stored->count; ++index)
                status = kiln_c99_array_push(snapshot, kiln_val_to_boundary(stored->elements[index]));
            if (status != KILN_C99_OK) return status;
            copy = kiln_c99_array_of(snapshot);
            for (index = 0; index < copy->count; ++index) {
                kiln_value callback;
                kiln_value once;
                kiln_value ignored;
                kiln_value copy_elem = kiln_val_to_boundary(copy->elements[index]);
                status = kiln_c99_get(NULL, copy_elem, "callback", 8, &callback, 0,
                                      NULL);
                if (status == KILN_C99_OK)
                    status = kiln_c99_get(NULL, copy_elem, "once", 4, &once, 0, NULL);
                if (status != KILN_C99_OK) return status;
                if (kiln_c99_truthy(once)) {
                    size_t live;
                    for (live = 0; live < stored->count; ++live) {
                        if (stored->elements[live] == copy->elements[index]) {
                            memmove(stored->elements + live, stored->elements + live + 1,
                                    (stored->count - live - 1) * sizeof *stored->elements);
                            --stored->count;
                            break;
                        }
                    }
                }
                status = kiln_c99_call(host, &ignored, callback, receiver,
                                       argc > 1 ? args + 1 : NULL, argc > 1 ? argc - 1 : 0,
                                       depth, thrown);
                if (status != KILN_C99_OK) return status;
            }
            *result = kiln_c99_boolean(copy->count != 0);
            return KILN_C99_OK;
        }
    }
    KILN_C99_REFUSE("an EventEmitter method this backend has not implemented");
}


static int kiln_c99_node_set_hidden_pointer(kiln_value object, const char *key, size_t key_len,
                                            void *pointer) {
    kiln_property *slot;
    int status = kiln_c99_own_slot(object, key, key_len, &slot);
    if (status != KILN_C99_OK) return status;
    /* A raw host pointer smuggled through a private slot. It used to be told apart from a real
     * object value by `ptr == NULL` while the pointer itself sat in `bits` -- a scheme that
     * needed the two to be separate fields. They now share one word, so the marker moves to
     * `len`, which no object value uses. */
    slot->value = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)pointer, NULL,
                                 0xFFFFFFFFu);
    slot->enumerable = 0;
    slot->writable = 0;
    slot->configurable = 0;
    kiln_c99_object_of(object)->read_only_seen = 1;
    /* Reserved `@@node:*` machinery slots are never routed through `define_accessor`, so this is
     * always already 0 -- guarded anyway, since the cost of asking is one branch off a path that
     * runs once per socket or request rather than per property access. */
    if (slot->accessor) {
        kiln_c99_object_of(object)->accessor_count -= 1;
    }
    slot->accessor = 0;
    return KILN_C99_OK;
}

static int kiln_c99_is_node_hidden_key(const char *key, size_t key_len) {
    static const char *const names[] = {
        "@@node:socket", "@@node:server", "@@node:client", "@@node:response",
        "@@kiln:node:is-file", "@@kiln:node:is-directory",
        "@@kiln:node:is-symlink",
    };
    size_t index;
    /* Every reserved name is `@@`-prefixed and nearly every key a program reads is not, so the
     * read that reaches here on every property access answers in two byte compares. */
    if (key_len < 2 || key[0] != '@' || key[1] != '@') {
        return 0;
    }
    for (index = 0; index < sizeof names / sizeof names[0]; ++index) {
        size_t length = strlen(names[index]);
        if (key_len == length && memcmp(key, names[index], length) == 0) {
            return 1;
        }
    }
    return 0;
}

static void *kiln_c99_node_hidden_pointer(kiln_value object, const char *key, size_t key_len) {
    kiln_property *slot = kiln_c99_own_private(object, key, key_len);
    void *pointer;
    if (slot == NULL || slot->value.tag != KILN_VALUE_OBJECT) return NULL;
    pointer = (void *)(uintptr_t)slot->value.bits;
    if (slot->value.len == 0xFFFFFFFFu) return pointer;
    /* A layout-built request or response keeps its pointer in a shaped slot as a plain
     * object-tagged word, and moving that object to dictionary mode leaves exactly that here
     * (see the layouts below). Accepted only when the block really is a response. */
    if (pointer != NULL && kiln_c99_gc_kind_of_ptr(pointer) == KILN_GC_NODE_RESPONSE)
        return pointer;
    return NULL;
}

static int kiln_c99_ascii_case_equal(const char *a, size_t a_len, const char *b, size_t b_len) {
    size_t i;
    if (a_len != b_len) return 0;
    for (i = 0; i < a_len; ++i) {
        unsigned char ca = (unsigned char)a[i];
        unsigned char cb = (unsigned char)b[i];
        if (ca >= 'A' && ca <= 'Z') ca += 'a' - 'A';
        if (cb >= 'A' && cb <= 'Z') cb += 'a' - 'A';
        if (ca != cb) return 0;
    }
    return 1;
}

/* ---------------------------------------------------------------------------------------------
 * The `node:http` request and response a server hands its listener, built from fixed layouts.
 *
 * Every request builds the same two objects: an `IncomingMessage` with the response's hidden
 * pointer, `method`, `url`, `socket`, `connection` and `complete`, and a `ServerResponse` with
 * the pointer, `req`, `socket`, `connection`, `statusCode`, `headersSent` and `writableEnded`.
 * Building them a property at a time interned and hashed every name, walked the prototype chain
 * for a setter before every write, grew a property list by doubling, and left both objects in
 * dictionary mode, where no call site could cache `res.end` or `req.url` -- per request, for
 * answers that never change. Here the keys are pinned once per heap (ADR 0047's "pre-interned
 * static per prelude literal key"), each layout is one shape chain built once per heap
 * (ADR 0050), and an instance is a fresh object pointed at the chain's last shape with its slots
 * stored by index: no hashing, no transition search, no property list, no `malloc`.
 *
 * The objects are fresh every request. An earlier revision recycled one pair per keep-alive
 * socket and reset it with nine `delete`s; that was both the costliest step in the path and
 * observably wrong -- a request a program kept (a log, a queue, a closure) changed its `url`
 * under it when the next request arrived, and own properties a handler added (`req.user`,
 * `res.locals`) leaked into the next request on the same socket.
 *
 * The chains hang off the root shape's *parent* link only: the root's own transition table is
 * bounded (32 edges, ADR 0050) and a program exhausts it long before its first request, so these
 * are not registered there. A handler that adds a member extends the chain the ordinary way.
 *
 * The hidden pointer sits in slot 0 as an object-tagged word, which is what keeps the response
 * block alive while a program holds either object (the tracer marks it). A `delete` or an
 * attribute change moves an object to dictionary mode and turns that slot into an ordinary
 * property holding the same pointer; `kiln_c99_node_response_of` and
 * `kiln_c99_node_hidden_pointer` accept both forms.
 *
 * The fields a program may write (`statusCode`) or that the runtime updates (`headersSent`,
 * `writableEnded`, `complete`) are ordinary own data properties at a fixed index. The runtime
 * writes them in place when the index still holds that key as a data property, and through the
 * ordinary `put` otherwise, so a program that deletes or redefines one keeps its answer.
 * ------------------------------------------------------------------------------------------- */
enum {
    KILN_C99_NODE_REQ_RESPONSE,
    KILN_C99_NODE_REQ_METHOD,
    KILN_C99_NODE_REQ_URL,
    KILN_C99_NODE_REQ_SOCKET,
    KILN_C99_NODE_REQ_CONNECTION,
    KILN_C99_NODE_REQ_COMPLETE,
    KILN_C99_NODE_REQ_COUNT
};
enum {
    KILN_C99_NODE_RES_RESPONSE,
    KILN_C99_NODE_RES_REQ,
    KILN_C99_NODE_RES_SOCKET,
    KILN_C99_NODE_RES_CONNECTION,
    KILN_C99_NODE_RES_STATUS_CODE,
    KILN_C99_NODE_RES_HEADERS_SENT,
    KILN_C99_NODE_RES_WRITABLE_ENDED,
    KILN_C99_NODE_RES_COUNT
};

typedef struct {
    /* The serial of the heap the keys and shapes below were resolved under; another heap
     * (realm) resolves them again. A serial rather than the heap pointer, because a freed heap's
     * address can come back as the next realm's, and a realm-local key or shape of the first
     * would then be read in the second. */
    uint64_t heap_serial;
    const kiln_string *request_keys[KILN_C99_NODE_REQ_COUNT];
    const kiln_string *response_keys[KILN_C99_NODE_RES_COUNT];
    kiln_shape *request_shape;
    kiln_shape *response_shape;
    /* The listener store `kiln_c99_node_event` keeps on an emitter, looked for on every emit. */
    const kiln_string *events_key;
} kiln_node_http_layout;

/* Per thread, like the realm cache it follows: a worker's realm runs on its own thread with its
 * own heap, and the heap pointer inside says which one the keys and shapes belong to. */
static _Thread_local kiln_node_http_layout kiln_c99_node_http_layouts;

/* The shape spelling `keys` in order from the root, by ordinary transitions: with shared keys the
 * chain is shared and every realm meets the same shapes, and a program that builds the same member
 * sequence meets them too instead of forking one. Unbounded, because the runtime's own layout must
 * not depend on how many branches the program grew off the root first. */
static kiln_shape *kiln_c99_node_layout_shape(const kiln_string *const *keys, size_t count) {
    kiln_shape *shape = kiln_c99_shape_root();
    size_t index;
    for (index = 0; index < count; ++index) {
        kiln_shape *next = NULL;
        if (!kiln_c99_shape_transition_ex(shape, keys[index], 0, 0, 1, &next)) return NULL;
        shape = next;
    }
    return shape;
}

typedef struct {
    const char *text;
    size_t len;
} kiln_node_layout_name;

static const kiln_node_layout_name kiln_c99_node_request_names[KILN_C99_NODE_REQ_COUNT] = {
    {"@@node:response", 15}, {"method", 6}, {"url", 3}, {"socket", 6}, {"connection", 10},
    {"complete", 8}};
static const kiln_node_layout_name kiln_c99_node_response_names[KILN_C99_NODE_RES_COUNT] = {
    {"@@node:response", 15}, {"req", 3}, {"socket", 6}, {"connection", 10},
    {"statusCode", 10}, {"headersSent", 11}, {"writableEnded", 13}};

/* The layouts' keys, registered with the built-ins' before the first realm (see
 * `kiln_c99_register_builtin_keys`), so every realm's request and response meet one shared
 * shape. Called under the process lock. */
static int kiln_c99_node_register_keys(kiln_shared_state *process) {
    size_t index;
    for (index = 0; index < KILN_C99_NODE_REQ_COUNT; ++index)
        if (!kiln_c99_register_static_text(process, kiln_c99_node_request_names[index].text,
                                           kiln_c99_node_request_names[index].len))
            return 0;
    for (index = 0; index < KILN_C99_NODE_RES_COUNT; ++index)
        if (!kiln_c99_register_static_text(process, kiln_c99_node_response_names[index].text,
                                           kiln_c99_node_response_names[index].len))
            return 0;
    return kiln_c99_register_static_text(process, "@@node:events", 13);
}

static int kiln_c99_node_http_layout_ready(const kiln_node_http_layout **out) {
    const kiln_node_layout_name *request_names = kiln_c99_node_request_names;
    const kiln_node_layout_name *response_names = kiln_c99_node_response_names;
    kiln_node_http_layout *layout = &kiln_c99_node_http_layouts;
    const kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t index;
    if (heap == NULL) return KILN_C99_OUT_OF_MEMORY;
    if (layout->heap_serial == heap->serial && heap->serial != 0) {
        *out = layout;
        return KILN_C99_OK;
    }
    layout->heap_serial = 0;
    /* Constant keys: shared when that is sound, and otherwise pinned in the realm's own table,
     * because shapes are plain memory the collector never traces. */
    for (index = 0; index < KILN_C99_NODE_REQ_COUNT; ++index) {
        layout->request_keys[index] =
            kiln_c99_intern_static(request_names[index].text, request_names[index].len);
        if (layout->request_keys[index] == NULL) return KILN_C99_OUT_OF_MEMORY;
    }
    for (index = 0; index < KILN_C99_NODE_RES_COUNT; ++index) {
        layout->response_keys[index] =
            kiln_c99_intern_static(response_names[index].text, response_names[index].len);
        if (layout->response_keys[index] == NULL) return KILN_C99_OUT_OF_MEMORY;
    }
    layout->events_key = kiln_c99_intern_static("@@node:events", 13);
    if (layout->events_key == NULL) return KILN_C99_OUT_OF_MEMORY;
    layout->request_shape = kiln_c99_node_layout_shape(layout->request_keys, KILN_C99_NODE_REQ_COUNT);
    layout->response_shape =
        kiln_c99_node_layout_shape(layout->response_keys, KILN_C99_NODE_RES_COUNT);
    if (layout->request_shape == NULL || layout->response_shape == NULL)
        return KILN_C99_OUT_OF_MEMORY;
    layout->heap_serial = heap->serial;
    *out = layout;
    return KILN_C99_OK;
}

/* A fresh object on `shape`, linked to `prototype`, with the hidden response pointer in slot 0
 * and every other slot undefined. Nothing here can run program code or collect: the object is
 * new, and the rest is stores. */
static int kiln_c99_node_http_instance(kiln_value *out, kiln_value prototype, kiln_shape *shape,
                                       kiln_node_response *response) {
    kiln_object *object;
    int status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) return status;
    object = kiln_c99_object_of(*out);
    object->shape = shape;
    object->slots[0] = KILN_VAL_TAG_PTR(KILN_TAG_OBJECT_PREFIX, response);
    if (kiln_c99_can_be_prototype(prototype)) object->prototype = prototype;
    return KILN_C99_OK;
}

/* A pointer read back from a hidden slot, in either form the object may hold it: the marked
 * word `kiln_c99_node_set_hidden_pointer` writes, or the plain object-tagged word a layout-built
 * slot becomes when its object moves to dictionary mode. The second is accepted only when the
 * block really is a response, since that conversion is the one way it can arise. */
static void *kiln_c99_node_hidden_pointer_value(kiln_value value) {
    void *pointer;
    if (value.tag != KILN_VALUE_OBJECT) return NULL;
    pointer = (void *)(uintptr_t)value.bits;
    if (value.len == 0xFFFFFFFFu) return pointer;
    if (pointer != NULL && kiln_c99_gc_kind_of_ptr(pointer) == KILN_GC_NODE_RESPONSE)
        return pointer;
    return NULL;
}

/* The response a request or response object carries, read from the layout's slot 0 before
 * falling back to the by-name lookup every other object needs. */
static kiln_node_response *kiln_c99_node_response_of(kiln_value receiver) {
    if (receiver.tag == KILN_VALUE_OBJECT) {
        const kiln_object *object = kiln_c99_object_of(receiver);
        const kiln_node_http_layout *layout = &kiln_c99_node_http_layouts;
        const kiln_string *key = layout->response_keys[KILN_C99_NODE_RES_RESPONSE];
        if (layout->heap_serial != 0 && key != NULL) {
            if (object->shape != NULL) {
                if (object->shape->property_count != 0 && object->shape->keys[0] == key &&
                    object->slot_capacity != 0 && KILN_VAL_IS_OBJECT(object->slots[0]))
                    return (kiln_node_response *)KILN_VAL_PTR(object->slots[0]);
            } else if (object->count != 0 && object->properties[0].key == key) {
                return (kiln_node_response *)kiln_c99_node_hidden_pointer_value(
                    object->properties[0].value);
            }
        }
    }
    return (kiln_node_response *)kiln_c99_node_hidden_pointer(receiver, "@@node:response", 15);
}

/* Store a runtime-owned member (`headersSent`, `writableEnded`, `complete`, `statusCode`) the
 * way an assignment would: in place when slot `index` still holds `key` as a plain data member,
 * and through the ordinary `put` when the program has moved, deleted or redefined it. */
static int kiln_c99_node_layout_store(kiln_value target, size_t index, const kiln_string *key,
                                      kiln_value value) {
    if (target.tag == KILN_VALUE_OBJECT) {
        kiln_object *object = kiln_c99_object_of(target);
        if (object->shape != NULL) {
            if (index < object->shape->property_count && object->shape->keys[index] == key &&
                index < object->slot_capacity && !object->frozen &&
                (object->shape->store_mask & (1u << index)) == 0) {
                object->slots[index] = kiln_val_from_boundary(value);
                return KILN_C99_OK;
            }
        } else if (index < object->count && object->properties[index].key == key &&
                   !object->properties[index].accessor && object->properties[index].writable &&
                   !object->frozen) {
            object->properties[index].value = value;
            return KILN_C99_OK;
        }
    }
    return kiln_c99_put(NULL, target, (const char *)kiln_string_latin1(key), key->unit_len, value,
                        0, NULL);
}

/* The value of the plain data member `key` at slot `index`, or 0 when the program has moved
 * it and the caller must read it the ordinary way. */
static int kiln_c99_node_layout_load(kiln_value target, size_t index, const kiln_string *key,
                                     kiln_value *out) {
    if (target.tag == KILN_VALUE_OBJECT) {
        const kiln_object *object = kiln_c99_object_of(target);
        if (object->shape != NULL) {
            if (index < object->shape->property_count && object->shape->keys[index] == key &&
                index < object->slot_capacity &&
                (object->shape->accessor_mask & (1u << index)) == 0) {
                *out = kiln_val_to_boundary(object->slots[index]);
                return 1;
            }
        } else if (index < object->count && object->properties[index].key == key &&
                   !object->properties[index].accessor) {
            *out = object->properties[index].value;
            return 1;
        }
    }
    return 0;
}

/* ---- the request's header bytes ---------------------------------------------------------- */

/* The head event's header buffer belongs to the runtime, which reuses it for the next request
 * on the connection: a `req.headers` first read after the listener returned -- after an
 * `await`, in a timer, from a request a program kept -- would read another request's headers,
 * or freed memory. Called when the head dispatch returns; a request whose headers were already
 * read has nothing borrowed left. */
static int kiln_c99_node_request_headers_keep(kiln_node_response *response) {
    char *owned;
    if (response == NULL || response->request_headers == NULL || response->request_headers_owned)
        return KILN_C99_OK;
    if (response->request_headers_len == 0) {
        response->request_headers = NULL;
        return KILN_C99_OK;
    }
    owned = (char *)malloc(response->request_headers_len);
    if (owned == NULL) {
        response->request_headers = NULL;
        response->request_headers_len = 0;
        return KILN_C99_OUT_OF_MEMORY;
    }
    memcpy(owned, response->request_headers, response->request_headers_len);
    response->request_headers = owned;
    response->request_headers_owned = 1;
    return KILN_C99_OK;
}

typedef struct {
    const char *name;
    size_t name_len;
    const char *value;
    size_t value_len;
} kiln_c99_node_header_field;

static int kiln_c99_node_header_name_order(const kiln_c99_node_header_field *left,
                                           const kiln_c99_node_header_field *right) {
    size_t length = left->name_len < right->name_len ? left->name_len : right->name_len;
    int order = length == 0 ? 0 : memcmp(left->name, right->name, length);
    if (order != 0) return order;
    return left->name_len < right->name_len ? -1 : left->name_len > right->name_len ? 1 : 0;
}

/* `req.headers` from the flattened bytes, with the interpreter's answer (`HttpHeaders::
 * headers_object`): names lowercased, one property per name in byte order of the name, and a
 * repeated field joined with ", " in arrival order. The runtime already lowercases names; the
 * lowering here is for a buffer that did not. */
static int kiln_c99_node_request_headers_object(kiln_node_response *response, kiln_value *out) {
    kiln_c99_node_header_field inline_fields[16];
    kiln_c99_node_header_field *fields = inline_fields;
    size_t count = 0;
    size_t capacity = sizeof inline_fields / sizeof inline_fields[0];
    size_t at = 0;
    size_t index;
    int status;
    const char *bytes = response == NULL ? NULL : response->request_headers;
    size_t len = response == NULL ? 0 : response->request_headers_len;
    status = kiln_c99_new_object_with_capacity(out, 8);
    if (status != KILN_C99_OK) return status;
    while (bytes != NULL && at < len) {
        const char *name = bytes + at;
        const char *name_end = (const char *)memchr(name, '\0', len - at);
        const char *value;
        const char *value_end;
        if (name_end == NULL) break;
        value = name_end + 1;
        if ((size_t)(value - bytes) >= len) break;
        value_end = (const char *)memchr(value, '\0', len - (size_t)(value - bytes));
        if (value_end == NULL) break;
        at = (size_t)(value_end - bytes) + 1;
        if (name_end == name) continue;
        if (count == capacity) {
            kiln_c99_node_header_field *grown;
            size_t next = capacity * 2;
            grown = (kiln_c99_node_header_field *)malloc(next * sizeof *grown);
            if (grown == NULL) {
                if (fields != inline_fields) free(fields);
                return KILN_C99_OUT_OF_MEMORY;
            }
            memcpy(grown, fields, count * sizeof *fields);
            if (fields != inline_fields) free(fields);
            fields = grown;
            capacity = next;
        }
        fields[count].name = name;
        fields[count].name_len = (size_t)(name_end - name);
        fields[count].value = value;
        fields[count].value_len = (size_t)(value_end - value);
        ++count;
    }
    /* Insertion sort: stable, so repeats keep their arrival order for the join below, and a
     * request carries a dozen fields, not a thousand. */
    for (index = 1; index < count; ++index) {
        kiln_c99_node_header_field moving = fields[index];
        size_t hole = index;
        while (hole > 0 && kiln_c99_node_header_name_order(&fields[hole - 1], &moving) > 0) {
            fields[hole] = fields[hole - 1];
            --hole;
        }
        fields[hole] = moving;
    }
    status = KILN_C99_OK;
    for (index = 0; status == KILN_C99_OK && index < count;) {
        size_t run = index + 1;
        size_t total = fields[index].value_len;
        char name_buffer[64];
        char *name = fields[index].name_len <= sizeof name_buffer
                         ? name_buffer
                         : (char *)malloc(fields[index].name_len);
        size_t byte;
        kiln_value value;
        kiln_property *slot;
        while (run < count && fields[run].name_len == fields[index].name_len &&
               memcmp(fields[run].name, fields[index].name, fields[index].name_len) == 0) {
            total += 2 + fields[run].value_len;
            ++run;
        }
        if (name == NULL) {
            status = KILN_C99_OUT_OF_MEMORY;
            break;
        }
        for (byte = 0; byte < fields[index].name_len; ++byte)
            name[byte] = (char)tolower((unsigned char)fields[index].name[byte]);
        if (run == index + 1) {
            value = kiln_c99_copy_text(fields[index].value, fields[index].value_len);
        } else {
            char *joined = (char *)malloc(total == 0 ? 1 : total);
            size_t written = 0;
            size_t part;
            if (joined == NULL) {
                if (name != name_buffer) free(name);
                status = KILN_C99_OUT_OF_MEMORY;
                break;
            }
            for (part = index; part < run; ++part) {
                if (part != index) {
                    joined[written++] = ',';
                    joined[written++] = ' ';
                }
                memcpy(joined + written, fields[part].value, fields[part].value_len);
                written += fields[part].value_len;
            }
            value = kiln_c99_copy_text(joined, written);
            free(joined);
        }
        if (value.tag != KILN_VALUE_STRING) {
            status = KILN_C99_OUT_OF_MEMORY;
        } else {
            /* An own definition, not an assignment: a header named `__proto__` is a header. */
            status = kiln_c99_own_slot(*out, name, fields[index].name_len, &slot);
            if (status == KILN_C99_OK) slot->value = value;
        }
        if (name != name_buffer) free(name);
        index = run;
    }
    if (fields != inline_fields) free(fields);
    return status;
}

/* ---- the response head the program is building ------------------------------------------ */

/* A string as UTF-8 bytes. An all-ASCII Latin-1 string (every header name, nearly every value)
 * is borrowed as is; anything else is converted into `*owned`, which the caller frees. */
static int kiln_c99_node_utf8_bytes(kiln_value text, const char **bytes, size_t *len,
                                    uint8_t **owned) {
    const kiln_string *string;
    *owned = NULL;
    if (text.tag != KILN_VALUE_STRING || text.ptr == NULL) return KILN_C99_INVALID_ARGUMENT;
    string = (const kiln_string *)text.ptr;
    if (!kiln_string_is_utf16(string)) {
        const uint8_t *latin1 = kiln_string_latin1(string);
        size_t index;
        for (index = 0; index < string->unit_len; ++index) {
            if (latin1[index] >= 0x80) break;
        }
        if (index == string->unit_len) {
            *bytes = string->unit_len == 0 ? "" : (const char *)latin1;
            *len = string->unit_len;
            return KILN_C99_OK;
        }
    }
    if (kiln_c99_string_to_utf8_lossy(text, owned, len) != KILN_C99_OK)
        return KILN_C99_OUT_OF_MEMORY;
    *bytes = *len == 0 ? "" : (const char *)*owned;
    return KILN_C99_OK;
}

/* The entry for `name` (ASCII case-insensitive) in the flattened head: its offset, the offset
 * of its value, and the offset just past it. Answers 0 when there is none. */
static int kiln_c99_node_out_find(const kiln_node_response *response, const char *name,
                                  size_t name_len, size_t *entry, size_t *value_at,
                                  size_t *end) {
    size_t at = 0;
    const char *bytes = response->out_headers;
    size_t len = response->out_headers_len;
    while (at < len) {
        const char *here = bytes + at;
        const char *name_end = (const char *)memchr(here, '\0', len - at);
        const char *value;
        const char *value_end;
        if (name_end == NULL) return 0;
        value = name_end + 1;
        value_end = (const char *)memchr(value, '\0', len - (size_t)(value - bytes));
        if (value_end == NULL) return 0;
        if (kiln_c99_ascii_case_equal(here, (size_t)(name_end - here), name, name_len)) {
            *entry = at;
            *value_at = (size_t)(value - bytes);
            *end = (size_t)(value_end - bytes) + 1;
            return 1;
        }
        at = (size_t)(value_end - bytes) + 1;
    }
    return 0;
}

static int kiln_c99_node_out_reserve(kiln_node_response *response, size_t needed) {
    return kiln_c99_reserve((void **)&response->out_headers, &response->out_headers_capacity,
                            needed, 1);
}

/* `setHeader`'s edit: a name already present keeps its spelling and position and takes the
 * new value; a new name is appended. The interpreter's `HttpResponseState::headers` rule. */
static int kiln_c99_node_out_set(kiln_node_response *response, const char *name,
                                 size_t name_len, const char *value, size_t value_len) {
    size_t entry, value_at, end;
    if (kiln_c99_node_out_find(response, name, name_len, &entry, &value_at, &end)) {
        size_t old_value_len = end - 1 - value_at;
        size_t tail = response->out_headers_len - end;
        size_t next_len = response->out_headers_len - old_value_len + value_len;
        (void)entry;
        if (value_len > old_value_len &&
            kiln_c99_node_out_reserve(response, next_len) != KILN_C99_OK)
            return KILN_C99_OUT_OF_MEMORY;
        memmove(response->out_headers + value_at + value_len + 1,
                response->out_headers + end, tail);
        if (value_len != 0) memcpy(response->out_headers + value_at, value, value_len);
        response->out_headers[value_at + value_len] = '\0';
        response->out_headers_len = next_len;
        return KILN_C99_OK;
    }
    if (name_len > SIZE_MAX / 2 || value_len > SIZE_MAX / 2 - name_len - 2 ||
        kiln_c99_node_out_reserve(response, response->out_headers_len + name_len + value_len + 2) !=
            KILN_C99_OK)
        return KILN_C99_OUT_OF_MEMORY;
    if (name_len != 0)
        memcpy(response->out_headers + response->out_headers_len, name, name_len);
    response->out_headers_len += name_len;
    response->out_headers[response->out_headers_len++] = '\0';
    if (value_len != 0)
        memcpy(response->out_headers + response->out_headers_len, value, value_len);
    response->out_headers_len += value_len;
    response->out_headers[response->out_headers_len++] = '\0';
    return KILN_C99_OK;
}

static void kiln_c99_node_out_remove(kiln_node_response *response, const char *name,
                                     size_t name_len) {
    size_t entry, value_at, end;
    while (kiln_c99_node_out_find(response, name, name_len, &entry, &value_at, &end)) {
        memmove(response->out_headers + entry, response->out_headers + end,
                response->out_headers_len - end);
        response->out_headers_len -= end - entry;
    }
}

/* `setHeader(name, value)` and one member of `writeHead`'s object: both are converted to text
 * the way the interpreter's `to_display_string` does, then edited into the head. */
static int kiln_c99_node_out_set_values(kiln_node_response *response, kiln_value name,
                                        kiln_value value) {
    const char *name_bytes;
    const char *value_bytes;
    size_t name_len;
    size_t value_len;
    uint8_t *name_owned;
    uint8_t *value_owned;
    int status;
    name = kiln_c99_text(name);
    value = kiln_c99_text(value);
    if (name.tag != KILN_VALUE_STRING || value.tag != KILN_VALUE_STRING)
        return KILN_C99_OUT_OF_MEMORY;
    status = kiln_c99_node_utf8_bytes(name, &name_bytes, &name_len, &name_owned);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_node_utf8_bytes(value, &value_bytes, &value_len, &value_owned);
    if (status != KILN_C99_OK) {
        free(name_owned);
        return status;
    }
    status = kiln_c99_node_out_set(response, name_bytes, name_len, value_bytes, value_len);
    free(name_owned);
    free(value_owned);
    return status;
}

static int kiln_c99_node_client_try_start(kiln_node_client_request *request) {
    kiln_text headers = {NULL, 0, 0};
    kiln_realm_http_request framed;
    int status;
    if (request == NULL || request->sent || request->socket == NULL)
        return KILN_C99_OK;
    if (kiln_c99_text_append(&headers, request->headers, request->headers_len) != KILN_C99_OK)
        return KILN_C99_OUT_OF_MEMORY;
    if (kiln_c99_text_append(&headers, "host", 4) != KILN_C99_OK ||
        kiln_c99_text_append(&headers, "\0", 1) != KILN_C99_OK ||
        kiln_c99_text_append(&headers, request->host_name, request->host_name_len) != KILN_C99_OK ||
        kiln_c99_text_append(&headers, "\0", 1) != KILN_C99_OK) {
        free(headers.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    memset(&framed, 0, sizeof framed);
    framed.method = (const uint8_t *)request->method;
    framed.method_len = request->method_len;
    framed.target = (const uint8_t *)request->path;
    framed.target_len = request->path_len;
    framed.headers = (const uint8_t *)headers.bytes;
    framed.headers_len = headers.len;
    status = kiln_realm_http_client_start_active(request->socket->resource, request->request_id,
                                                 &framed, 16384);
    free(headers.bytes);
    if (status == KILN_C99_OK) {
        request->sent = 1;
        if (request->pending_body_len != 0) {
            int write_status = kiln_realm_http_client_write_active(
                request->socket->resource, request->request_id,
                (const uint8_t *)request->pending_body,
                request->pending_body_len);
            status = write_status < KILN_C99_OK ? write_status : KILN_C99_OK;
        }
        free(request->pending_body);
        request->pending_body = NULL;
        request->pending_body_len = 0;
        request->pending_body_capacity = 0;
        if (status == KILN_C99_OK && request->ended) {
            int end_status = kiln_realm_http_client_end_active(request->socket->resource,
                                                               request->request_id,
                                                               NULL, 0);
            status = end_status < KILN_C99_OK ? end_status : KILN_C99_OK;
        }
    }
    return status;
}

static int kiln_c99_node_emit(kiln_host *host, kiln_value object, const char *event,
                              kiln_value argument, int has_argument, int depth,
                              kiln_value *thrown) {
    kiln_value payload[2];
    kiln_value ignored;
    /* No listener was ever registered on this object -- `end` on a request and `finish` on a
     * response nearly always. `kiln_c99_node_event` answers `emit` with false for exactly this
     * state without looking at the name, so the name need not be allocated to ask. */
    if (object.tag == KILN_VALUE_OBJECT) {
        const kiln_object *store = kiln_c99_object_of(object);
        const kiln_node_http_layout *layout = &kiln_c99_node_http_layouts;
        const kiln_gc_heap *heap = kiln_c99_gc_heap();
        if (layout->heap_serial != 0 && heap != NULL && layout->heap_serial == heap->serial) {
            /* The pinned key is the canonical pointer, so either representation is searched by
             * pointer, and a shaped emitter stays shaped. */
            int listening = 0;
            if (store->shape != NULL) {
                uint32_t slot;
                listening = kiln_c99_shape_find_slot(store->shape, layout->events_key, &slot);
            } else {
                size_t index;
                for (index = 0; index < store->count; ++index) {
                    if (store->properties[index].key == layout->events_key) {
                        listening = store->properties[index].value.tag == KILN_VALUE_ARRAY;
                        break;
                    }
                }
            }
            if (!listening) return KILN_C99_OK;
        } else {
            kiln_property *state = kiln_c99_own_private(object, "@@node:events", 13);
            if (state == NULL || state->value.tag != KILN_VALUE_ARRAY) return KILN_C99_OK;
        }
    }
    payload[0] = kiln_c99_copy_text(event, strlen(event));
    payload[1] = argument;
    return kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_EMIT, object, payload,
                               has_argument ? 2 : 1, depth, thrown);
}

static int kiln_c99_node_server_maybe_emit_close(kiln_node_server *server,
                                                  kiln_value *thrown) {
    kiln_node_socket *socket;
    if (server == NULL || !server->closing || server->closed || server->listener != 0)
        return KILN_C99_OK;
    for (socket = server->sockets; socket != NULL; socket = socket->next) {
        if (!socket->closed) return KILN_C99_OK;
    }
    server->closed = 1;
    return kiln_c99_node_emit(server->host, server->object, "close",
                              kiln_c99_undefined(), 0, 0, thrown);
}

static void kiln_c99_node_drop_completion(void *opaque) { kiln_c99_gc_release(opaque); }
static void kiln_c99_node_drop_http_connection(void *opaque) { (void)opaque; }

static int kiln_c99_node_complete(void *opaque, const kiln_async_completion *completion,
                                  void *host, void *thrown);

static int kiln_c99_node_start(uint32_t operation, uint64_t resource, uint64_t argument,
                               const char *text, size_t text_len, const uint8_t *bytes,
                               size_t bytes_len, kiln_node_completion *continuation,
                               uint64_t *request) {
    kiln_async_host_op framed;
    int status;
    memset(&framed, 0, sizeof framed);
    framed.operation = operation;
    framed.flags = 1;
    framed.resource = resource;
    framed.argument = argument;
    framed.text = (const uint8_t *)text;
    framed.text_len = text_len;
    framed.bytes = bytes;
    framed.bytes_len = bytes_len;
    status = kiln_realm_start_active_host_op(&framed, kiln_c99_node_complete,
                                             kiln_c99_node_drop_completion, continuation, request);
    if (status != KILN_C99_OK) kiln_c99_gc_release(continuation);
    return status;
}

static void kiln_c99_node_cancel_request(uint64_t *request) {
    if (*request != 0) {
        (void)kiln_realm_cancel_active_request(*request);
        *request = 0;
    }
}

#if defined(__unix__) || defined(__APPLE__)
#include <termios.h>
static struct termios kiln_orig_termios;
static int kiln_raw_mode_active = 0;
static int kiln_atexit_registered = 0;

static void kiln_c99_cfmakeraw(struct termios *t) {
    t->c_iflag &= ~(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
    t->c_oflag &= ~OPOST;
    t->c_lflag &= ~(ECHO | ECHONL | ICANON | ISIG | IEXTEN);
    t->c_cflag &= ~(CSIZE | PARENB);
    t->c_cflag |= CS8;
    t->c_cc[VMIN] = 1;
    t->c_cc[VTIME] = 0;
}

static void kiln_disable_raw_mode(void) {
    if (kiln_raw_mode_active) {
        tcsetattr(STDIN_FILENO, TCSAFLUSH, &kiln_orig_termios);
        kiln_raw_mode_active = 0;
    }
}

static int kiln_c99_node_set_raw_mode(int mode) {
    if (!isatty(STDIN_FILENO)) {
        return KILN_C99_OK;
    }
    if (mode) {
        if (!kiln_raw_mode_active) {
            struct termios raw;
            if (tcgetattr(STDIN_FILENO, &kiln_orig_termios) == -1) {
                return KILN_C99_OK;
            }
            if (!kiln_atexit_registered) {
                atexit(kiln_disable_raw_mode);
                kiln_atexit_registered = 1;
            }
            raw = kiln_orig_termios;
            kiln_c99_cfmakeraw(&raw);
            if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw) == -1) {
                return KILN_C99_OK;
            }
            kiln_raw_mode_active = 1;
        }
    } else {
        kiln_disable_raw_mode();
    }
    return KILN_C99_OK;
}
#else
static int kiln_c99_node_set_raw_mode(int mode) {
    (void)mode;
    return KILN_C99_OK;
}
#endif

static uint64_t kiln_c99_node_stdin_request = 0;
static int kiln_c99_node_stdin_paused = 1;
static int kiln_c99_node_stdin_read_active = 0;

static int kiln_c99_node_stdin_start_read(void) {
    if (kiln_c99_node_stdin_read_active || kiln_c99_node_stdin_paused) {
        return KILN_C99_OK;
    }
    kiln_node_completion *read = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION, sizeof *read);
    if (read == NULL) return KILN_C99_OUT_OF_MEMORY;
    read->kind = KILN_REALM_OP_STDIN_READ;
    kiln_c99_node_stdin_read_active = 1;
    return kiln_c99_node_start(KILN_REALM_OP_STDIN_READ, 0, 16384, NULL, 0, NULL, 0, read, &kiln_c99_node_stdin_request);
}

static int kiln_c99_node_stdin_resume(void *host, kiln_value receiver) {
    (void)host;
    (void)receiver;
    kiln_c99_node_stdin_paused = 0;
    return kiln_c99_node_stdin_start_read();
}

static void kiln_c99_node_stdin_pause(kiln_value receiver) {
    (void)receiver;
    kiln_c99_node_stdin_paused = 1;
    if (kiln_c99_node_stdin_request != 0) {
        kiln_c99_node_cancel_request(&kiln_c99_node_stdin_request);
        kiln_c99_node_stdin_read_active = 0;
    }
}

static int kiln_c99_node_close_socket(kiln_node_socket *socket, int force) {
    kiln_node_completion *work;
    if (socket->closed) return KILN_C99_OK;
    if (socket->closing) return KILN_C99_OK;
    if (socket->close_request != 0) return KILN_C99_OK;
    socket->closing = 1;
    kiln_c99_node_cancel_request(&socket->connect_request);
    kiln_c99_node_cancel_request(&socket->read_request);
    if (socket->server != NULL && socket->server->protocol_http) {
        // The C adapter registers a response before invoking the JavaScript listener, while the
        // realm only registers its Hyper body when response.start/end reaches the runtime. A
        // same-turn server.close() must therefore wait on C's response list before asking the
        // runtime to close; otherwise an idle runtime transport can tear down the socket before
        // the delayed response command is observed.
        if (!force && kiln_c99_node_has_unended_response(socket)) {
            socket->close_after_responses = 1;
            return KILN_C99_OK;
        }
        if (!force && socket->write_request != 0) {
            socket->close_after_write = 1;
            return KILN_C99_OK;
        }
        return kiln_realm_http_server_close_active(
            socket->resource,
            force ? KILN_REALM_HTTP_CLOSE_ABORT : KILN_REALM_HTTP_CLOSE_GRACEFUL);
    }
    kiln_c99_node_cancel_request(&socket->write_request);
    kiln_c99_node_cancel_request(&socket->shutdown_request);
    work = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION, sizeof *work);
    if (work == NULL) return KILN_C99_OUT_OF_MEMORY;
    work->kind = KILN_REALM_OP_CLOSE_RESOURCE;
    work->socket = socket;
    return kiln_c99_node_start(work->kind, socket->resource, 0, NULL, 0, NULL, 0, work,
                               &socket->close_request);
}

static int kiln_c99_node_socket_object(kiln_node_socket *socket, kiln_value *thrown) {
    kiln_value ignored;
    int status = kiln_c99_new_object_tenured(&socket->object);
    if (status == KILN_C99_OK)
        status = kiln_c99_node_event(socket->host, &ignored, KILN_C99_METHOD_NODE_EVENT_EMITTER,
                                     socket->object, NULL, 0, 0, thrown);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(socket->object, "write", 5,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_SOCKET_WRITE));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(socket->object, "end", 3,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_SOCKET_END));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(socket->object, "destroy", 7,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_SOCKET_DESTROY));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(
            socket->object, "setEncoding", 11,
            kiln_c99_method(KILN_C99_METHOD_NODE_READABLE_SET_ENCODING));
    if (status == KILN_C99_OK)
        status = kiln_c99_node_set_hidden_pointer(socket->object, "@@node:socket", 13, socket);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, socket->object, "readyState", 10,
                              kiln_c99_copy_text("open", 4), 0, NULL);
    return status;
}

static int kiln_c99_node_socket_ready_state(kiln_node_socket *socket,
                                            const char *state, size_t state_len) {
    return kiln_c99_put(NULL, socket->object, "readyState", 10,
                        kiln_c99_copy_text(state, state_len), 0, NULL);
}

static const char kiln_c99_transfer_encoding_chunked[] = "chunked";

static const char kiln_c99_slash[] = "/";

static kiln_value kiln_c99_node_http_target_text(const char *bytes, size_t len) {
    static _Thread_local kiln_value slash;
    if (len == 1 && bytes[0] == '/') {
        if (slash.tag != KILN_VALUE_STRING) slash = kiln_c99_static_text(kiln_c99_slash, 1);
        return slash;
    }
    return kiln_c99_copy_text(bytes, len);
}

/* A request method as a value, without copying bytes that are always one of nine spellings.
 *
 * Every method a server sees is a token from a fixed registry, so the string a program reads
 * back can be this unit's own literal rather than a fresh block per request. An extension method
 * -- the registry is open -- still copies, so nothing depends on the list being complete. */
static kiln_value kiln_c99_node_http_method_text(const char *bytes, size_t len) {
    static const struct { const char *name; size_t len; } known[] = {
        {"GET", 3}, {"POST", 4}, {"PUT", 3},     {"DELETE", 6},  {"HEAD", 4},
        {"OPTIONS", 7}, {"PATCH", 5}, {"CONNECT", 7}, {"TRACE", 5}};
    /* The answer for each, made once: `kiln_c99_static_text` scans its table to find it. */
    static _Thread_local kiln_value made[sizeof known / sizeof known[0]];
    size_t index;
    for (index = 0; index < sizeof known / sizeof known[0]; ++index) {
        if (known[index].len == len && known[index].name[0] == bytes[0] &&
            memcmp(known[index].name, bytes, len) == 0) {
            if (made[index].tag != KILN_VALUE_STRING)
                made[index] = kiln_c99_static_text(known[index].name, len);
            return made[index];
        }
    }
    return kiln_c99_copy_text(bytes, len);
}

typedef struct {
    const uint8_t *bytes;
    size_t len;
    /* Keeps a fallback ToString result rooted for the duration of the host operation. */
    kiln_value text;
    uint8_t *owned;
} kiln_c99_node_byte_span;

/* Borrow bytes from strings and typed views without coercing a Buffer/Uint8Array through the
 * language string representation (which would turn [0, 1, 2] into the text "0,1,2"). */
static int kiln_c99_node_http_byte_span(kiln_value value, kiln_c99_node_byte_span *out) {
    out->bytes = (const uint8_t *)"";
    out->len = 0;
    out->text = kiln_c99_undefined();
    out->owned = NULL;
    if (value.tag == KILN_VALUE_STRING) {
        /* An ASCII body is its own UTF-8 and is borrowed; `value` is the caller's argument,
         * which stays rooted for the whole host call. */
        const kiln_string *string = (const kiln_string *)value.ptr;
        if (string != NULL && !kiln_string_is_utf16(string)) {
            const uint8_t *latin1 = kiln_string_latin1(string);
            size_t index;
            for (index = 0; index < string->unit_len; ++index) {
                if (latin1[index] >= 0x80) break;
            }
            if (index == string->unit_len) {
                out->bytes = string->unit_len == 0 ? (const uint8_t *)"" : latin1;
                out->len = string->unit_len;
                return KILN_C99_OK;
            }
        }
        return kiln_c99_string_to_utf8_lossy(value, &out->owned, &out->len) == KILN_C99_OK
                   ? (out->bytes = out->owned, KILN_C99_OK)
                   : KILN_C99_OUT_OF_MEMORY;
    }
    if (value.tag == KILN_VALUE_TYPED_ARRAY) {
        kiln_typed_array *view = kiln_c99_typed_array_of(value);
        size_t width = kiln_c99_typed_width(view->kind);
        if (view->length > SIZE_MAX / width) return KILN_C99_OUT_OF_MEMORY;
        out->bytes = view->buffer->bytes + view->byte_offset;
        out->len = view->length * width;
        return KILN_C99_OK;
    }
    if (value.tag == KILN_VALUE_UNDEFINED) return KILN_C99_OK;
    out->text = kiln_c99_text(value);
    if (out->text.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    if (kiln_c99_string_to_utf8_lossy(out->text, &out->owned, &out->len) != KILN_C99_OK)
        return KILN_C99_OUT_OF_MEMORY;
    out->bytes = out->owned;
    return KILN_C99_OK;
}

static void kiln_c99_node_http_byte_span_release(kiln_c99_node_byte_span *span) {
    if (span != NULL) {
        free(span->owned);
        span->owned = NULL;
    }
}

static int kiln_c99_node_copy_utf8(kiln_value value, char **out, size_t *out_len) {
    uint8_t *encoded = NULL;
    size_t length = 0;
    char *copy;
    int status = kiln_c99_string_to_utf8_lossy(value, &encoded, &length);
    if (status != KILN_C99_OK) return status;
    copy = length == 0 ? NULL : (char *)malloc(length);
    if (length != 0 && copy == NULL) {
        free(encoded);
        return KILN_C99_OUT_OF_MEMORY;
    }
    if (length != 0) memcpy(copy, encoded, length);
    free(encoded);
    *out = copy;
    *out_len = length;
    return KILN_C99_OK;
}

/* A request for a fetch-style (`serve`) server. Nothing Node-shaped is built: the engine took the
 * request whole (the attach's whole-requests option), so this is the one event a request
 * produces, and the listener receives the head as strings, the body as a Buffer (undefined when
 * there is none), and the `(resource, id)` pair `fetchRespond` answers through. Everything handed
 * over is copied out of the engine's buffers, which are only borrowed for this call; the
 * arguments live in this frame, which a mid-turn collection scans. */
static int kiln_c99_node_fetch_request(kiln_node_socket *socket, const kiln_realm_http_event *event,
                                       kiln_value *exception) {
    kiln_value arguments[6];
    kiln_value ignored;
    int status;
    if (!kiln_c99_is_callable(socket->server->connection_listener)) return KILN_C99_OK;
    arguments[0] = kiln_c99_node_http_method_text((const char *)event->method,
                                                  (size_t)event->method_len);
    arguments[1] = kiln_c99_undefined();
    arguments[2] = kiln_c99_undefined();
    arguments[3] = kiln_c99_undefined();
    arguments[4] = kiln_c99_number((double)socket->resource);
    arguments[5] = kiln_c99_number((double)event->id);
    arguments[1] = kiln_c99_node_http_target_text((const char *)event->target,
                                                  (size_t)event->target_len);
    if (arguments[0].tag != KILN_VALUE_STRING || arguments[1].tag != KILN_VALUE_STRING)
        return KILN_C99_OUT_OF_MEMORY;
    /* The header list in its wire form, `\nname:value` per field, kept as one string: it is
     * the representation `Headers` itself holds, so nothing parses it until a handler asks. The
     * target above is the URL the engine built from the Host field (the bare target when there
     * was none). */
    arguments[2] = kiln_c99_copy_text((const char *)event->headers, (size_t)event->headers_len);
    if (arguments[2].tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    if (event->data_len != 0) {
        status = kiln_c99_buffer_from_bytes(&arguments[3], event->data, event->data_len);
        if (status != KILN_C99_OK) return status;
    }
    return kiln_c99_call(socket->host, &ignored, socket->server->connection_listener,
                         kiln_c99_undefined(), arguments, 6, 0, exception);
}

/* `fetchRespond(resource, id, status, headers, body)`: a whole response in one call, where
 * `headers` is the header list's wire form (`\nname:value` per field), which the engine parses
 * straight into Hyper's map on a fetch-style connection, and `body` a string or a byte view. Answers false, without throwing, when the connection is
 * gone -- an asynchronous handler can outlive its client. */
static int kiln_c99_node_fetch_respond(kiln_value *result, const kiln_value *args, size_t argc,
                                       kiln_value *thrown) {
    double resource = kiln_c99_as_number(argc > 0 ? args[0] : kiln_c99_undefined());
    double id = kiln_c99_as_number(argc > 1 ? args[1] : kiln_c99_undefined());
    double code = kiln_c99_as_number(argc > 2 ? args[2] : kiln_c99_undefined());
    kiln_c99_node_byte_span headers;
    kiln_c99_node_byte_span body;
    kiln_realm_http_response framed;
    int status;
    if (!(resource >= 1.0 && resource <= 9007199254740991.0 && floor(resource) == resource) ||
        !(id >= 1.0 && id <= 9007199254740991.0 && floor(id) == id))
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "fetchRespond requires a request handle");
    if (!(code >= 100.0 && code <= 999.0 && floor(code) == code))
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR, "Invalid status code");
    status = kiln_c99_node_http_byte_span(argc > 3 ? args[3] : kiln_c99_undefined(), &headers);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_node_http_byte_span(argc > 4 ? args[4] : kiln_c99_undefined(), &body);
    if (status != KILN_C99_OK) {
        kiln_c99_node_http_byte_span_release(&headers);
        return status;
    }
    memset(&framed, 0, sizeof framed);
    framed.status = (uint16_t)code;
    framed.headers = headers.bytes;
    framed.headers_len = headers.len;
    status = kiln_realm_http_response_start_active((uint64_t)resource, (uint64_t)id, &framed,
                                                   16384);
    if (status == KILN_C99_OK)
        status = kiln_realm_http_response_end_active((uint64_t)resource, (uint64_t)id,
                                                     body.bytes, body.len);
    kiln_c99_node_http_byte_span_release(&headers);
    kiln_c99_node_http_byte_span_release(&body);
    *result = kiln_c99_boolean(status >= KILN_C99_OK);
    return KILN_C99_OK;
}

static int kiln_c99_node_http_event(void *opaque, const kiln_realm_http_event *event,
                                    void *host, void *thrown) {
    kiln_node_socket *socket = (kiln_node_socket *)opaque;
    kiln_value *exception = (kiln_value *)thrown;
    kiln_node_client_request *client;
    kiln_node_response *response;
    kiln_value request;
    kiln_value ignored;
    int status;
    (void)host;
    if (socket == NULL || event == NULL) return KILN_C99_INVALID_ARGUMENT;
    client = kiln_c99_node_client_for_id(socket, event->id);
    if (event->kind == 1 && socket->server != NULL && socket->server->fetch_mode)
        return kiln_c99_node_fetch_request(socket, event, exception);
    if (event->kind == 1) {
        const kiln_node_http_layout *layout;
        kiln_value prototype;
        kiln_value response_prototype;
        status = kiln_c99_node_http_layout_ready(&layout);
        if (status == KILN_C99_OK) status = kiln_c99_node_incoming_message_prototype(&prototype);
        if (status == KILN_C99_OK)
            status = kiln_c99_callable_prototype(
                kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE), &response_prototype);
        if (status != KILN_C99_OK) return status;
        response = (kiln_node_response *)kiln_c99_gc_calloc(KILN_GC_NODE_RESPONSE,
                                                            sizeof *response);
        if (response == NULL) return KILN_C99_OUT_OF_MEMORY;
        response->socket = socket;
        response->request_id = event->id;
        response->status = 200;
        response->target_ptr = (const char *)event->target;
        response->target_len = (size_t)event->target_len;
        /* Borrowed for the dispatch only: `kiln_c99_node_request_headers_keep` below copies
         * what the listener did not consume before the runtime reuses the buffer. */
        if (event->headers_len != 0) {
            response->request_headers = (char *)event->headers;
            response->request_headers_len = (size_t)event->headers_len;
        }
        status = kiln_c99_node_http_instance(&request, prototype, layout->request_shape,
                                             response);
        if (status != KILN_C99_OK) return status;
        response->request = request;
        {
            /* The target copy is the one allocation among these values, so it is made before
             * the slots are written rather than between two writes through one pointer. */
            kiln_value url = kiln_c99_node_http_target_text((const char *)event->target,
                                                            (size_t)event->target_len);
            kiln_val *slots = kiln_c99_object_of(request)->slots;
            if (url.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
            slots[KILN_C99_NODE_REQ_METHOD] = kiln_val_from_boundary(kiln_c99_node_http_method_text(
                (const char *)event->method, (size_t)event->method_len));
            slots[KILN_C99_NODE_REQ_URL] = kiln_val_from_boundary(url);
            slots[KILN_C99_NODE_REQ_SOCKET] = kiln_val_from_boundary(socket->object);
            slots[KILN_C99_NODE_REQ_CONNECTION] = kiln_val_from_boundary(socket->object);
            slots[KILN_C99_NODE_REQ_COMPLETE] = KILN_VAL_FALSE;
        }
        if (!event->version_minor) {
            status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_HTTP_VERSION,
                                               kiln_c99_static_text(kiln_c99_http_version_10, 3));
            if (status == KILN_C99_OK)
                status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_HTTP_VERSION_MINOR,
                                                   kiln_c99_number(0.0));
            if (status != KILN_C99_OK) return status;
        }
        status = kiln_c99_node_http_instance(&response->object, response_prototype,
                                             layout->response_shape, response);
        if (status != KILN_C99_OK) return status;
        {
            kiln_val *slots = kiln_c99_object_of(response->object)->slots;
            slots[KILN_C99_NODE_RES_REQ] = kiln_val_from_boundary(request);
            slots[KILN_C99_NODE_RES_SOCKET] = kiln_val_from_boundary(socket->object);
            slots[KILN_C99_NODE_RES_CONNECTION] = kiln_val_from_boundary(socket->object);
            slots[KILN_C99_NODE_RES_STATUS_CODE] = kiln_val_from_boundary(kiln_c99_number(200.0));
            slots[KILN_C99_NODE_RES_HEADERS_SENT] = KILN_VAL_FALSE;
            slots[KILN_C99_NODE_RES_WRITABLE_ENDED] = KILN_VAL_FALSE;
        }
        response->next = socket->responses;
        socket->responses = response;
        {
            kiln_value arguments[2] = {request, response->object};
            int kept;
            if (!kiln_c99_is_callable(socket->server->connection_listener)) {
                return kiln_c99_node_request_headers_keep(response);
            }
            kiln_c99_active_http_response = response;
            status = kiln_c99_call(socket->host, &ignored, socket->server->connection_listener,
                                  socket->server->object, arguments, 2, 0, exception);
            kiln_c99_active_http_response = NULL;
            kept = kiln_c99_node_request_headers_keep(response);
            return status != KILN_C99_OK ? status : kept;
        }
    }
    if (event->kind == 2 || event->kind == 3) {
        for (response = socket->responses; response != NULL; response = response->next) {
            if (response->request_id == event->id) {
                const kiln_node_http_layout *layout;
                if (event->kind == 2) {
                    kiln_value data;
                    status = kiln_c99_buffer_from_bytes(&data, event->data, event->data_len);
                    if (status != KILN_C99_OK) return status;
                    return kiln_c99_node_emit(socket->host, response->request, "data", data, 1,
                                              0, exception);
                }
                status = kiln_c99_node_http_layout_ready(&layout);
                if (status == KILN_C99_OK)
                    status = kiln_c99_node_layout_store(
                        response->request, KILN_C99_NODE_REQ_COMPLETE,
                        layout->request_keys[KILN_C99_NODE_REQ_COMPLETE], kiln_c99_boolean(1));
                if (status != KILN_C99_OK) return status;
                return kiln_c99_node_emit(socket->host, response->request, "end",
                                          kiln_c99_undefined(), 0, 0, exception);
            }
        }
        return KILN_C99_OK;
    }
    if (event->kind == 5) {
        for (response = socket->responses; response != NULL; response = response->next) {
            if (response->request_id == event->id) {
                status = kiln_c99_node_emit(socket->host, response->object, "finish",
                                            kiln_c99_undefined(), 0, 0, exception);
                kiln_c99_node_unlink_response(response);
                return status;
            }
        }
    }
    if (client != NULL && event->kind == 6) {
        status = kiln_c99_new_object(&client->response);
            if (status == KILN_C99_OK) {
                kiln_value prototype;
                status = kiln_c99_node_incoming_message_prototype(&prototype);
                if (status == KILN_C99_OK)
                    status = kiln_c99_set_prototype(client->response, prototype);
            }
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, client->response, "statusCode", 10,
                                      kiln_c99_number((double)event->status), 0, NULL);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, client->response, "statusMessage", 13,
                                      kiln_c99_copy_text((const char *)event->reason,
                                                         (size_t)event->reason_len), 0, NULL);
            if (status == KILN_C99_OK) {
                kiln_value headers;
                size_t at = 0;
                /* Same shape as the server-side request headers object: a byte buffer of names
                 * and values with no header count of its own, and an ordinary response carries
                 * under a dozen. */
                status = kiln_c99_new_object_with_capacity(&headers, 8);
                while (status == KILN_C99_OK && at < event->headers_len) {
                    const char *name = (const char *)event->headers + at;
                    size_t name_len = strlen(name);
                    char *owned_name;
                    const char *value;
                    size_t value_len;
                    at += name_len + 1;
                    if (at > event->headers_len) { status = kiln_c99_refuse("an HTTP "
                                                                            "response "
                                                                            "header "
                                                                            "buffer "
                                                                            "that ends "
                                                                            "inside a "
                                                                            "header"); break; }
                    value = (const char *)event->headers + at;
                    value_len = strlen(value);
                    at += value_len + 1;
                    if (at > event->headers_len) { status = kiln_c99_refuse("an HTTP "
                                                                            "response "
                                                                            "header "
                                                                            "buffer "
                                                                            "that ends "
                                                                            "inside a "
                                                                            "header"); break; }
                    owned_name = name_len == 0 ? NULL : kiln_c99_gc_alloc_text(name_len);
                    if (name_len != 0 && owned_name == NULL) {
                        status = KILN_C99_OUT_OF_MEMORY;
                        break;
                    }
                    if (name_len != 0) memcpy(owned_name, name, name_len);
                    status = kiln_c99_put(NULL, headers, owned_name, name_len,
                                          kiln_c99_copy_text(value, value_len), 0, NULL);
                }
                if (status == KILN_C99_OK)
                    status = kiln_c99_put(NULL, client->response, "headers", 7, headers, 0,
                                          NULL);
            }
            if (status == KILN_C99_OK)
                status = kiln_c99_node_set_hidden_pointer(client->response, "@@node:client", 13,
                                                          client);
            if (status == KILN_C99_OK) {
                client->response_started = 1;
                status = kiln_c99_call(client->host, &ignored, client->callback,
                                       kiln_c99_undefined(), &client->response, 1, 0, exception);
            }
        return status;
    }
    if (client != NULL && (event->kind == 4 || event->kind == 5)) {
        return kiln_c99_node_emit(client->host, client->object,
                                  event->kind == 4 ? "drain" : "finish",
                                  kiln_c99_undefined(), 0, 0, exception);
    }
    if (client != NULL && event->kind == 7) {
        kiln_value data_value;
        int status;
        /* Readable.setEncoding("utf8") preserves the established text-chunk path; incremental
         * cross-chunk decoding remains a separate StringDecoder integration concern. Without it,
         * Node exposes each response chunk as a binary Buffer. */
        if (client->utf8) {
            data_value = kiln_c99_copy_text((const char *)event->data,
                                            (size_t)event->data_len);
        } else {
            status = kiln_c99_buffer_from_bytes(&data_value, event->data, event->data_len);
            if (status != KILN_C99_OK) return status;
        }
        return kiln_c99_node_emit(client->host, client->response, "data", data_value, 1,
                                  0, exception);
    }
    if (client != NULL && event->kind == 8) {
        uint64_t next_request_id = 0;
        int release;
        int status;
        if (client->response_ended) return KILN_C99_OK;
        client->response_ended = 1;
        status = kiln_c99_node_emit(client->host, client->response, "end",
                                    kiln_c99_undefined(), 0, 0, exception);
        if (status != KILN_C99_OK) return status;
        release = kiln_realm_http_agent_release_active(socket->resource,
                                                       event->flags != 0,
                                                       &next_request_id);
        if (release == KILN_REALM_HTTP_AGENT_DISPATCH) {
            kiln_node_client_request *next =
                kiln_c99_node_client_for_id(socket, next_request_id);
            if (next == NULL)
                KILN_C99_REFUSE("the agent dispatched a queued request that is no longer on this "
                                "socket");
            return kiln_c99_node_client_try_start(next);
        }
        if (release != KILN_REALM_HTTP_AGENT_IDLE &&
            release != KILN_REALM_HTTP_AGENT_RETIRE)
            KILN_C99_REFUSE("an HTTP agent release decision this backend does not model");
        return KILN_C99_OK;
    }
    if (event->kind == 9) {
        kiln_value message = kiln_c99_copy_text((const char *)event->data,
                                                (size_t)event->data_len);
        kiln_value object = client != NULL ? client->object : socket->object;
        return kiln_c99_node_emit(socket->host, object, "error", message, 1, 0, exception);
    }
    if (event->kind == 10) {
        int status;
        socket->closing = 1;
        socket->closed = 1;
        kiln_c99_node_unlink_socket(socket);
        if (kiln_c99_node_socket_ready_state(socket, "closed", 6) != KILN_C99_OK)
            KILN_C99_REFUSE("a socket ready state that could not be recorded");
        status = kiln_c99_node_emit(socket->host, socket->object, "close",
                                    kiln_c99_undefined(), 0, 0, exception);
        if (status != KILN_C99_OK) return status;
        return kiln_c99_node_server_maybe_emit_close(socket->server, exception);
    }
    return KILN_C99_OK;
}

static int kiln_c99_node_complete(void *opaque, const kiln_async_completion *completion,
                                  void *host, void *thrown) {
    kiln_node_completion *work = (kiln_node_completion *)opaque;
    kiln_value *exception = (kiln_value *)thrown;
    if (work == NULL || completion == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (work->kind == KILN_REALM_OP_STDIN_READ) {
        kiln_value *process_slot = NULL;
        kiln_value stdin_stream = kiln_c99_undefined();
        kiln_c99_node_stdin_read_active = 0;
        kiln_c99_node_stdin_request = 0;
        if (kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_PROCESS, &process_slot) == KILN_C99_OK &&
            process_slot != NULL && process_slot->tag == KILN_VALUE_OBJECT) {
            (void)kiln_c99_get(NULL, *process_slot, "stdin", 5, &stdin_stream, 0, NULL);
        }
        if (completion->status != KILN_REALM_ASYNC_OK) {
            if (stdin_stream.tag == KILN_VALUE_OBJECT) {
                kiln_value message = kiln_c99_copy_text((const char *)completion->error_message,
                                                        (size_t)completion->error_message_len);
                return kiln_c99_node_emit((kiln_host *)host, stdin_stream, "error", message, 1, 0, exception);
            }
            return KILN_C99_OK;
        }
        if (stdin_stream.tag != KILN_VALUE_OBJECT) {
            return KILN_C99_OK;
        }
        if (completion->bytes_len == 0) {
            return kiln_c99_node_emit((kiln_host *)host, stdin_stream, "end",
                                      kiln_c99_undefined(), 0, 0, exception);
        }
        {
            kiln_value bytes = kiln_c99_copy_text((const char *)completion->bytes,
                                                  (size_t)completion->bytes_len);
            kiln_value data;
            int status = kiln_c99_buffer_from(&data, bytes, NULL, 0, exception);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_node_emit((kiln_host *)host, stdin_stream, "data", data, 1, 0, exception);
            if (status != KILN_C99_OK) return status;
            if (!kiln_c99_node_stdin_paused) {
                return kiln_c99_node_stdin_start_read();
            }
            return KILN_C99_OK;
        }
    }
    if (completion->status != KILN_REALM_ASYNC_OK) {
        kiln_value message = kiln_c99_copy_text((const char *)completion->error_message,
                                                (size_t)completion->error_message_len);
        kiln_value object = work->server != NULL
                                ? work->server->object
                                : work->client_request != NULL
                                      ? work->client_request->object
                                      : work->socket->object;
        return kiln_c99_node_emit((kiln_host *)host, object, "error", message, 1, 0, exception);
    }
    if (work->client_request != NULL) {
        kiln_node_client_request *request = work->client_request;
        if (work->kind == KILN_REALM_OP_TCP_CONNECT) {
            kiln_node_socket *socket = (kiln_node_socket *)kiln_c99_gc_calloc(KILN_GC_NODE_SOCKET,
                                                                              sizeof *socket);
            int status;
            if (socket == NULL) return KILN_C99_OUT_OF_MEMORY;
            socket->host = request->host;
            socket->resource = completion->resource;
            status = kiln_realm_http_agent_connected_active(
                (const uint8_t *)request->host_name, request->host_name_len,
                request->port, socket->resource, request->request_id);
            if (status != 1)
                KILN_C99_REFUSE("the HTTP agent would not accept the connection it asked for");
            return kiln_c99_node_client_attach(request, socket, 1, exception);
        }
    }
    if (work->socket != NULL && work->kind == KILN_REALM_OP_TCP_CONNECT) {
        kiln_node_socket *socket = work->socket;
        kiln_node_completion *read;
        int status;
        socket->connect_request = 0;
        socket->resource = completion->resource;
        status = kiln_c99_node_emit(socket->host, socket->object, "connect",
                                    kiln_c99_undefined(), 0, 0, exception);
        if (status != KILN_C99_OK || socket->closing) return status;
        read = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION, sizeof *read);
        if (read == NULL) return KILN_C99_OUT_OF_MEMORY;
        read->kind = KILN_REALM_OP_TCP_READ;
        read->socket = socket;
        return kiln_c99_node_start(KILN_REALM_OP_TCP_READ, socket->resource, 16384,
                                   NULL, 0, NULL, 0, read, &socket->read_request);
    }
    if (work->server != NULL) {
        kiln_node_server *server = work->server;
        if (work->kind == KILN_REALM_OP_TCP_LISTEN) {
            kiln_node_completion *accept;
            int status;
            server->listener = completion->resource;
            free(server->address);
            server->address = NULL;
            server->address_len = (size_t)completion->address_len;
            if (server->address_len != 0) {
                server->address = (char *)malloc(server->address_len);
                if (server->address == NULL) return KILN_C99_OUT_OF_MEMORY;
                memcpy(server->address, completion->address, server->address_len);
            }
            server->port = completion->resource_port;
            server->listen_request = 0;
            status = kiln_c99_node_emit(server->host, server->object, "listening",
                                        kiln_c99_undefined(), 0, 0, exception);
            if (status != KILN_C99_OK || server->closing) return status;
            accept = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION,
                                                                sizeof *accept);
            if (accept == NULL) return KILN_C99_OUT_OF_MEMORY;
            accept->kind = KILN_REALM_OP_TCP_ACCEPT;
            accept->server = server;
            return kiln_c99_node_start(KILN_REALM_OP_TCP_ACCEPT, server->listener, 0, NULL, 0,
                                       NULL, 0, accept, &server->accept_request);
        }
        if (work->kind == KILN_REALM_OP_TCP_ACCEPT) {
            kiln_node_socket *socket;
            kiln_node_completion *accept;
            kiln_value ignored;
            int status;
            kiln_value allow_half_close;
            server->accept_request = 0;
            socket = (kiln_node_socket *)kiln_c99_gc_calloc(KILN_GC_NODE_SOCKET, sizeof *socket);
            if (socket == NULL) return KILN_C99_OUT_OF_MEMORY;
            socket->host = server->host;
            socket->next = server->sockets;
            server->sockets = socket;
            socket->resource = completion->resource;
            socket->server = server;
            status = kiln_c99_node_socket_object(socket, exception);
            if (status != KILN_C99_OK) return status;
            status = server->protocol_http
                         ? KILN_C99_OK
                         : kiln_c99_call(server->host, &ignored, server->connection_listener,
                                         server->object, &socket->object, 1, 0, exception);
            if (status != KILN_C99_OK) return status;
            if (server->closing || socket->closing) return KILN_C99_OK;
            status = server->protocol_http
                         ? kiln_c99_get(server->host, server->object, "httpAllowHalfOpen", 17,
                                        &allow_half_close, 0, exception)
                         : KILN_C99_OK;
            /* Option bit 0 is half-close; bit 1 asks the engine for whole requests, which is
             * what a fetch-style server's listener consumes. */
            if (status == KILN_C99_OK && server->protocol_http)
                status = kiln_realm_http_server_attach_with_options_active(
                    socket->resource,
                    (kiln_c99_truthy(allow_half_close) ? 1 : 0) | (server->fetch_mode ? 2 : 0),
                    kiln_c99_node_http_event, kiln_c99_node_drop_http_connection, socket);
            if (status != KILN_C99_OK || server->closing) return status;
            if (!server->protocol_http) {
                kiln_node_completion *read =
                    (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION,
                                                               sizeof *read);
                if (read == NULL) return KILN_C99_OUT_OF_MEMORY;
                read->kind = KILN_REALM_OP_TCP_READ;
                read->socket = socket;
                status = kiln_c99_node_start(KILN_REALM_OP_TCP_READ, socket->resource, 16384,
                                             NULL, 0, NULL, 0, read,
                                             &socket->read_request);
                if (status != KILN_C99_OK) return status;
            }
            accept = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION,
                                                                sizeof *accept);
            if (accept == NULL) return KILN_C99_OUT_OF_MEMORY;
            accept->kind = KILN_REALM_OP_TCP_ACCEPT;
            accept->server = server;
            return kiln_c99_node_start(KILN_REALM_OP_TCP_ACCEPT, server->listener, 0, NULL, 0,
                                       NULL, 0, accept, &server->accept_request);
        }
        if (work->kind == KILN_REALM_OP_CLOSE_RESOURCE) {
            server->listener = 0;
            return kiln_c99_node_server_maybe_emit_close(server, exception);
        }
    }
    if (work->socket != NULL) {
        kiln_node_socket *socket = work->socket;
        if (work->kind == KILN_REALM_OP_TCP_READ) {
            socket->read_request = 0;
            if (completion->bytes_len == 0) {
                int status = kiln_c99_node_emit(socket->host, socket->object, "end",
                                                kiln_c99_undefined(), 0, 0, exception);
                if (status != KILN_C99_OK) return status;
                return kiln_c99_node_close_socket(socket, 1);
            }
            {
                kiln_node_completion *read;
                kiln_value data = kiln_c99_copy_text((const char *)completion->bytes,
                                                     (size_t)completion->bytes_len);
                int status;
                status = kiln_c99_node_emit(socket->host, socket->object, "data", data, 1, 0,
                                            exception);
                if (status != KILN_C99_OK || socket->closing) return status;
                read = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION,
                                                                  sizeof *read);
                if (read == NULL) return KILN_C99_OUT_OF_MEMORY;
                read->kind = KILN_REALM_OP_TCP_READ;
                read->socket = socket;
                return kiln_c99_node_start(KILN_REALM_OP_TCP_READ, socket->resource, 16384,
                                           NULL, 0, NULL, 0, read, &socket->read_request);
            }
        }
        if (work->kind == KILN_REALM_OP_TCP_WRITE) {
            kiln_node_completion *next;
            socket->write_request = 0;
            if (socket->close_after_write) {
                next = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION,
                                                                  sizeof *next);
                if (next == NULL) return KILN_C99_OUT_OF_MEMORY;
                next->kind = KILN_REALM_OP_CLOSE_RESOURCE;
                next->socket = socket;
                return kiln_c99_node_start(KILN_REALM_OP_CLOSE_RESOURCE, socket->resource, 0,
                                           NULL, 0, NULL, 0, next,
                                           &socket->close_request);
            }
            if (socket->shutdown_after_write) {
                socket->shutdown_after_write = 0;
                next = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION,
                                                                  sizeof *next);
                if (next == NULL) return KILN_C99_OUT_OF_MEMORY;
                next->kind = KILN_REALM_OP_TCP_SHUTDOWN;
                next->socket = socket;
                return kiln_c99_node_start(KILN_REALM_OP_TCP_SHUTDOWN, socket->resource, 0,
                                           NULL, 0, NULL, 0, next,
                                           &socket->shutdown_request);
            }
            return kiln_c99_node_emit(socket->host, socket->object, "drain",
                                      kiln_c99_undefined(), 0, 0, exception);
        }
        if (work->kind == KILN_REALM_OP_TCP_SHUTDOWN) {
            socket->shutdown_request = 0;
            return kiln_c99_node_emit(socket->host, socket->object, "finish",
                                      kiln_c99_undefined(), 0, 0, exception);
        }
        if (work->kind == KILN_REALM_OP_CLOSE_RESOURCE) {
            int status;
            socket->close_request = 0;
            socket->closed = 1;
            kiln_c99_node_unlink_socket(socket);
            if (kiln_c99_node_socket_ready_state(socket, "closed", 6) != KILN_C99_OK)
                KILN_C99_REFUSE("a socket ready state that could not be recorded");
            status = kiln_c99_node_emit(socket->host, socket->object, "close",
                                        kiln_c99_undefined(), 0, 0, exception);
            if (status != KILN_C99_OK) return status;
            return kiln_c99_node_server_maybe_emit_close(socket->server, exception);
        }
    }
    KILN_C99_REFUSE("a socket completion this backend does not model");
}

static void kiln_c99_node_drop_fs_completion(void *opaque) { kiln_c99_gc_release(opaque); }

static int kiln_c99_node_fs_error_property(kiln_value error, const char *key, size_t key_len,
                                           const uint8_t *bytes, uint64_t length) {
    kiln_property *property;
    size_t length_as_size;
    kiln_value value;
    int status;
    if (length > SIZE_MAX)
        KILN_C99_REFUSE("a filesystem error field longer than this platform can address");
    length_as_size = (size_t)length;
    value = kiln_c99_copy_text((const char *)bytes, length_as_size);
    if (value.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    status = kiln_c99_put(NULL, error, key, key_len, value, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_own_slot(error, key, key_len, &property);
    if (status != KILN_C99_OK || property == NULL)
        KILN_C99_REFUSE("a filesystem error field that left no property slot");
    property->enumerable = 0;
    return KILN_C99_OK;
}

static int kiln_c99_node_fs_stat(kiln_value *out, const kiln_async_completion *completion) {
    kiln_value stat;
    kiln_property *property;
    int status;
    status = kiln_c99_new_object(&stat);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(NULL, stat, "size", 4,
                          kiln_c99_number((double)completion->value_u64), 0, NULL);
    if (status == KILN_C99_OK) {
        status = kiln_c99_own_slot(stat, "@@kiln:node:is-file", 19, &property);
        if (status == KILN_C99_OK && property != NULL) {
            property->value = kiln_c99_boolean((completion->resource_flags &
                                                (UINT32_C(1) << 3)) != 0);
            property->enumerable = 0;
            property->writable = 0;
            property->configurable = 0;
            kiln_c99_object_of(stat)->read_only_seen = 1;
        }
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_own_slot(stat, "@@kiln:node:is-directory", 24, &property);
        if (status == KILN_C99_OK && property != NULL) {
            property->value = kiln_c99_boolean((completion->resource_flags &
                                                (UINT32_C(1) << 4)) != 0);
            property->enumerable = 0;
            property->writable = 0;
            property->configurable = 0;
            kiln_c99_object_of(stat)->read_only_seen = 1;
        }
    }
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, stat, "isFile", 6,
                              kiln_c99_method(KILN_C99_METHOD_NODE_STAT_IS_FILE), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, stat, "isDirectory", 11,
                              kiln_c99_method(KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY), 0, NULL);
    if (status == KILN_C99_OK) *out = stat;
    return status;
}

static int kiln_c99_node_fs_complete(void *opaque, const kiln_async_completion *completion,
                                     void *host, void *thrown) {
    kiln_node_fs_completion *work = (kiln_node_fs_completion *)opaque;
    kiln_value value;
    int status;
    size_t length;
    (void)host;
    (void)thrown;
    if (work == NULL || completion == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (completion->status != KILN_REALM_ASYNC_OK) {
        if (completion->error_message_len > SIZE_MAX || completion->error_code_len > SIZE_MAX ||
            completion->error_syscall_len > SIZE_MAX || completion->error_path_len > SIZE_MAX)
            KILN_C99_REFUSE("a filesystem error field longer than this platform can address");
        if ((completion->error_message_len != 0 && completion->error_message == NULL) ||
            (completion->error_code_len != 0 && completion->error_code == NULL) ||
            (completion->error_syscall_len != 0 && completion->error_syscall == NULL) ||
            (completion->error_path_len != 0 && completion->error_path == NULL))
            return KILN_C99_INVALID_ARGUMENT;
        status = kiln_c99_new_error(
            &value, KILN_C99_BUILTIN_ERROR,
            kiln_c99_copy_text((const char *)completion->error_message,
                               (size_t)completion->error_message_len));
        if (status == KILN_C99_OK)
            status = kiln_c99_node_fs_error_property(value, "code", 4, completion->error_code,
                                                     completion->error_code_len);
        if (status == KILN_C99_OK)
            status = kiln_c99_node_fs_error_property(value, "syscall", 7,
                                                     completion->error_syscall,
                                                     completion->error_syscall_len);
        if (status == KILN_C99_OK && completion->error_path_len != 0)
            status = kiln_c99_node_fs_error_property(value, "path", 4, completion->error_path,
                                                     completion->error_path_len);
        if (status != KILN_C99_OK) return status;
        return kiln_c99_settle(work->promise, value, 1);
    }
    if (work->operation == KILN_REALM_OP_WRITE_FILE ||
        work->operation == KILN_REALM_OP_MKDIR) {
        if (completion->value_kind != KILN_REALM_ASYNC_UNDEFINED)
            KILN_C99_REFUSE("writeFile completed carrying a value where it should have none");
        return kiln_c99_settle(work->promise, kiln_c99_undefined(), 0);
    }
    if (work->operation == KILN_REALM_OP_STAT) {
        if (completion->value_kind != KILN_REALM_ASYNC_FILE_STAT)
            KILN_C99_REFUSE("stat completed without file statistics");
        status = kiln_c99_node_fs_stat(&value, completion);
        if (status != KILN_C99_OK) return status;
        return kiln_c99_settle(work->promise, value, 0);
    }
    if (completion->value_kind != KILN_REALM_ASYNC_BYTES || completion->bytes_len > SIZE_MAX)
        KILN_C99_REFUSE("a filesystem read completed without bytes");
    length = (size_t)completion->bytes_len;
    if (length != 0 && completion->bytes == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (work->utf8) {
        value = kiln_c99_copy_utf8_lossy(completion->bytes, length);
    } else {
        status = kiln_c99_typed_alloc(&value, KILN_TYPED_UINT8, length);
        if (status != KILN_C99_OK) return status;
        kiln_c99_typed_array_of(value)->is_buffer = 1;
        if (length != 0)
            memcpy(kiln_c99_typed_array_of(value)->buffer->bytes, completion->bytes, length);
    }
    if (value.tag == KILN_VALUE_UNDEFINED) return KILN_C99_OUT_OF_MEMORY;
    return kiln_c99_settle(work->promise, value, 0);
}

static void kiln_c99_node_drop_process(void *opaque) { kiln_c99_gc_release(opaque); }

static int kiln_c99_node_process_complete(void *opaque, const kiln_async_completion *completion,
                                          void *host, void *thrown) {
    kiln_node_process_completion *work = (kiln_node_process_completion *)opaque;
    kiln_value result, stdout_value, stderr_value;
    int status;
    (void)host; (void)thrown;
    if (work == NULL || completion == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (completion->status != KILN_REALM_ASYNC_OK) {
        if (completion->error_message_len > SIZE_MAX || completion->error_code_len > SIZE_MAX ||
            completion->error_syscall_len > SIZE_MAX || completion->error_path_len > SIZE_MAX)
            KILN_C99_REFUSE("a process error field longer than this platform can address");
        if ((completion->error_message_len != 0 && completion->error_message == NULL) ||
            (completion->error_code_len != 0 && completion->error_code == NULL) ||
            (completion->error_syscall_len != 0 && completion->error_syscall == NULL) ||
            (completion->error_path_len != 0 && completion->error_path == NULL))
            return KILN_C99_INVALID_ARGUMENT;
        status = kiln_c99_new_error(
            &result, KILN_C99_BUILTIN_ERROR,
            kiln_c99_copy_text((const char *)completion->error_message,
                               (size_t)completion->error_message_len));
        work->result = result;
        if (status == KILN_C99_OK)
            status = kiln_c99_node_fs_error_property(result, "code", 4, completion->error_code,
                                                     completion->error_code_len);
        if (status == KILN_C99_OK)
            status = kiln_c99_node_fs_error_property(result, "syscall", 7,
                                                     completion->error_syscall,
                                                     completion->error_syscall_len);
        if (status == KILN_C99_OK && completion->error_path_len != 0)
            status = kiln_c99_node_fs_error_property(result, "path", 4, completion->error_path,
                                                     completion->error_path_len);
        if (status != KILN_C99_OK) return status;
        return kiln_c99_settle(work->promise, result, 1);
    }
    if (completion->value_kind != KILN_REALM_ASYNC_PROCESS_OUTPUT)
        KILN_C99_REFUSE("process host completion had no output");
    if (completion->bytes_len > SIZE_MAX || completion->stderr_len > SIZE_MAX)
        KILN_C99_REFUSE("process output longer than this platform can address");
    if ((completion->bytes_len != 0 && completion->bytes == NULL) ||
        (completion->stderr_len != 0 && completion->stderr_bytes == NULL))
        return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_new_object(&result);
    if (status != KILN_C99_OK) return status;
    work->result = result;
    status = kiln_c99_typed_alloc(&stdout_value, KILN_TYPED_UINT8, (size_t)completion->bytes_len);
    if (status != KILN_C99_OK) return status;
    work->stdout_value = stdout_value;
    status = kiln_c99_typed_alloc(&stderr_value, KILN_TYPED_UINT8, (size_t)completion->stderr_len);
    if (status != KILN_C99_OK) return status;
    work->stderr_value = stderr_value;
    if (completion->bytes_len) memcpy(kiln_c99_typed_array_of(stdout_value)->buffer->bytes, completion->bytes, (size_t)completion->bytes_len);
    if (completion->stderr_len) memcpy(kiln_c99_typed_array_of(stderr_value)->buffer->bytes, completion->stderr_bytes, (size_t)completion->stderr_len);
    kiln_c99_typed_array_of(stdout_value)->is_buffer = 1;
    kiln_c99_typed_array_of(stderr_value)->is_buffer = 1;
    status = kiln_c99_put(NULL, result, "stdout", 6, stdout_value, 0, NULL);
    if (status == KILN_C99_OK) status = kiln_c99_put(NULL, result, "stderr", 6, stderr_value, 0, NULL);
    if (status == KILN_C99_OK) status = kiln_c99_put(NULL, result, "exitCode", 8, completion->exit_code == INT32_MIN ? kiln_c99_null() : kiln_c99_number((double)completion->exit_code), 0, NULL);
    if (status == KILN_C99_OK) status = kiln_c99_put(NULL, result, "signal", 6, completion->signal == INT32_MIN ? kiln_c99_null() : kiln_c99_number((double)completion->signal), 0, NULL);
    if (status == KILN_C99_OK) status = kiln_c99_settle(work->promise, result, 0);
    return status;
}

int kiln_c99_node_process_raw(kiln_host *host, kiln_value *result, uint64_t code,
                              const kiln_value *args, size_t argc, int depth, kiln_value *thrown) {
    (void)host; (void)depth;
    if (code == KILN_C99_METHOD_PROCESS_CANCEL_RAW) {
        double number = argc && args[0].tag == KILN_VALUE_NUMBER ? kiln_c99_as_number(args[0]) : -1;
        uint64_t request = number >= 0 && isfinite(number) && number == floor(number) &&
                                   number <= 9007199254740991.0
                               ? (uint64_t)number
                               : 0;
        *result = kiln_c99_boolean(kiln_realm_cancel_active_request(request) == KILN_C99_OK);
        return KILN_C99_OK;
    }
    kiln_value bytes = argc ? args[0] : kiln_c99_undefined();
    kiln_value promise, pair;
    kiln_node_process_completion *work;
    kiln_async_host_op operation = {0};
    uint64_t request = 0;
    if (bytes.tag != KILN_VALUE_TYPED_ARRAY || kiln_c99_typed_array_of(bytes)->kind != KILN_TYPED_UINT8)
        return kiln_c99_throw_not_a(thrown, bytes, "Uint8Array request");
    if (kiln_c99_new_promise(&promise) != KILN_C99_OK) return KILN_C99_OUT_OF_MEMORY;
    work = (kiln_node_process_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_PROCESS_COMPLETION, sizeof *work);
    if (work == NULL) return KILN_C99_OUT_OF_MEMORY;
    work->promise = promise;
    operation.operation = KILN_REALM_OP_PROCESS_RUN; operation.flags = 1;
    operation.bytes = kiln_c99_typed_array_of(bytes)->buffer->bytes + kiln_c99_typed_array_of(bytes)->byte_offset;
    operation.bytes_len = kiln_c99_typed_array_of(bytes)->length;
    int status_code = kiln_realm_start_active_host_op(&operation, kiln_c99_node_process_complete, kiln_c99_node_drop_process, work, &request);
    if (status_code != KILN_C99_OK) {
        kiln_c99_gc_release(work);
        return status_code;
    }
    if (kiln_c99_new_object(&pair) != KILN_C99_OK) {
        (void)kiln_realm_cancel_active_request(request);
        return KILN_C99_OUT_OF_MEMORY;
    }
    work->result = pair;
    status_code = kiln_c99_put(NULL, pair, "requestId", 9, kiln_c99_number((double)request), 0, NULL);
    if (status_code == KILN_C99_OK)
        status_code = kiln_c99_put(NULL, pair, "promise", 7, promise, 0, NULL);
    if (status_code != KILN_C99_OK) {
        (void)kiln_realm_cancel_active_request(request);
        return status_code;
    }
    *result = pair; return KILN_C99_OK;
}

/* ---- Cross-realm ports and `node:worker_threads` (`kiln_runtime::workers`; realms issue 06) ---
 *
 * Two primitives, captured and deleted by the realm bootstrap (`kiln-builtins/src/
 * web_events_streams.ts`), which owns every rule about what a message is: `__kilnWorkerOp(op,
 * ...)`, the runtime's synchronous operations, and `__kilnPortWait(endpoint, referenced)`, a host
 * operation done when an endpoint has something for this realm. What is this engine's own is the
 * translation between its values and message tokens -- and a transferred `ArrayBuffer`, whose
 * block moves with the message rather than being copied (ADR 0070 primitive C). */
#define KILN_C99_WORKER_OP_CHANNEL 1
#define KILN_C99_WORKER_OP_POST 2
#define KILN_C99_WORKER_OP_TAKE 3
#define KILN_C99_WORKER_OP_CLOSE 4
#define KILN_C99_WORKER_OP_STATE 5
#define KILN_C99_WORKER_OP_ENQUEUE 6
#define KILN_C99_WORKER_OP_BROADCAST_JOIN 7
#define KILN_C99_WORKER_OP_BROADCAST_POST 8
#define KILN_C99_WORKER_OP_SPAWN 9
#define KILN_C99_WORKER_OP_TERMINATE 10
#define KILN_C99_WORKER_OP_SELF 11
#define KILN_C99_WORKER_OP_CANCEL_WAIT 12
#define KILN_C99_WORKER_OP_REAP 13
#define KILN_C99_WORKER_OP_STDIO 14
#define KILN_C99_WORKER_OP_REPORT_ERROR 15
/* Process signals (`kiln_runtime::signals`, realms issue 23), for `kiln-builtins/src/signals.ts`. */
#define KILN_C99_WORKER_OP_SIGNAL_NUMBER 16
#define KILN_C99_WORKER_OP_SIGNAL_LISTEN 17
#define KILN_C99_WORKER_OP_SIGNAL_UNHANDLED 18
#define KILN_C99_WORKER_OP_KILL 19
#define KILN_C99_WORKER_OP_POST_PLAIN 21
#define KILN_C99_WORKER_OP_ACTOR_REGISTER 22
#define KILN_C99_WORKER_OP_ACTOR_LOOKUP 23
#define KILN_C99_WORKER_OP_ACTOR_CLOSE 24
#define KILN_C99_WORKER_OP_ACTOR_REQUEST_ID 25
#define KILN_C99_WORKER_OP_ACTOR_ISOLATE 26
#define KILN_C99_WORKER_OP_ACTOR_ISOLATED_HAS 27
#define KILN_C99_WORKER_OP_ACTOR_CANDIDATE_BEGIN 28
#define KILN_C99_WORKER_OP_ACTOR_CANDIDATE_END 29
#define KILN_C99_WORKER_OP_ACTOR_GRAPH_REQUEST 30
#define KILN_C99_WORKER_OP_ACTOR_GRAPH_STATUS 31
#define KILN_C99_WORKER_OP_ACTOR_GRAPH_CANCEL 32
#define KILN_C99_WORKER_OP_ACTOR_BIND_RECEIVER 33
#define KILN_C99_WORKER_OP_ACTOR_GRAPH_RESERVE 34
#define KILN_C99_WORKER_OP_ACTOR_GRAPH_RECEIVE 35
#define KILN_C99_WORKER_OP_ACTOR_GRAPH_RELEASE 36

static uint64_t kiln_c99_worker_word(const kiln_value *args, size_t argc, size_t at) {
    double number;
    if (at >= argc || args[at].tag != KILN_VALUE_NUMBER) return 0;
    number = kiln_c99_as_number(args[at]);
    return number >= 0 && number <= 9007199254740991.0 ? (uint64_t)number : 0;
}

/* The tokens of a message the bootstrap flattened. A buffer in it is one the structured clone
 * just made, which nothing else in this realm references: its block moves into the message and
 * the buffer is left detached -- or, for a broadcast's several receivers (`copy`), its bytes are
 * copied. The list stays rooted by the caller, and nothing here allocates on the collected heap,
 * so the string units the tokens point at stay where they are until the provider has copied
 * them. */
static int kiln_c99_worker_tokens(kiln_value list, int copy, kiln_worker_token **out,
                                  size_t *out_count, kiln_value *thrown) {
    kiln_array *items;
    kiln_worker_token *tokens;
    size_t index, count;
    *out = NULL;
    *out_count = 0;
    if (list.tag != KILN_VALUE_ARRAY) return kiln_c99_throw_not_a(thrown, list, "message token list");
    items = kiln_c99_array_of(list);
    count = items->count;
    tokens = (kiln_worker_token *)calloc(count == 0 ? 1 : count, sizeof *tokens);
    if (tokens == NULL) return KILN_C99_OUT_OF_MEMORY;
    for (index = 0; index < count; ++index) {
        kiln_value item = kiln_val_to_boundary(items->elements[index]);
        kiln_worker_token *token = &tokens[index];
        switch (item.tag) {
        case KILN_VALUE_UNDEFINED:
        case KILN_VALUE_HOLE:
            token->kind = KILN_WORKER_TOKEN_UNDEFINED;
            break;
        case KILN_VALUE_NULL:
            token->kind = KILN_WORKER_TOKEN_NULL;
            break;
        case KILN_VALUE_BOOLEAN:
            token->kind = KILN_WORKER_TOKEN_BOOL;
            token->flag = item.bits != 0 ? 1u : 0u;
            break;
        case KILN_VALUE_NUMBER:
            token->kind = KILN_WORKER_TOKEN_NUMBER;
            token->number = kiln_c99_as_number(item);
            break;
        case KILN_VALUE_STRING: {
            const kiln_string *text = (const kiln_string *)item.ptr;
            token->kind = KILN_WORKER_TOKEN_STRING;
            if (text != NULL) {
                token->flag = kiln_string_is_utf16(text) ? 1u : 0u;
                token->data = text->data;
                token->len = text->unit_len;
            }
            break;
        }
        case KILN_VALUE_SHARED: {
            kiln_shared_facade *facade = (kiln_shared_facade *)(uintptr_t)item.bits;
            if (facade == NULL) { free(tokens); return KILN_C99_INVALID_ARGUMENT; }
            token->kind = KILN_WORKER_TOKEN_GRAPH;
            token->flag = facade->graph;
            token->len = facade->generation;
            token->number = (double)facade->node;
            break;
        }
        case KILN_VALUE_ARRAY_BUFFER: {
            kiln_typed_buffer *buffer = kiln_c99_typed_buffer_of(item);
            if (buffer->shared != NULL) {
                token->kind = KILN_WORKER_TOKEN_SHARED;
                token->handle = buffer->shared;
                token->len = buffer->byte_length;
                break;
            }
            if (buffer->detached) {
                free(tokens);
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "An ArrayBuffer is detached and could not be cloned.");
            }
            if (copy) {
                token->kind = KILN_WORKER_TOKEN_BYTES;
                token->data = buffer->bytes;
                token->len = buffer->byte_length;
                break;
            }
            token->kind = KILN_WORKER_TOKEN_MOVED;
            token->len = buffer->byte_length;
            if (buffer->owns_bytes) {
                token->handle = buffer->bytes;
                if (buffer->moved_external)
                    kiln_c99_gc_unnote_moved_external(buffer->byte_length);
            } else {
                /* Bytes this runtime does not own (a borrowed `node:ffi` view) move as a copy. */
                uint8_t *block = (uint8_t *)malloc(buffer->byte_length == 0 ? 1 : buffer->byte_length);
                if (block == NULL) {
                    free(tokens);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                if (buffer->byte_length != 0) memcpy(block, buffer->bytes, buffer->byte_length);
                token->handle = block;
            }
            buffer->bytes = (uint8_t *)kiln_c99_detached_bytes;
            buffer->byte_length = 0;
            buffer->owns_bytes = 0;
            buffer->moved_external = 0;
            buffer->detached = 1;
            break;
        }
        default:
            free(tokens);
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "kiln: a message token must be a primitive or a buffer");
        }
    }
    *out = tokens;
    *out_count = count;
    return KILN_C99_OK;
}

/* This engine's value for one token of a message it received. A moved block or a shared store
 * is adopted, and cleared from the token so the message does not free it too. */
static int kiln_c99_worker_value(kiln_worker_token *token, kiln_value *out) {
    switch (token->kind) {
    case KILN_WORKER_TOKEN_NULL:
        *out = kiln_c99_null();
        return KILN_C99_OK;
    case KILN_WORKER_TOKEN_BOOL:
        *out = kiln_c99_boolean(token->flag != 0);
        return KILN_C99_OK;
    case KILN_WORKER_TOKEN_NUMBER:
        *out = kiln_c99_number(token->number);
        return KILN_C99_OK;
    case KILN_WORKER_TOKEN_STRING: {
        kiln_string *text;
        size_t width = token->flag != 0 ? 2u : 1u;
        if (token->len == 0) {
            *out = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)&kiln_c99_empty_string, 0);
            return KILN_C99_OK;
        }
        if (token->len > UINT32_MAX || token->data == NULL) return KILN_C99_INVALID_ARGUMENT;
        text = kiln_c99_gc_alloc_units((size_t)token->len,
                                       token->flag != 0 ? KILN_STRING_UTF16 : KILN_STRING_LATIN1);
        if (text == NULL) return KILN_C99_OUT_OF_MEMORY;
        memcpy((void *)(uintptr_t)text->data, token->data, (size_t)token->len * width);
        *out = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)text, text->unit_len);
        return KILN_C99_OK;
    }
    case KILN_WORKER_TOKEN_MOVED:
    case KILN_WORKER_TOKEN_SHARED: {
        kiln_typed_buffer *buffer;
        if (token->len > SIZE_MAX) return KILN_C99_INVALID_ARGUMENT;
        buffer = (kiln_typed_buffer *)kiln_c99_gc_alloc(KILN_GC_TYPED_BUFFER, sizeof *buffer);
        if (buffer == NULL) return KILN_C99_OUT_OF_MEMORY;
        buffer->byte_length = (size_t)token->len;
        if (token->kind == KILN_WORKER_TOKEN_MOVED) {
            buffer->bytes = token->handle != NULL ? (uint8_t *)token->handle
                                                  : (uint8_t *)kiln_c99_detached_bytes;
            buffer->owns_bytes = token->handle != NULL;
            buffer->moved_external = token->handle != NULL;
            if (buffer->moved_external)
                kiln_c99_gc_note_moved_external(buffer->byte_length);
        } else {
            buffer->bytes = (uint8_t *)(uintptr_t)token->data;
            buffer->owns_bytes = 0;
            buffer->shared = token->handle;
            buffer->shared_capacity = token->number > 0 ? (size_t)token->number : buffer->byte_length;
            buffer->shared_growable = token->flag != 0;
        }
        token->handle = NULL;
        *out = kiln_c99_value(KILN_VALUE_ARRAY_BUFFER, (uint64_t)(uintptr_t)buffer, NULL, 0);
        return KILN_C99_OK;
    }
    case KILN_WORKER_TOKEN_BYTES: {
        int status;
        if (token->len > SIZE_MAX) return KILN_C99_INVALID_ARGUMENT;
        status = kiln_c99_new_typed_buffer_ex(out, (size_t)token->len, 0);
        if (status == KILN_C99_OK && token->len != 0)
            memcpy(kiln_c99_typed_buffer_of(*out)->bytes, token->data, (size_t)token->len);
        return status;
    }
    case KILN_WORKER_TOKEN_GRAPH:
        if (!isfinite(token->number) || token->number < 0.0 ||
            token->number > (double)UINT32_MAX || floor(token->number) != token->number ||
            token->len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
        return kiln_c99_new_shared_facade(token->flag, (uint32_t)token->len,
                                           (uint32_t)token->number, out);
    default:
        /* The token that stands for `undefined` in a message: a value, not a missing member. */
        *out = kiln_c99_undefined();
        break;
    }
    return KILN_C99_OK;
}

static int kiln_c99_worker_post(uint32_t op, uint64_t port, uint32_t kind, kiln_value list,
                                kiln_value *result, kiln_value *thrown) {
    kiln_worker_token *tokens;
    size_t count;
    int posted;
    int status = kiln_c99_worker_tokens(list, op == KILN_C99_WORKER_OP_BROADCAST_POST, &tokens,
                                        &count, thrown);
    if (status != KILN_C99_OK) return status;
    posted = kiln_worker_post(op, port, kind, tokens, (uint64_t)count);
    /* A moved block the provider refused is still this call's to free. */
    if (posted < 0) {
        size_t index;
        for (index = 0; index < count; ++index)
            if (tokens[index].kind == KILN_WORKER_TOKEN_MOVED && tokens[index].handle != NULL)
                free(tokens[index].handle);
    }
    free(tokens);
    if (posted < 0) KILN_C99_REFUSE("a message the runtime's ports would not carry");
    *result = kiln_c99_boolean(posted > 0);
    return KILN_C99_OK;
}

static int kiln_c99_worker_take(uint64_t port, kiln_value *result) {
    uint32_t kind = 0;
    kiln_worker_token *tokens = NULL;
    uint64_t count = 0, index;
    void *message = NULL;
    kiln_value list, pair, item;
    int status = kiln_worker_take(port, &kind, &tokens, &count, &message);
    if (status == 0) {
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (status < 0) KILN_C99_REFUSE("a message the runtime's ports would not hand over");
    status = kiln_c99_decode_message_data(tokens, (size_t)count, &item);
    if (status == KILN_C99_OK) {
        kiln_worker_message_free(message);
        status = kiln_c99_new_array(&pair);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, kiln_c99_number((double)kind));
        if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, item);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, kiln_c99_boolean(1));
        if (status == KILN_C99_OK) *result = pair;
        return status;
    }
    if (status != KILN_C99_CLONE_BAIL) {
        kiln_worker_message_free(message);
        return status;
    }
    status = kiln_c99_new_array(&list);
    for (index = 0; status == KILN_C99_OK && index < count; ++index) {
        status = kiln_c99_worker_value(&tokens[index], &item);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(list, item);
    }
    kiln_worker_message_free(message);
    if (status == KILN_C99_OK) status = kiln_c99_new_array(&pair);
    if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, kiln_c99_number((double)kind));
    if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, list);
    if (status == KILN_C99_OK) *result = pair;
    return status;
}

/* `OP_SPAWN(entry, isEval, argv, port, control, info)`: `[key, threadId]` once the worker's
 * thread has started. */
static int kiln_c99_worker_spawn(const kiln_value *args, size_t argc, kiln_value *result,
                                 kiln_value *thrown) {
    kiln_value entry = argc > 1 ? args[1] : kiln_c99_undefined();
    kiln_value argv = argc > 3 ? args[3] : kiln_c99_undefined();
    kiln_value info = argc > 6 ? args[6] : kiln_c99_undefined();
    kiln_string *arguments = NULL;
    kiln_array *items;
    uint64_t key = 0, thread = 0;
    size_t index, count = 0;
    kiln_value pair;
    int status;
    if (entry.tag != KILN_VALUE_STRING || info.tag != KILN_VALUE_STRING || argv.tag != KILN_VALUE_ARRAY)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "kiln: a worker needs its entry, argv and description");
    items = kiln_c99_array_of(argv);
    count = items->count;
    arguments = (kiln_string *)calloc(count == 0 ? 1 : count, sizeof *arguments);
    if (arguments == NULL) return KILN_C99_OUT_OF_MEMORY;
    for (index = 0; index < count; ++index) {
        kiln_value item = kiln_val_to_boundary(items->elements[index]);
        if (item.tag == KILN_VALUE_STRING && item.ptr != NULL) arguments[index] = *(const kiln_string *)item.ptr;
        else arguments[index] = kiln_c99_empty_string;
    }
    status = kiln_worker_spawn((const kiln_string *)entry.ptr, argc > 2 && kiln_c99_truthy(args[2]) ? 1 : 0,
                               arguments, (uint64_t)count, kiln_c99_worker_word(args, argc, 4),
                               kiln_c99_worker_word(args, argc, 5), (const kiln_string *)info.ptr,
                               &key, &thread);
    free(arguments);
    if (status == -2)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR,
                                    "kiln: this program cannot start workers (it was built without a launcher that runs them)");
    if (status != 0)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR, "kiln: could not start a worker");
    status = kiln_c99_new_array(&pair);
    if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, kiln_c99_number((double)key));
    if (status == KILN_C99_OK) status = kiln_c99_array_push(pair, kiln_c99_number((double)thread));
    if (status == KILN_C99_OK) *result = pair;
    return status;
}

static int kiln_c99_worker_op_raw(kiln_value *result, const kiln_value *args, size_t argc,
                                  kiln_value *thrown) {
    uint64_t op = kiln_c99_worker_word(args, argc, 0);
    uint64_t first = kiln_c99_worker_word(args, argc, 1);
    uint64_t words[4] = {0, 0, 0, 0};
    kiln_value built;
    int status;
    switch (op) {
    case KILN_C99_WORKER_OP_CHANNEL:
        if (kiln_worker_op((uint32_t)op, 0, words) != 0) KILN_C99_REFUSE("a channel the runtime would not make");
        status = kiln_c99_new_array(&built);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(built, kiln_c99_number((double)words[0]));
        if (status == KILN_C99_OK) status = kiln_c99_array_push(built, kiln_c99_number((double)words[1]));
        if (status == KILN_C99_OK) *result = built;
        return status;
    case KILN_C99_WORKER_OP_POST:
    case KILN_C99_WORKER_OP_ENQUEUE:
        return kiln_c99_worker_post((uint32_t)op, first, (uint32_t)kiln_c99_worker_word(args, argc, 2),
                                    argc > 3 ? args[3] : kiln_c99_undefined(), result, thrown);
    case KILN_C99_WORKER_OP_POST_PLAIN:
        return kiln_c99_worker_post_plain(first, argc > 2 ? args[2] : kiln_c99_undefined(), result);
    case KILN_C99_WORKER_OP_ACTOR_REQUEST_ID:
        if (kiln_worker_op((uint32_t)op, 0, words) != 0) return KILN_C99_INVALID_ARGUMENT;
        *result = kiln_c99_number((double)words[0]);
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_ACTOR_ISOLATE: {
        kiln_c99_realm_state *realm = kiln_c99_realm();
        if (argc < 3 || args[1].tag != KILN_VALUE_OBJECT) return KILN_C99_INVALID_ARGUMENT;
        if (realm->isolated_graphs.tag != KILN_VALUE_COLLECTION) {
            kiln_value registry;
            status = kiln_c99_new_collection(&registry, 1, 1);
            if (status != KILN_C99_OK) return status;
            realm->isolated_graphs = registry;
        }
        status = kiln_c99_collection_put(realm->isolated_graphs, args[1], args[2]);
        if (status != KILN_C99_OK) return status;
        /* Any registry write while an opaque request waits makes its prior lookup stale.
         * Refuse that request at the turn boundary, even if this was another handle. */
        if (realm->isolated_request_id != 0) realm->isolated_request_invalidated = 1;
        *result = kiln_c99_boolean(1);
        return KILN_C99_OK;
    }
    case KILN_C99_WORKER_OP_ACTOR_ISOLATED_HAS: {
        kiln_c99_realm_state *realm = kiln_c99_realm();
        int found = argc >= 2 && args[1].tag == KILN_VALUE_OBJECT &&
                    realm->isolated_graphs.tag == KILN_VALUE_COLLECTION &&
                    kiln_c99_collection_find(kiln_c99_collection_of(realm->isolated_graphs),
                                             args[1]) >= 0.0;
        *result = kiln_c99_boolean(found);
        return KILN_C99_OK;
    }
    case KILN_C99_WORKER_OP_ACTOR_CANDIDATE_BEGIN:
        *result = kiln_c99_boolean(kiln_c99_actor_candidate_begin(kiln_c99_gc_heap()));
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_ACTOR_CANDIDATE_END:
        *result = kiln_c99_boolean(kiln_c99_actor_candidate_end(
            kiln_c99_gc_heap(), argc > 1 && kiln_c99_truthy(args[1])));
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_ACTOR_BIND_RECEIVER:
        words[0] = 0;
        if (kiln_worker_op((uint32_t)op, first, words) != 0) return KILN_C99_INVALID_ARGUMENT;
        *result = kiln_c99_boolean(words[0] != 0);
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_ACTOR_GRAPH_RESERVE:
        words[0] = kiln_c99_worker_word(args, argc, 2);
        words[1] = kiln_c99_worker_word(args, argc, 3);
        if (first == 0 || words[0] == 0 || words[1] == 0 ||
            kiln_worker_op((uint32_t)op, first, words) != 0) {
            *result = kiln_c99_number(0);
            return KILN_C99_OK;
        }
        *result = kiln_c99_number((double)words[0]);
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_ACTOR_GRAPH_RECEIVE: {
        uint64_t call = kiln_c99_worker_word(args, argc, 2);
        uint64_t reply = kiln_c99_worker_word(args, argc, 3);
        kiln_c99_realm_state *realm = kiln_c99_realm();
        kiln_value received = kiln_c99_undefined();
        int state = -1;
        if (first != 0 && call != 0 && reply != 0) {
            words[0] = call;
            words[1] = reply;
            if (kiln_worker_op((uint32_t)op, first, words) == 0)
                state = (int)(int64_t)words[0];
        }
        if (state == 1) {
            if (realm->graph_inbox_state != KILN_C99_GRAPH_INBOX_PUBLISHED ||
                realm->graph_inbox_reservation_id != first ||
                realm->graph_inbox_root.tag == KILN_VALUE_UNDEFINED) {
                state = -1;
            } else {
                received = realm->graph_inbox_root;
            }
        }
        status = kiln_c99_new_array(&built);
        if (status == KILN_C99_OK)
            status = kiln_c99_array_push(built, kiln_c99_number((double)state));
        if (status == KILN_C99_OK)
            status = kiln_c99_array_push(built, received);
        if (status == KILN_C99_OK && state == 1) {
            /* The native inbox keeps the moved graph rooted across array construction. Once
             * the value is in the result, release the matching host grant before clearing
             * that root. A failed release leaves both inbox and root intact for retry. */
            words[0] = 0;
            if (kiln_worker_op(KILN_C99_WORKER_OP_ACTOR_GRAPH_RELEASE, first, words) != 0 ||
                words[0] == 0) return KILN_C99_INVALID_ARGUMENT;
            realm->graph_inbox_state = KILN_C99_GRAPH_INBOX_EMPTY;
            realm->graph_inbox_reservation_id = 0;
            realm->graph_inbox_reserved_chunks = 0;
            realm->graph_inbox_region_count = 0;
            realm->graph_inbox_epoch = 0;
            realm->graph_inbox_root = kiln_c99_undefined();
        }
        if (status == KILN_C99_OK) *result = built;
        return status;
    }
    case KILN_C99_WORKER_OP_ACTOR_GRAPH_RELEASE: {
        kiln_c99_realm_state *realm = kiln_c99_realm();
        words[0] = 0;
        if (first == 0 || kiln_worker_op((uint32_t)op, first, words) != 0) {
            *result = kiln_c99_boolean(0);
            return KILN_C99_OK;
        }
        if (words[0] != 0 && realm->graph_inbox_reservation_id == first &&
            realm->graph_inbox_state != KILN_C99_GRAPH_INBOX_EMPTY) {
            realm->graph_inbox_state = KILN_C99_GRAPH_INBOX_EMPTY;
            realm->graph_inbox_reservation_id = 0;
            realm->graph_inbox_reserved_chunks = 0;
            realm->graph_inbox_region_count = 0;
            realm->graph_inbox_epoch = 0;
            realm->graph_inbox_root = kiln_c99_undefined();
        }
        *result = kiln_c99_boolean(words[0] != 0);
        return KILN_C99_OK;
    }
    case KILN_C99_WORKER_OP_ACTOR_GRAPH_REQUEST: {
        uint64_t request = 0;
        kiln_gc_heap *heap = kiln_c99_gc_heap();
        if (argc < 3 || args[2].tag != KILN_VALUE_OBJECT ||
            !kiln_c99_actor_isolated_request_begin(heap, args[2], &request)) {
            *result = kiln_c99_number(0);
            return KILN_C99_OK;
        }
        /* JavaScript will poll this opaque ID as a Number; a rounded ID could cancel a newer
         * request, so refuse before posting it to the scheduler. */
        words[0] = request;
        words[1] = kiln_c99_worker_word(args, argc, 3);
        words[2] = kiln_c99_worker_word(args, argc, 4);
        words[3] = kiln_c99_worker_word(args, argc, 5);
        if (request > 9007199254740991ULL ||
            kiln_worker_op((uint32_t)op, first, words) != 0 || words[0] == 0) {
            (void)kiln_c99_actor_isolated_request_rollback(heap, request);
            *result = kiln_c99_number(0);
            return KILN_C99_OK;
        }
        *result = kiln_c99_number((double)request);
        return KILN_C99_OK;
    }
    case KILN_C99_WORKER_OP_ACTOR_GRAPH_STATUS:
        if (first == 0 || kiln_worker_op((uint32_t)op, first, words) != 0)
            return KILN_C99_INVALID_ARGUMENT;
        if (words[0] != 0) {
            kiln_gc_heap *heap = kiln_c99_gc_heap();
            /* A paused sender probe may already have restored this request's escrow. Never
             * roll back a different request created after that turn boundary. */
            if (heap->realm.isolated_request_id != 0 &&
                (heap->realm.isolated_request_id != first ||
                 !kiln_c99_actor_isolated_request_rollback(heap, first)))
                    return KILN_C99_INVALID_ARGUMENT;
            (void)kiln_worker_op(KILN_C99_WORKER_OP_ACTOR_GRAPH_CANCEL, first, NULL);
        }
        *result = kiln_c99_number((double)words[0]);
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_ACTOR_GRAPH_CANCEL:
        if (first == 0 || !kiln_c99_actor_isolated_request_rollback(kiln_c99_gc_heap(), first)) {
            *result = kiln_c99_boolean(0);
            return KILN_C99_OK;
        }
        (void)kiln_worker_op((uint32_t)op, first, NULL);
        *result = kiln_c99_boolean(1);
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_ACTOR_REGISTER:
    case KILN_C99_WORKER_OP_ACTOR_LOOKUP:
    case KILN_C99_WORKER_OP_ACTOR_CLOSE: {
        kiln_shared_facade *address;
        kiln_shared_facade *owner;
        uint64_t packed;
        if (argc < 2 || args[1].tag != KILN_VALUE_SHARED || args[1].bits == 0) {
            *result = kiln_c99_number(0);
            return KILN_C99_OK;
        }
        address = (kiln_shared_facade *)(uintptr_t)args[1].bits;
        if (address->lease == 0) {
            *result = kiln_c99_number(0);
            return KILN_C99_OK;
        }
        packed = ((uint64_t)address->graph << 32) | (uint64_t)address->generation;
        words[0] = address->node;
        words[1] = kiln_c99_worker_word(args, argc, 2);
        if (op != KILN_C99_WORKER_OP_ACTOR_LOOKUP) {
            size_t owner_index = op == KILN_C99_WORKER_OP_ACTOR_REGISTER ? 3 : 2;
            if (argc <= owner_index || args[owner_index].tag != KILN_VALUE_SHARED || args[owner_index].bits == 0) {
                *result = kiln_c99_number(0);
                return KILN_C99_OK;
            }
            owner = (kiln_shared_facade *)(uintptr_t)args[owner_index].bits;
            if (owner->lease == 0) {
                *result = kiln_c99_number(0);
                return KILN_C99_OK;
            }
            words[2] = ((uint64_t)owner->graph << 32) | (uint64_t)owner->generation;
            words[3] = owner->node;
        }
        if (kiln_worker_op((uint32_t)op, packed, words) != 0) words[0] = 0;
        *result = kiln_c99_number((double)words[0]);
        return KILN_C99_OK;
    }
    case KILN_C99_WORKER_OP_BROADCAST_POST:
        return kiln_c99_worker_post((uint32_t)op, first, 0, argc > 2 ? args[2] : kiln_c99_undefined(),
                                    result, thrown);
    case KILN_C99_WORKER_OP_TAKE:
        return kiln_c99_worker_take(first, result);
    case KILN_C99_WORKER_OP_CLOSE:
    case KILN_C99_WORKER_OP_REAP:
        (void)kiln_worker_op((uint32_t)op, first, NULL);
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_STATE:
    case KILN_C99_WORKER_OP_TERMINATE:
        (void)kiln_worker_op((uint32_t)op, first, words);
        *result = op == KILN_C99_WORKER_OP_STATE ? kiln_c99_number((double)words[0])
                                                 : kiln_c99_boolean(words[0] != 0);
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_BROADCAST_JOIN: {
        kiln_value name = argc > 1 ? kiln_c99_text(args[1]) : kiln_c99_static_text("", 0);
        if (name.tag != KILN_VALUE_STRING || kiln_worker_broadcast_join((const kiln_string *)name.ptr, words) != 0)
            KILN_C99_REFUSE("a broadcast group the runtime would not join");
        *result = kiln_c99_number((double)words[0]);
        return KILN_C99_OK;
    }
    case KILN_C99_WORKER_OP_SPAWN:
        return kiln_c99_worker_spawn(args, argc, result, thrown);
    case KILN_C99_WORKER_OP_SELF: {
        uint64_t thread = 0, port = 0, control = 0, info_len = 0;
        const uint8_t *info = NULL;
        if (kiln_worker_self(&thread, &port, &control, &info, &info_len) != 1) {
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        status = kiln_c99_new_array(&built);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(built, kiln_c99_number((double)thread));
        if (status == KILN_C99_OK) status = kiln_c99_array_push(built, kiln_c99_number((double)port));
        if (status == KILN_C99_OK) status = kiln_c99_array_push(built, kiln_c99_number((double)control));
        if (status == KILN_C99_OK)
            status = kiln_c99_array_push(built, kiln_c99_copy_text((const char *)info, (size_t)info_len));
        if (status == KILN_C99_OK) *result = built;
        return status;
    }
    case KILN_C99_WORKER_OP_CANCEL_WAIT:
        *result = kiln_c99_boolean(kiln_realm_cancel_active_request(first) == KILN_C99_OK);
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_STDIO: {
        uint8_t *bytes = NULL;
        size_t length = 0;
        kiln_value text = argc > 2 ? kiln_c99_text(args[2]) : kiln_c99_static_text("", 0);
        if (text.tag == KILN_VALUE_STRING &&
            kiln_c99_string_to_utf8_lossy(text, &bytes, &length) == KILN_C99_OK) {
            (void)kiln_worker_stdio((uint32_t)first, bytes, (uint64_t)length);
            free(bytes);
        }
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    case KILN_C99_WORKER_OP_REPORT_ERROR:
        return kiln_c99_worker_post((uint32_t)op, 0, 3, argc > 1 ? args[1] : kiln_c99_undefined(),
                                    result, thrown);
    case KILN_C99_WORKER_OP_SIGNAL_NUMBER:
        (void)kiln_worker_op((uint32_t)op, first, words);
        *result = kiln_c99_number((double)words[0]);
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_SIGNAL_LISTEN:
        /* The endpoint, or the errno negated. */
        (void)kiln_worker_op((uint32_t)op, first, words);
        *result = words[0] != 0 ? kiln_c99_number((double)words[0])
                                : kiln_c99_number(-(double)(words[1] != 0 ? words[1] : 22));
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_SIGNAL_UNHANDLED:
        (void)kiln_worker_op((uint32_t)op, first, NULL);
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    case KILN_C99_WORKER_OP_KILL: {
        /* `kill(pid, signal)`: 0, or the errno. The pid may be negative (a process group). */
        double pid = argc > 1 && args[1].tag == KILN_VALUE_NUMBER ? kiln_c99_as_number(args[1]) : 0;
        uint64_t packed = (uint64_t)(uint32_t)(int32_t)pid |
                          ((uint64_t)(uint32_t)kiln_c99_worker_word(args, argc, 2) << 32);
        if (kiln_worker_op((uint32_t)op, packed, words) != 0) words[0] = 22;
        *result = kiln_c99_number((double)words[0]);
        return KILN_C99_OK;
    }
    default:
        KILN_C99_REFUSE("an operation the runtime's ports and workers do not have");
    }
}

/* The exception ending a worker's realm, for its parent's `error` event: the tokens the shared
 * TypeScript reads (`web_events_streams.ts`: no ports, then the value) -- an error's name, message
 * and stack; a primitive as itself; any other object as an empty one. The graph launcher calls
 * this inside the worker's realm once its program has failed. */
static int kiln_c99_worker_report_error(kiln_value error) {
    kiln_value tokens, name, message, stack, ignored;
    int status = kiln_c99_new_array(&tokens);
    if (status == KILN_C99_OK) status = kiln_c99_array_push(tokens, kiln_c99_number(0));
    if (status != KILN_C99_OK) return status;
    switch (error.tag) {
    case KILN_VALUE_UNDEFINED:
        status = kiln_c99_array_push(tokens, kiln_c99_number(0));
        break;
    case KILN_VALUE_NULL:
        status = kiln_c99_array_push(tokens, kiln_c99_number(1));
        break;
    case KILN_VALUE_BOOLEAN:
        status = kiln_c99_array_push(tokens, kiln_c99_number(error.bits != 0 ? 3 : 2));
        break;
    case KILN_VALUE_NUMBER:
        status = kiln_c99_array_push(tokens, kiln_c99_number(4));
        if (status == KILN_C99_OK) status = kiln_c99_array_push(tokens, error);
        break;
    case KILN_VALUE_STRING:
        status = kiln_c99_array_push(tokens, kiln_c99_number(5));
        if (status == KILN_C99_OK) status = kiln_c99_array_push(tokens, error);
        break;
    default:
        if (kiln_c99_get(NULL, error, "message", 7, &message, 0, NULL) != KILN_C99_OK ||
            message.tag != KILN_VALUE_STRING) {
            status = kiln_c99_array_push(tokens, kiln_c99_number(8));
            if (status == KILN_C99_OK) status = kiln_c99_array_push(tokens, kiln_c99_number(0));
            break;
        }
        if (kiln_c99_get(NULL, error, "name", 4, &name, 0, NULL) != KILN_C99_OK ||
            name.tag != KILN_VALUE_STRING)
            name = kiln_c99_static_text("Error", 5);
        status = kiln_c99_array_push(tokens, kiln_c99_number(19));
        if (status == KILN_C99_OK) status = kiln_c99_array_push(tokens, name);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(tokens, message);
        if (status == KILN_C99_OK &&
            kiln_c99_get(NULL, error, "stack", 5, &stack, 0, NULL) == KILN_C99_OK &&
            stack.tag == KILN_VALUE_STRING) {
            status = kiln_c99_array_push(tokens, kiln_c99_number(5));
            if (status == KILN_C99_OK) status = kiln_c99_array_push(tokens, stack);
        } else if (status == KILN_C99_OK) {
            status = kiln_c99_array_push(tokens, kiln_c99_number(0));
        }
        if (status == KILN_C99_OK) status = kiln_c99_array_push(tokens, kiln_c99_number(0));
        break;
    }
    if (status != KILN_C99_OK) return status;
    return kiln_c99_worker_post(KILN_C99_WORKER_OP_REPORT_ERROR, 0, 3, tokens, &ignored, NULL);
}

static int kiln_c99_node_port_wait_complete(void *opaque, const kiln_async_completion *completion,
                                            void *host, void *thrown) {
    kiln_node_process_completion *work = (kiln_node_process_completion *)opaque;
    (void)completion;
    if (work == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (work->port_callback) {
        kiln_value ignored;
        return kiln_c99_call((kiln_host *)host, &ignored, work->promise, kiln_c99_undefined(),
                             NULL, 0, 0, (kiln_value *)thrown);
    }
    return kiln_c99_settle(work->promise, kiln_c99_undefined(), 0);
}

/* `__kilnPortWait(endpoint, referenced, callback)`: a native callback runs in the host I/O turn
 * once the endpoint is readable. The promise form remains for callers without a callback. */
static int kiln_c99_port_wait_raw(kiln_value *result, const kiln_value *args, size_t argc) {
    kiln_value promise, pair;
    kiln_node_process_completion *work;
    kiln_async_host_op operation = {0};
    uint64_t request = 0;
    int status;
    int direct = argc > 2 && kiln_c99_is_callable(args[2]);
    if (!direct && kiln_c99_new_promise(&promise) != KILN_C99_OK) return KILN_C99_OUT_OF_MEMORY;
    work = (kiln_node_process_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_PROCESS_COMPLETION, sizeof *work);
    if (work == NULL) return KILN_C99_OUT_OF_MEMORY;
    work->promise = direct ? args[2] : promise;
    work->port_callback = (uint8_t)direct;
    operation.operation = KILN_REALM_OP_PORT_WAIT;
    operation.flags = argc > 1 && kiln_c99_truthy(args[1]) ? 1u : 0u;
    operation.resource = kiln_c99_worker_word(args, argc, 0);
    status = kiln_realm_start_active_host_op(&operation, kiln_c99_node_port_wait_complete,
                                             kiln_c99_node_drop_process, work, &request);
    if (status != KILN_C99_OK) {
        kiln_c99_gc_release(work);
        return status;
    }
    if (direct) {
        *result = kiln_c99_number((double)request);
        return KILN_C99_OK;
    }
    if (kiln_c99_new_object(&pair) != KILN_C99_OK) {
        (void)kiln_realm_cancel_active_request(request);
        return KILN_C99_OUT_OF_MEMORY;
    }
    work->result = pair;
    status = kiln_c99_put(NULL, pair, "requestId", 9, kiln_c99_number((double)request), 0, NULL);
    if (status == KILN_C99_OK) status = kiln_c99_put(NULL, pair, "promise", 7, promise, 0, NULL);
    if (status != KILN_C99_OK) {
        (void)kiln_realm_cancel_active_request(request);
        return status;
    }
    *result = pair;
    return KILN_C99_OK;
}

static int kiln_c99_node_fs_encoding(kiln_host *host, kiln_value option, int *utf8,
                                     int depth, kiln_value *thrown) {
    kiln_value encoding;
    if (option.tag == KILN_VALUE_UNDEFINED || option.tag == KILN_VALUE_NULL) {
        *utf8 = 0;
        return KILN_C99_OK;
    }
    if (option.tag == KILN_VALUE_OBJECT) {
        int status = kiln_c99_get(host, option, "encoding", 8, &encoding, depth, thrown);
        if (status != KILN_C99_OK) return status;
        if (encoding.tag == KILN_VALUE_UNDEFINED || encoding.tag == KILN_VALUE_NULL) {
            *utf8 = 0;
            return KILN_C99_OK;
        }
    } else {
        encoding = option;
    }
    if (encoding.tag != KILN_VALUE_STRING)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "encoding must be a string");
    size_t encoding_len = 0;
    const char *encoding_bytes = kiln_c99_string_ascii_bytes(encoding, &encoding_len);
    if ((encoding_bytes != NULL && encoding_len == 4 && tolower((unsigned char)encoding_bytes[0]) == 'u' &&
         tolower((unsigned char)encoding_bytes[1]) == 't' &&
         tolower((unsigned char)encoding_bytes[2]) == 'f' &&
         encoding_bytes[3] == '8') ||
        (encoding_bytes != NULL && encoding_len == 5 && tolower((unsigned char)encoding_bytes[0]) == 'u' &&
         tolower((unsigned char)encoding_bytes[1]) == 't' &&
         tolower((unsigned char)encoding_bytes[2]) == 'f' && encoding_bytes[3] == '-' &&
         encoding_bytes[4] == '8')) {
        *utf8 = 1;
        return KILN_C99_OK;
    }
    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                "unsupported file encoding");
}

static int kiln_c99_node_fs(kiln_host *host, kiln_value *result, uint64_t code,
                            kiln_value receiver, const kiln_value *args, size_t argc,
                            int depth, kiln_value *thrown) {
    kiln_value path = argc > 0 ? args[0] : kiln_c99_undefined();
    kiln_value promise;
    kiln_value data;
    kiln_node_fs_completion *work;
    kiln_async_host_op operation;
    uint64_t request = 0;
    const uint8_t *bytes = NULL;
    size_t bytes_len = 0;
    uint8_t *path_owned = NULL;
    uint8_t *data_owned = NULL;
    size_t width;
    int utf8 = 0;
    int status;
    (void)receiver;
    if (code == KILN_C99_METHOD_NODE_STAT_IS_FILE ||
        code == KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY ||
        code == KILN_C99_METHOD_NODE_STAT_IS_SYMBOLIC_LINK) {
        kiln_property *marker;
        const char *key = code == KILN_C99_METHOD_NODE_STAT_IS_FILE
                              ? "@@kiln:node:is-file"
                              : (code == KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY
                                     ? "@@kiln:node:is-directory"
                                     : "@@kiln:node:is-symlink");
        size_t key_len = code == KILN_C99_METHOD_NODE_STAT_IS_FILE
                             ? 19
                             : (code == KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY ? 24 : 22);
        if (receiver.tag != KILN_VALUE_OBJECT)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Stats method called on an incompatible receiver");
        marker = kiln_c99_own_private(receiver, key, key_len);
        *result = kiln_c99_boolean(marker != NULL && kiln_c99_truthy(marker->value));
        return KILN_C99_OK;
    }
    if (path.tag != KILN_VALUE_STRING)
        return kiln_c99_throw_not_a(thrown, path, "string path");
    if (code == KILN_C99_METHOD_NODE_FS_READ_FILE) {
        status = kiln_c99_node_fs_encoding(host,
                                           argc > 1 ? args[1] : kiln_c99_undefined(), &utf8,
                                           depth, thrown);
        if (status != KILN_C99_OK) return status;
    } else if (code == KILN_C99_METHOD_NODE_FS_WRITE_FILE) {
        data = argc > 1 ? args[1] : kiln_c99_undefined();
        if (data.tag == KILN_VALUE_STRING) {
            status = kiln_c99_string_to_utf8_lossy(data, &data_owned, &bytes_len);
            if (status != KILN_C99_OK) return status;
            bytes = data_owned;
        } else if (data.tag == KILN_VALUE_TYPED_ARRAY) {
            kiln_typed_array *view = kiln_c99_typed_array_of(data);
            width = kiln_c99_typed_width(view->kind);
            if (view->length > SIZE_MAX / width) return KILN_C99_OUT_OF_MEMORY;
            bytes = view->buffer->bytes + view->byte_offset;
            bytes_len = view->length * width;
        } else {
            return kiln_c99_throw_not_a(thrown, data, "string or typed array data");
        }
    } else if (code == KILN_C99_METHOD_NODE_FS_MKDIR) {
        if (argc > 1 && args[1].tag == KILN_VALUE_OBJECT) {
            kiln_value rec_val;
            int rec_status = kiln_c99_get(host, args[1], "recursive", 9, &rec_val, depth, thrown);
            if (rec_status == KILN_C99_OK && kiln_c99_truthy(rec_val)) {
                width = 1; // reuse width variable as recursive flag
            }
        }
    }
    status = kiln_c99_new_promise(&promise);
    if (status != KILN_C99_OK) {
        free(data_owned);
        return status;
    }
    work = (kiln_node_fs_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_FS_COMPLETION, sizeof *work);
    if (work == NULL) {
        free(data_owned);
        return KILN_C99_OUT_OF_MEMORY;
    }
    work->promise = promise;
    work->operation = code == KILN_C99_METHOD_NODE_FS_READ_FILE
                          ? KILN_REALM_OP_READ_FILE
                          : code == KILN_C99_METHOD_NODE_FS_WRITE_FILE
                                ? KILN_REALM_OP_WRITE_FILE
                                : code == KILN_C99_METHOD_NODE_FS_MKDIR
                                      ? KILN_REALM_OP_MKDIR
                                      : KILN_REALM_OP_STAT;
    work->utf8 = utf8;
    memset(&operation, 0, sizeof operation);
    operation.operation = work->operation;
    operation.flags = 1;
    {
        size_t path_len = 0;
        status = kiln_c99_string_to_utf8_lossy(path, &path_owned, &path_len);
        operation.text_len = (uint64_t)path_len;
    }
    if (status != KILN_C99_OK) {
        free(data_owned);
        kiln_c99_gc_release(work);
        return status;
    }
    operation.text = path_owned;
    operation.bytes = bytes;
    operation.bytes_len = bytes_len;
    if (code == KILN_C99_METHOD_NODE_FS_MKDIR && width == 1) {
        operation.argument = 1;
    }
    status = kiln_realm_start_active_host_op(&operation, kiln_c99_node_fs_complete,
                                             kiln_c99_node_drop_fs_completion, work, &request);
    if (status != KILN_C99_OK) {
        free(path_owned);
        free(data_owned);
        kiln_c99_gc_release(work);
        return status;
    }
    free(path_owned);
    free(data_owned);
    *result = promise;
    return KILN_C99_OK;
}

static int kiln_c99_throw_node_fs_error(kiln_value *thrown, const char *syscall, size_t syscall_len,
                                       const char *path_str, size_t path_len,
                                       int32_t error_code) {
    const char *code_str = "EIO";
    size_t code_len = 3;
    const char *msg_reason = "input/output error";
    size_t reason_len = 18;
    if (error_code == 1) {
        code_str = "ENOENT";
        code_len = 6;
        msg_reason = "no such file or directory";
        reason_len = 25;
    } else if (error_code == 2) {
        code_str = "EEXIST";
        code_len = 6;
        msg_reason = "file already exists";
        reason_len = 19;
    } else if (error_code == 3) {
        code_str = "EACCES";
        code_len = 6;
        msg_reason = "permission denied";
        reason_len = 17;
    } else if (error_code == 4) {
        code_str = "EBADF";
        code_len = 5;
        msg_reason = "bad file descriptor";
        reason_len = 19;
    }
    char buf[1024];
    size_t len = 0;
    if (code_len + 2 + reason_len + 2 + syscall_len + (path_str != NULL && path_len > 0 ? (3 + path_len) : 0) < sizeof(buf)) {
        memcpy(buf + len, code_str, code_len); len += code_len;
        memcpy(buf + len, ": ", 2); len += 2;
        memcpy(buf + len, msg_reason, reason_len); len += reason_len;
        memcpy(buf + len, ", ", 2); len += 2;
        memcpy(buf + len, syscall, syscall_len); len += syscall_len;
        if (path_str != NULL && path_len > 0) {
            memcpy(buf + len, " '", 2); len += 2;
            memcpy(buf + len, path_str, path_len); len += path_len;
            buf[len++] = '\'';
        }
    }
    kiln_value err_val = kiln_c99_undefined();
    int status = kiln_c99_new_error(&err_val, KILN_C99_BUILTIN_ERROR,
                                    kiln_c99_copy_text(buf, len));
    if (status != KILN_C99_OK) return status;
    kiln_c99_node_fs_error_property(err_val, "code", 4, (const uint8_t *)code_str, code_len);
    kiln_c99_node_fs_error_property(err_val, "syscall", 7, (const uint8_t *)syscall, syscall_len);
    if (path_str != NULL && path_len > 0) {
        kiln_c99_node_fs_error_property(err_val, "path", 4, (const uint8_t *)path_str, path_len);
    }
    if (thrown != NULL) *thrown = err_val;
    return KILN_C99_THROWN;
}

/* A private descriptor transport for node:fs's TypeScript open/write/close/fsync methods.
 * The module validates overloads and arguments; this primitive only moves bytes to an OS fd.
 * The fd is process-wide, as Node's is, so a worker may write while its parent runs. */
static int kiln_c99_fs_descriptor_raw(kiln_value *result, const kiln_value *args, size_t argc,
                                       kiln_value *thrown) {
#if defined(_WIN32)
    (void)result; (void)args; (void)argc; (void)thrown;
    KILN_C99_REFUSE("file descriptor I/O on Windows");
#else
    int op, fd, failed, error_code;
    if (argc < 2 || args[0].tag != KILN_VALUE_NUMBER)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "invalid file descriptor operation");
    op = (int)kiln_c99_as_number(args[0]);
    if (op == 1) {
        static const int flags[] = {
            O_RDONLY, O_RDWR, O_WRONLY | O_CREAT | O_TRUNC,
            O_WRONLY | O_CREAT | O_EXCL, O_RDWR | O_CREAT | O_TRUNC,
            O_RDWR | O_CREAT | O_EXCL, O_WRONLY | O_CREAT | O_APPEND,
            O_WRONLY | O_CREAT | O_APPEND | O_EXCL, O_RDWR | O_CREAT | O_APPEND,
            O_RDWR | O_CREAT | O_APPEND | O_EXCL
        };
        uint8_t *path_bytes = NULL;
        size_t path_len = 0;
        char *path;
        int which, mode, status;
        if (args[1].tag != KILN_VALUE_STRING || argc < 4 ||
            args[2].tag != KILN_VALUE_NUMBER || args[3].tag != KILN_VALUE_NUMBER)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "invalid file open arguments");
        which = (int)kiln_c99_as_number(args[2]);
        if (which < 0 || (size_t)which >= sizeof(flags) / sizeof(flags[0]))
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "invalid file open flags");
        mode = (int)kiln_c99_as_number(args[3]);
        status = kiln_c99_string_to_utf8_lossy(args[1], &path_bytes, &path_len);
        if (status != KILN_C99_OK) return status;
        path = (char *)malloc(path_len + 1);
        if (path == NULL) { free(path_bytes); return KILN_C99_OUT_OF_MEMORY; }
        memcpy(path, path_bytes, path_len);
        path[path_len] = '\0';
        fd = open(path, flags[which], (mode_t)mode);
        error_code = errno;
        free(path_bytes);
        if (fd < 0) {
            status = kiln_c99_throw_node_fs_error(thrown, "open", 4, path, path_len,
                      error_code == ENOENT ? 1 : error_code == EEXIST ? 2 :
                      error_code == EACCES ? 3 : error_code == EBADF ? 4 : 0);
            free(path);
            return status;
        }
        free(path);
        *result = kiln_c99_number((double)fd);
        return KILN_C99_OK;
    }
    if (args[1].tag != KILN_VALUE_NUMBER)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "invalid file descriptor");
    fd = (int)kiln_c99_as_number(args[1]);
    if (op == 2) {
        kiln_typed_array *view;
        const uint8_t *bytes;
        size_t length;
        ssize_t written;
        double position;
        if (argc < 4 || args[2].tag != KILN_VALUE_TYPED_ARRAY ||
            args[3].tag != KILN_VALUE_NUMBER)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "invalid file write arguments");
        view = kiln_c99_typed_array_of(args[2]);
        if (view->kind != KILN_TYPED_UINT8)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "file write requires bytes");
        bytes = view->buffer->bytes + view->byte_offset;
        length = view->length;
        position = kiln_c99_as_number(args[3]);
        written = position < 0 ? write(fd, bytes, length)
                               : pwrite(fd, bytes, length, (off_t)position);
        if (written < 0) {
            error_code = errno;
            return kiln_c99_throw_node_fs_error(thrown, "write", 5, NULL, 0,
                   error_code == EBADF ? 4 : error_code == EACCES ? 3 : 0);
        }
        *result = kiln_c99_number((double)written);
        return KILN_C99_OK;
    }
    if (op == 3) failed = close(fd);
    else if (op == 4) failed = fsync(fd);
    else return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                     "invalid file descriptor operation");
    if (failed != 0) {
        error_code = errno;
        return kiln_c99_throw_node_fs_error(thrown, op == 3 ? "close" : "fsync",
               op == 3 ? 5 : 5, NULL, 0, error_code == EBADF ? 4 : 0);
    }
    *result = kiln_c99_undefined();
    return KILN_C99_OK;
#endif
}

static int kiln_c99_node_fs_sync(kiln_host *host, kiln_value *result, uint64_t code,
                                 kiln_value receiver, const kiln_value *args, size_t argc,
                                 int depth, kiln_value *thrown) {
    kiln_value path_val = argc > 0 ? args[0] : kiln_c99_undefined();
    uint8_t *path_bytes = NULL;
    size_t path_len = 0;
    int status;
    (void)receiver;

    if (code == KILN_C99_METHOD_NODE_STAT_IS_FILE ||
        code == KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY ||
        code == KILN_C99_METHOD_NODE_STAT_IS_SYMBOLIC_LINK) {
        kiln_property *marker;
        const char *key = code == KILN_C99_METHOD_NODE_STAT_IS_FILE
                              ? "@@kiln:node:is-file"
                              : (code == KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY
                                     ? "@@kiln:node:is-directory"
                                     : "@@kiln:node:is-symlink");
        size_t key_len = code == KILN_C99_METHOD_NODE_STAT_IS_FILE
                             ? 19
                             : (code == KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY ? 24 : 22);
        if (receiver.tag != KILN_VALUE_OBJECT)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Stats method called on an incompatible receiver");
        marker = kiln_c99_own_private(receiver, key, key_len);
        *result = kiln_c99_boolean(marker != NULL && kiln_c99_truthy(marker->value));
        return KILN_C99_OK;
    }

    if (path_val.tag != KILN_VALUE_STRING) {
        path_val = kiln_c99_text(path_val);
    }
    if (path_val.tag != KILN_VALUE_STRING) {
        return kiln_c99_throw_not_a(thrown, path_val, "string path");
    }
    status = kiln_c99_string_to_utf8_lossy(path_val, &path_bytes, &path_len);
    if (status != KILN_C99_OK) return status;

    if (code == KILN_C99_METHOD_NODE_FS_READ_FILE_SYNC) {
        int utf8 = 0;
        uint8_t *out_bytes = NULL;
        uint64_t out_len = 0;
        int32_t err_code = 0;
        if (argc > 1) {
            status = kiln_c99_node_fs_encoding(host, args[1], &utf8, depth, thrown);
            if (status != KILN_C99_OK) {
                free(path_bytes);
                return status;
            }
        }
        kiln_node_fs_read_file_sync(path_bytes, (uint64_t)path_len, &out_bytes, &out_len, &err_code);
        if (err_code != 0) {
            status = kiln_c99_throw_node_fs_error(thrown, "open", 4, (const char *)path_bytes, path_len, err_code);
            free(path_bytes);
            return status;
        }
        if (utf8) {
            *result = kiln_c99_copy_text((const char *)out_bytes, (size_t)out_len);
        } else {
            status = kiln_c99_buffer_from_bytes(result, out_bytes, out_len);
        }
        kiln_node_fs_free_bytes(out_bytes, out_len);
        free(path_bytes);
        return status;
    }

    if (code == KILN_C99_METHOD_NODE_FS_WRITE_FILE_SYNC) {
        kiln_value data_val = argc > 1 ? args[1] : kiln_c99_undefined();
        const uint8_t *bytes = NULL;
        size_t bytes_len = 0;
        uint8_t *data_owned = NULL;
        int32_t err_code = 0;

        if (data_val.tag == KILN_VALUE_STRING) {
            status = kiln_c99_string_to_utf8_lossy(data_val, &data_owned, &bytes_len);
            if (status != KILN_C99_OK) {
                free(path_bytes);
                return status;
            }
            bytes = data_owned;
        } else if (data_val.tag == KILN_VALUE_TYPED_ARRAY) {
            kiln_typed_array *view = kiln_c99_typed_array_of(data_val);
            size_t width = kiln_c99_typed_width(view->kind);
            if (view->length > SIZE_MAX / width) {
                free(path_bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            bytes = view->buffer->bytes + view->byte_offset;
            bytes_len = view->length * width;
        } else {
            free(path_bytes);
            return kiln_c99_throw_not_a(thrown, data_val, "string or typed array data");
        }

        kiln_node_fs_write_file_sync(path_bytes, (uint64_t)path_len, bytes, (uint64_t)bytes_len, &err_code);
        free(data_owned);
        if (err_code != 0) {
            status = kiln_c99_throw_node_fs_error(thrown, "open", 4, (const char *)path_bytes, path_len, err_code);
            free(path_bytes);
            return status;
        }
        free(path_bytes);
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_NODE_FS_MKDIR_SYNC) {
        int recursive = 0;
        int32_t err_code = 0;
        if (argc > 1 && args[1].tag == KILN_VALUE_OBJECT) {
            kiln_value rec_val;
            int rec_status = kiln_c99_get(host, args[1], "recursive", 9, &rec_val, depth, thrown);
            if (rec_status == KILN_C99_OK && kiln_c99_truthy(rec_val)) {
                recursive = 1;
            }
        }
        kiln_node_fs_mkdir_sync(path_bytes, (uint64_t)path_len, recursive, &err_code);
        if (err_code != 0) {
            status = kiln_c99_throw_node_fs_error(thrown, "mkdir", 5, (const char *)path_bytes, path_len, err_code);
            free(path_bytes);
            return status;
        }
        free(path_bytes);
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_NODE_FS_STAT_SYNC || code == KILN_C99_METHOD_NODE_FS_LSTAT_SYNC) {
        int follow = (code == KILN_C99_METHOD_NODE_FS_STAT_SYNC);
        int throw_if_no_entry = 1;
        uint64_t size = 0;
        uint32_t flags = 0;
        double mtime_ms = 0.0;
        uint32_t mode = 0;
        int32_t err_code = 0;

        if (argc > 1 && args[1].tag == KILN_VALUE_OBJECT) {
            kiln_value tne_val;
            int tne_status = kiln_c99_get(host, args[1], "throwIfNoEntry", 14, &tne_val, depth, thrown);
            if (tne_status == KILN_C99_OK && !kiln_c99_truthy(tne_val)) {
                throw_if_no_entry = 0;
            }
        }

        kiln_node_fs_stat_sync(path_bytes, (uint64_t)path_len, follow, &size, &flags, &mtime_ms, &mode, &err_code);
        if (err_code != 0) {
            if (!throw_if_no_entry && err_code == 1) {
                free(path_bytes);
                *result = kiln_c99_undefined();
                return KILN_C99_OK;
            }
            status = kiln_c99_throw_node_fs_error(thrown, follow ? "stat" : "lstat",
                                                 (size_t)(follow ? 4 : 5),
                                                 (const char *)path_bytes, path_len, err_code);
            free(path_bytes);
            return status;
        }
        free(path_bytes);

        kiln_value stat_obj;
        kiln_property *property;
        status = kiln_c99_new_object(&stat_obj);
        if (status != KILN_C99_OK) return status;

        kiln_c99_put(NULL, stat_obj, "size", 4, kiln_c99_number((double)size), 0, NULL);
        kiln_c99_put(NULL, stat_obj, "mtimeMs", 7, kiln_c99_number(mtime_ms), 0, NULL);
        kiln_c99_put(NULL, stat_obj, "ctimeMs", 7, kiln_c99_number(mtime_ms), 0, NULL);
        kiln_c99_put(NULL, stat_obj, "atimeMs", 7, kiln_c99_number(mtime_ms), 0, NULL);
        kiln_c99_put(NULL, stat_obj, "birthtimeMs", 11, kiln_c99_number(mtime_ms), 0, NULL);
        kiln_c99_put(NULL, stat_obj, "mode", 4, kiln_c99_number((double)mode), 0, NULL);

        kiln_value date_val;
        if (kiln_c99_new_date(&date_val, mtime_ms) == KILN_C99_OK) {
            kiln_c99_put(NULL, stat_obj, "mtime", 5, date_val, 0, NULL);
            kiln_c99_put(NULL, stat_obj, "ctime", 5, date_val, 0, NULL);
            kiln_c99_put(NULL, stat_obj, "atime", 5, date_val, 0, NULL);
            kiln_c99_put(NULL, stat_obj, "birthtime", 9, date_val, 0, NULL);
        }

        kiln_c99_own_slot(stat_obj, "@@kiln:node:is-file", 19, &property);
        if (property != NULL) {
            property->value = kiln_c99_boolean((flags & (UINT32_C(1) << 3)) != 0);
            property->enumerable = 0; property->writable = 0; property->configurable = 0;
        }
        kiln_c99_own_slot(stat_obj, "@@kiln:node:is-directory", 24, &property);
        if (property != NULL) {
            property->value = kiln_c99_boolean((flags & (UINT32_C(1) << 4)) != 0);
            property->enumerable = 0; property->writable = 0; property->configurable = 0;
        }
        kiln_c99_own_slot(stat_obj, "@@kiln:node:is-symlink", 22, &property);
        if (property != NULL) {
            property->value = kiln_c99_boolean((flags & (UINT32_C(1) << 5)) != 0);
            property->enumerable = 0; property->writable = 0; property->configurable = 0;
        }
        kiln_c99_object_of(stat_obj)->read_only_seen = 1;

        kiln_c99_put(NULL, stat_obj, "isFile", 6, kiln_c99_method(KILN_C99_METHOD_NODE_STAT_IS_FILE), 0, NULL);
        kiln_c99_put(NULL, stat_obj, "isDirectory", 11, kiln_c99_method(KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY), 0, NULL);
        kiln_c99_put(NULL, stat_obj, "isSymbolicLink", 14, kiln_c99_method(KILN_C99_METHOD_NODE_STAT_IS_SYMBOLIC_LINK), 0, NULL);

        *result = stat_obj;
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_NODE_FS_READDIR_SYNC) {
        int with_file_types = 0;
        uint8_t *entries = NULL;
        uint64_t entries_len = 0;
        uint64_t count = 0;
        int32_t err_code = 0;

        if (argc > 1 && args[1].tag == KILN_VALUE_OBJECT) {
            kiln_value wft_val;
            int wft_status = kiln_c99_get(host, args[1], "withFileTypes", 13, &wft_val, depth, thrown);
            if (wft_status == KILN_C99_OK && kiln_c99_truthy(wft_val)) {
                with_file_types = 1;
            }
        }

        kiln_node_fs_readdir_sync(path_bytes, (uint64_t)path_len, &entries, &entries_len, &count, &err_code);
        if (err_code != 0) {
            status = kiln_c99_throw_node_fs_error(thrown, "scandir", 7, (const char *)path_bytes, path_len, err_code);
            free(path_bytes);
            return status;
        }
        free(path_bytes);

        status = kiln_c99_new_array(result);
        if (status != KILN_C99_OK) {
            kiln_node_fs_free_bytes(entries, entries_len);
            return status;
        }

        uint8_t *p = entries;
        for (uint64_t i = 0; i < count; ++i) {
            uint16_t name_len = (uint16_t)p[0] | ((uint16_t)p[1] << 8);
            uint8_t flags = p[2];
            const char *name = (const char *)(p + 3);
            p += 3 + name_len;

            if (!with_file_types) {
                kiln_c99_array_push(*result, kiln_c99_copy_text(name, name_len));
            } else {
                kiln_value dirent;
                kiln_property *property;
                kiln_c99_new_object(&dirent);
                kiln_c99_put(NULL, dirent, "name", 4, kiln_c99_copy_text(name, name_len), 0, NULL);
                kiln_c99_own_slot(dirent, "@@kiln:node:is-file", 19, &property);
                if (property != NULL) {
                    property->value = kiln_c99_boolean((flags & 1) != 0);
                    property->enumerable = 0; property->writable = 0; property->configurable = 0;
                }
                kiln_c99_own_slot(dirent, "@@kiln:node:is-directory", 24, &property);
                if (property != NULL) {
                    property->value = kiln_c99_boolean((flags & 2) != 0);
                    property->enumerable = 0; property->writable = 0; property->configurable = 0;
                }
                kiln_c99_own_slot(dirent, "@@kiln:node:is-symlink", 22, &property);
                if (property != NULL) {
                    property->value = kiln_c99_boolean((flags & 4) != 0);
                    property->enumerable = 0; property->writable = 0; property->configurable = 0;
                }
                kiln_c99_object_of(dirent)->read_only_seen = 1;
                kiln_c99_put(NULL, dirent, "isFile", 6, kiln_c99_method(KILN_C99_METHOD_NODE_STAT_IS_FILE), 0, NULL);
                kiln_c99_put(NULL, dirent, "isDirectory", 11, kiln_c99_method(KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY), 0, NULL);
                kiln_c99_put(NULL, dirent, "isSymbolicLink", 14, kiln_c99_method(KILN_C99_METHOD_NODE_STAT_IS_SYMBOLIC_LINK), 0, NULL);
                kiln_c99_array_push(*result, dirent);
            }
        }
        kiln_node_fs_free_bytes(entries, entries_len);
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_NODE_FS_REALPATH_SYNC) {
        uint8_t *out_bytes = NULL;
        uint64_t out_len = 0;
        int32_t err_code = 0;
        kiln_node_fs_realpath_sync(path_bytes, (uint64_t)path_len, &out_bytes, &out_len, &err_code);
        if (err_code != 0) {
            status = kiln_c99_throw_node_fs_error(thrown, "realpath", 8, (const char *)path_bytes, path_len, err_code);
            free(path_bytes);
            return status;
        }
        *result = kiln_c99_copy_text((const char *)out_bytes, (size_t)out_len);
        kiln_node_fs_free_bytes(out_bytes, out_len);
        free(path_bytes);
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_NODE_FS_UNLINK_SYNC) {
        int32_t err_code = 0;
        kiln_node_fs_unlink_sync(path_bytes, (uint64_t)path_len, &err_code);
        if (err_code != 0) {
            status = kiln_c99_throw_node_fs_error(thrown, "unlink", 6, (const char *)path_bytes, path_len, err_code);
            free(path_bytes);
            return status;
        }
        free(path_bytes);
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }

    if (code == KILN_C99_METHOD_NODE_FS_RMDIR_SYNC) {
        int32_t err_code = 0;
        kiln_node_fs_rmdir_sync(path_bytes, (uint64_t)path_len, &err_code);
        if (err_code != 0) {
            status = kiln_c99_throw_node_fs_error(thrown, "rmdir", 5, (const char *)path_bytes, path_len, err_code);
            free(path_bytes);
            return status;
        }
        free(path_bytes);
        *result = kiln_c99_undefined();
        return KILN_C99_OK;
    }

    free(path_bytes);
    return KILN_C99_INVALID_ARGUMENT;
}

static int kiln_c99_node_net(kiln_host *host, kiln_value *result, uint64_t code,
                             kiln_value receiver, const kiln_value *args, size_t argc,
                             int depth, kiln_value *thrown) {
    if (code == KILN_C99_METHOD_NODE_NET_CONNECT) {
        kiln_node_socket *socket;
        kiln_node_completion *work;
        uint8_t *host_owned = NULL;
        size_t host_len = 0;
        kiln_value port_value = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value host_value = argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED
                                    ? kiln_c99_text(args[1])
                                    : kiln_c99_copy_text("localhost", 9);
        size_t callback_index = argc > 1 && kiln_c99_is_callable(args[1]) ? 1 : 2;
        double port = kiln_c99_as_number(port_value);
        int status;
        if (!(port >= 0.0 && port <= 65535.0 && floor(port) == port))
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "Port should be >= 0 and < 65536");
        socket = (kiln_node_socket *)kiln_c99_gc_calloc(KILN_GC_NODE_SOCKET, sizeof *socket);
        if (socket == NULL) return KILN_C99_OUT_OF_MEMORY;
        socket->host = host;
        status = kiln_c99_node_socket_object(socket, thrown);
        if (status != KILN_C99_OK) return status;
        if (argc > callback_index && kiln_c99_is_callable(args[callback_index])) {
            kiln_value once_args[2] = {kiln_c99_copy_text("connect", 7),
                                       args[callback_index]};
            kiln_value ignored;
            status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_ONCE,
                                         socket->object, once_args, 2, depth, thrown);
            if (status != KILN_C99_OK) return status;
        }
        work = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION, sizeof *work);
        if (work == NULL) return KILN_C99_OUT_OF_MEMORY;
        work->kind = KILN_REALM_OP_TCP_CONNECT;
        work->socket = socket;
        status = kiln_c99_string_to_utf8_lossy(host_value, &host_owned, &host_len);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_node_start(work->kind, 0, (uint64_t)port,
                                     (const char *)host_owned, host_len, NULL, 0, work,
                                     &socket->connect_request);
        free(host_owned);
        if (status != KILN_C99_OK) return status;
        *result = socket->object;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_NET_CREATE_SERVER ||
        code == KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER ||
        code == KILN_C99_METHOD_NODE_HTTP_CREATE_FETCH_SERVER ||
        code == KILN_C99_METHOD_NODE_HTTP_SERVER) {
        kiln_node_server *server;
        kiln_value ignored;
        kiln_value listener = kiln_c99_undefined();
        int http = code != KILN_C99_METHOD_NODE_NET_CREATE_SERVER;
        int status;
        if (!http) {
            listener = argc > 0 ? args[0] : kiln_c99_undefined();
            if (!kiln_c99_is_callable(listener))
                return kiln_c99_throw_not_a(thrown, listener, "function");
        } else {
            kiln_value first = argc > 0 ? args[0] : kiln_c99_undefined();
            if (first.tag != KILN_VALUE_UNDEFINED && first.tag != KILN_VALUE_NULL &&
                first.tag != KILN_VALUE_OBJECT && !kiln_c99_is_callable(first)) {
                return kiln_c99_throw_node_invalid_arg_type(
                    thrown, "The \"options\" argument must be of type object");
            }
            if (kiln_c99_is_callable(first)) {
                listener = first;
            } else if (argc > 1) {
                listener = args[1];
                if (listener.tag != KILN_VALUE_UNDEFINED && !kiln_c99_is_callable(listener)) {
                    return kiln_c99_throw_node_invalid_arg_type(
                        thrown, "The \"listener\" argument must be of type function");
                }
            }
        }
        server = (kiln_node_server *)kiln_c99_gc_calloc(KILN_GC_NODE_SERVER, sizeof *server);
        if (server == NULL) return KILN_C99_OUT_OF_MEMORY;
        server->host = host;
        server->connection_listener = listener;
        status = kiln_c99_new_object(&server->object);
        if (status == KILN_C99_OK)
            status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_EMITTER,
                                         server->object, NULL, 0, depth, thrown);
        if (status == KILN_C99_OK)
            status = kiln_c99_define_method(server->object, "listen", 6,
                                            kiln_c99_method(KILN_C99_METHOD_NODE_SERVER_LISTEN));
        if (status == KILN_C99_OK)
            status = kiln_c99_define_method(server->object, "close", 5,
                                            kiln_c99_method(KILN_C99_METHOD_NODE_SERVER_CLOSE));
        if (status == KILN_C99_OK)
            status = kiln_c99_define_method(server->object, "address", 7,
                                            kiln_c99_method(KILN_C99_METHOD_NODE_SERVER_ADDRESS));
        if (status == KILN_C99_OK)
            status = kiln_c99_define_method(
                server->object, "setTimeout", 10,
                kiln_c99_method(KILN_C99_METHOD_NODE_SERVER_SET_TIMEOUT));
        if (status == KILN_C99_OK)
            status = kiln_c99_node_set_hidden_pointer(server->object, "@@node:server", 13, server);
        if (status != KILN_C99_OK) { kiln_c99_gc_release(server); return status; }
        if (http) {
            server->protocol_http = 1;
            server->fetch_mode = code == KILN_C99_METHOD_NODE_HTTP_CREATE_FETCH_SERVER;
            status = kiln_c99_put(host, server->object, "httpAllowHalfOpen", 17,
                                  kiln_c99_boolean(0), depth, thrown);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(host, server->object, "timeout", 7,
                                      kiln_c99_number(0.0), depth, thrown);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(host, server->object, "keepAliveTimeout", 16,
                                      kiln_c99_number(5000.0), depth, thrown);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(host, server->object, "requestTimeout", 14,
                                      kiln_c99_number(300000.0), depth, thrown);
        }
        if (status != KILN_C99_OK) { kiln_c99_gc_release(server); return status; }
        *result = server->object;
        return KILN_C99_OK;
    }
    if (kiln_c99_method_in_group(code, KILN_C99_GROUP_NODE_SERVER)) {
        kiln_node_server *server = (kiln_node_server *)kiln_c99_node_hidden_pointer(
            receiver, "@@node:server", 13);
        if (server == NULL) return kiln_c99_throw_not_a(thrown, receiver, "Server");
        if (code == KILN_C99_METHOD_NODE_SERVER_SET_TIMEOUT) {
            double timeout = kiln_c99_as_number(argc > 0 ? args[0] : kiln_c99_undefined());
            int status;
            if (!(timeout >= 0.0) || !isfinite(timeout))
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "The value of timeout is out of range");
            server->timeout_milliseconds = (uint64_t)floor(timeout);
            status = kiln_c99_put(host, receiver, "timeout", 7,
                                  kiln_c99_number((double)server->timeout_milliseconds),
                                  depth, thrown);
            if (status == KILN_C99_OK && argc > 1 && kiln_c99_is_callable(args[1])) {
                kiln_value on_args[2] = {kiln_c99_copy_text("timeout", 7), args[1]};
                kiln_value ignored;
                status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_ON,
                                             receiver, on_args, 2, depth, thrown);
            }
            if (status != KILN_C99_OK) return status;
            *result = receiver;
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_NODE_SERVER_LISTEN) {
            kiln_node_completion *work;
            uint8_t *host_owned = NULL;
            size_t host_len = 0;
            kiln_value port_value = argc > 0 ? args[0] : kiln_c99_undefined();
            size_t callback_index = argc > 1 && kiln_c99_is_callable(args[1]) ? 1 : 2;
            kiln_value host_value = callback_index == 1
                                        ? kiln_c99_copy_text("0.0.0.0", 7)
                                        : argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED
                                              ? kiln_c99_text(args[1])
                                              : kiln_c99_copy_text("0.0.0.0", 7);
            double port = kiln_c99_as_number(port_value);
            int status;
            if (!(port >= 0.0 && port <= 65535.0 && floor(port) == port))
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "Port should be >= 0 and < 65536");
            if (argc > callback_index && kiln_c99_is_callable(args[callback_index])) {
                kiln_value once_args[2] = {kiln_c99_copy_text("listening", 9),
                                           args[callback_index]};
                kiln_value ignored;
                status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_ONCE,
                                             receiver, once_args, 2, depth, thrown);
                if (status != KILN_C99_OK) return status;
            }
            work = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION,
                                                              sizeof *work);
            if (work == NULL) return KILN_C99_OUT_OF_MEMORY;
            work->kind = KILN_REALM_OP_TCP_LISTEN;
            work->server = server;
            status = kiln_c99_string_to_utf8_lossy(host_value, &host_owned, &host_len);
            if (status != KILN_C99_OK) {
                kiln_c99_gc_release(work);
                return status;
            }
            status = kiln_c99_node_start(KILN_REALM_OP_TCP_LISTEN, 0, (uint64_t)port,
                                         (const char *)host_owned, host_len, NULL, 0, work,
                                         &server->listen_request);
            free(host_owned);
            if (status != KILN_C99_OK) return status;
            *result = receiver;
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_NODE_SERVER_CLOSE) {
            kiln_node_completion *work;
            kiln_node_socket *socket;
            int status;
            if (server->closing) {
                *result = receiver;
                return KILN_C99_OK;
            }
            if (argc > 0 && kiln_c99_is_callable(args[0])) {
                kiln_value once_args[2] = {kiln_c99_copy_text("close", 5), args[0]};
                kiln_value ignored;
                status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_ONCE,
                                             receiver, once_args, 2, depth, thrown);
                if (status != KILN_C99_OK) return status;
            }
            server->closing = 1;
            if (server->accept_request != 0) {
                kiln_c99_node_cancel_request(&server->accept_request);
            }
            for (socket = server->sockets; socket != NULL; socket = socket->next) {
                // HTTP responses may still have a write in flight. The socket helper keeps
                // that final write ordered before closing; for an idle keep-alive connection it
                // cancels the read and closes immediately. This is the native equivalent of the
                // interpreter's server-close drain, and prevents an accepted HTTP socket from
                // retaining the realm after the listener has been closed.
                status = kiln_c99_node_close_socket(socket, 0);
                if (status != KILN_C99_OK) return status;
            }
            if (server->listener != 0) {
                work = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION,
                                                                  sizeof *work);
                if (work == NULL) return KILN_C99_OUT_OF_MEMORY;
                work->kind = KILN_REALM_OP_CLOSE_RESOURCE;
                work->server = server;
                status = kiln_c99_node_start(KILN_REALM_OP_CLOSE_RESOURCE, server->listener, 0,
                                             NULL, 0, NULL, 0, work, &server->listen_request);
                if (status != KILN_C99_OK) return status;
            }
            *result = receiver;
            return KILN_C99_OK;
        }
        if (server->listener == 0) {
            *result = kiln_c99_null();
            return KILN_C99_OK;
        }
        {
            kiln_value address;
            int status = kiln_c99_new_object(&address);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, address, "port", 4,
                                      kiln_c99_number((double)server->port), 0, NULL);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, address, "family", 6,
                                      kiln_c99_copy_text("IPv4", 4), 0, NULL);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, address, "address", 7,
                                      kiln_c99_copy_text(server->address, server->address_len), 0,
                                      NULL);
            *result = address;
            return status;
        }
    }
    {
        kiln_node_socket *socket = (kiln_node_socket *)kiln_c99_node_hidden_pointer(
            receiver, "@@node:socket", 13);
        kiln_node_completion *work;
        int status;
        if (socket == NULL) return kiln_c99_throw_not_a(thrown, receiver, "Socket");
        if (code == KILN_C99_METHOD_NODE_READABLE_SET_ENCODING) {
            kiln_value encoding = argc > 0 ? kiln_c99_text(args[0])
                                           : kiln_c99_copy_text("utf8", 4);
            size_t encoding_len = 0;
            const char *encoding_bytes = kiln_c99_string_ascii_bytes(encoding, &encoding_len);
            if (!((encoding_bytes != NULL && encoding_len == 4 && memcmp(encoding_bytes, "utf8", 4) == 0) ||
                  (encoding_bytes != NULL && encoding_len == 5 && memcmp(encoding_bytes, "utf-8", 5) == 0)))
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "unsupported Node stream encoding");
            socket->utf8 = 1;
            *result = receiver;
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_NODE_SOCKET_DESTROY) {
            status = kiln_c99_node_socket_ready_state(socket, "closed", 6);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_node_close_socket(socket, 1);
            if (status != KILN_C99_OK) return status;
            *result = receiver;
            return KILN_C99_OK;
        }
        work = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION, sizeof *work);
        if (work == NULL) return KILN_C99_OUT_OF_MEMORY;
        work->socket = socket;
        if (code == KILN_C99_METHOD_NODE_SOCKET_WRITE) {
            kiln_value text = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            uint8_t *owned = NULL;
            size_t text_len = 0;
            status = kiln_c99_string_to_utf8_lossy(text, &owned, &text_len);
            if (status != KILN_C99_OK) return status;
            work->kind = KILN_REALM_OP_TCP_WRITE;
            status = kiln_c99_node_start(KILN_REALM_OP_TCP_WRITE, socket->resource, 0, NULL, 0,
                                         owned, text_len, work,
                                         &socket->write_request);
            free(owned);
            if (status != KILN_C99_OK) return status;
            *result = kiln_c99_boolean(1);
            return KILN_C99_OK;
        }
        if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED) {
            kiln_value text = kiln_c99_text(args[0]);
            uint8_t *owned = NULL;
            size_t text_len = 0;
            status = kiln_c99_string_to_utf8_lossy(text, &owned, &text_len);
            if (status != KILN_C99_OK) return status;
            work->kind = KILN_REALM_OP_TCP_WRITE;
            socket->shutdown_after_write = 1;
            status = kiln_c99_node_socket_ready_state(socket, "readOnly", 8);
            if (status != KILN_C99_OK) {
                kiln_c99_gc_release(work);
                return status;
            }
            status = kiln_c99_node_start(KILN_REALM_OP_TCP_WRITE, socket->resource, 0, NULL, 0,
                                         owned, text_len, work,
                                         &socket->write_request);
            free(owned);
            if (status != KILN_C99_OK) {
                socket->shutdown_after_write = 0;
                return status;
            }
            *result = receiver;
            return KILN_C99_OK;
        }
        work->kind = KILN_REALM_OP_TCP_SHUTDOWN;
        status = kiln_c99_node_socket_ready_state(socket, "readOnly", 8);
        if (status != KILN_C99_OK) {
            kiln_c99_gc_release(work);
            return status;
        }
        status = kiln_c99_node_start(
            work->kind, socket->resource, 0, NULL, 0, NULL, 0, work,
            &socket->shutdown_request);
        if (status != KILN_C99_OK) return status;
        *result = receiver;
        return KILN_C99_OK;
    }
}

/* Flatten a header object into the NUL-separated block the runtime's HTTP frame takes.
 *
 * Sized once from the property list instead of grown into.  `kiln_c99_text_append` doubles
 * through `realloc` and was called four times per header, so a response with a handful of
 * headers reallocated several times per request to build a block whose exact length the property
 * list already knew.  Two passes over a few properties cost less than those reallocs, and the
 * buffer ends up exactly the size of the frame rather than up to twice it.
 *
 * The second pass re-reads each value through `kiln_c99_text`, which is free for the strings
 * these objects actually hold -- `setHeader` and `writeHead` both store text -- and is
 * deterministic for anything else, so the two passes cannot disagree about a length.  The bound
 * check keeps that a checked invariant rather than a promise.
 *
 * On success the caller owns `out->bytes`, which is what the three call sites already expected of
 * the builder they replaced. */
static int kiln_c99_node_flatten_headers(kiln_object *headers, kiln_text *out) {
    size_t needed = 0;
    size_t at = 0;
    size_t index;
    char *bytes;
    if (headers->shape != NULL) {
        kiln_c99_ensure_dictionary(headers);
    }
    out->bytes = NULL;
    out->len = 0;
    out->capacity = 0;
    for (index = 0; index < headers->count; ++index) {
        kiln_value key_value = kiln_c99_stable_key_value(headers->properties[index].key);
        uint8_t *key_encoded = NULL;
        size_t key_encoded_len = 0;
        uint8_t *encoded = NULL;
        size_t encoded_len = 0;
        if (kiln_c99_string_to_utf8_lossy(key_value, &key_encoded, &key_encoded_len) !=
            KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (kiln_c99_string_to_utf8_lossy(kiln_c99_text(headers->properties[index].value),
                                          &encoded, &encoded_len) != KILN_C99_OK) {
            free(key_encoded);
            return KILN_C99_OUT_OF_MEMORY;
        }
        free(key_encoded);
        free(encoded);
        if (key_encoded_len > SIZE_MAX - encoded_len - 2 ||
            needed > SIZE_MAX - key_encoded_len - encoded_len - 2) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        needed += key_encoded_len + encoded_len + 2;
    }
    if (needed == 0) return KILN_C99_OK;
    bytes = (char *)malloc(needed);
    if (bytes == NULL) return KILN_C99_OUT_OF_MEMORY;
    for (index = 0; index < headers->count; ++index) {
        kiln_property *property = &headers->properties[index];
        kiln_value key_value = kiln_c99_stable_key_value(property->key);
        uint8_t *key_encoded = NULL;
        size_t key_encoded_len = 0;
        uint8_t *encoded = NULL;
        size_t encoded_len = 0;
        if (kiln_c99_string_to_utf8_lossy(key_value, &key_encoded, &key_encoded_len) !=
            KILN_C99_OK) {
            free(bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (kiln_c99_string_to_utf8_lossy(kiln_c99_text(property->value), &encoded,
                                          &encoded_len) != KILN_C99_OK) {
            free(key_encoded);
            free(bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (at + key_encoded_len + encoded_len + 2 > needed) {
            free(key_encoded);
            free(encoded);
            free(bytes);
            KILN_C99_REFUSE("a header buffer that outgrew the size it was measured at");
        }
        if (key_encoded_len != 0) memcpy(bytes + at, key_encoded, key_encoded_len);
        at += key_encoded_len;
        bytes[at++] = '\0';
        if (encoded_len != 0) memcpy(bytes + at, encoded, encoded_len);
        at += encoded_len;
        bytes[at++] = '\0';
        free(key_encoded);
        free(encoded);
    }
    out->bytes = bytes;
    out->len = at;
    out->capacity = needed;
    return KILN_C99_OK;
}

static int kiln_c99_node_http(kiln_host *host, kiln_value *result, uint64_t code,
                              kiln_value receiver, const kiln_value *args, size_t argc,
                              int depth, kiln_value *thrown) {
    kiln_node_response *response;
    if (code == KILN_C99_METHOD_NODE_HTTP_FETCH_RESPOND)
        return kiln_c99_node_fetch_respond(result, args, argc, thrown);
    response = kiln_c99_node_response_of(receiver);
    (void)depth;
    if (code == KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER ||
        code == KILN_C99_METHOD_NODE_HTTP_CREATE_FETCH_SERVER ||
        code == KILN_C99_METHOD_NODE_HTTP_SERVER)
        return kiln_c99_node_net(host, result, code, receiver, args, argc, depth, thrown);
    if (code == KILN_C99_METHOD_NODE_HTTP_GET || code == KILN_C99_METHOD_NODE_HTTP_REQUEST) {
        kiln_node_client_request *request;
        kiln_node_completion *connect;
        kiln_value options = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value callback = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_value host_value = kiln_c99_copy_text("localhost", 9);
        kiln_value path_value = kiln_c99_copy_text("/", 1);
        kiln_value method_value = kiln_c99_copy_text(
            code == KILN_C99_METHOD_NODE_HTTP_GET ? "GET" : "GET", 3);
        kiln_value headers_value = kiln_c99_undefined();
        kiln_value port_value = kiln_c99_number(80.0);
        int status;
        if (options.tag == KILN_VALUE_OBJECT) {
            status = kiln_c99_get(host, options, "host", 4, &host_value, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (host_value.tag == KILN_VALUE_UNDEFINED)
                host_value = kiln_c99_copy_text("localhost", 9);
            status = kiln_c99_get(host, options, "port", 4, &port_value, depth, thrown);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_get(host, options, "path", 4, &path_value, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (path_value.tag == KILN_VALUE_UNDEFINED) path_value = kiln_c99_copy_text("/", 1);
            status = kiln_c99_get(host, options, "method", 6, &method_value, depth, thrown);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_get(host, options, "headers", 7, &headers_value, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (method_value.tag == KILN_VALUE_UNDEFINED)
                method_value = kiln_c99_copy_text("GET", 3);
        } else {
            return kiln_c99_throw_not_a(thrown, options, "request options object");
        }
        host_value = kiln_c99_text(host_value);
        path_value = kiln_c99_text(path_value);
        method_value = kiln_c99_text(method_value);
        if (!kiln_c99_is_callable(callback))
            return kiln_c99_throw_not_a(thrown, callback, "function");
        request = (kiln_node_client_request *)kiln_c99_gc_calloc(KILN_GC_NODE_CLIENT_REQUEST,
                                                                 sizeof *request);
        if (request == NULL) return KILN_C99_OUT_OF_MEMORY;
        request->host = host;
        request->callback = callback;
        status = kiln_c99_node_copy_utf8(host_value, &request->host_name, &request->host_name_len);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_node_copy_utf8(path_value, &request->path, &request->path_len);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_node_copy_utf8(method_value, &request->method, &request->method_len);
        if (status != KILN_C99_OK) return status;
        request->port = (uint16_t)kiln_c99_as_number(port_value);
        status = KILN_C99_OK;
        if (headers_value.tag == KILN_VALUE_UNDEFINED) {
        } else if (headers_value.tag == KILN_VALUE_OBJECT) {
            kiln_text flattened;
            status = kiln_c99_node_flatten_headers(kiln_c99_object_of(headers_value), &flattened);
            request->headers = flattened.bytes;
            request->headers_len = flattened.len;
        } else {
            return kiln_c99_throw_not_a(thrown, headers_value, "headers object");
        }
        if (status == KILN_C99_OK) status = kiln_c99_new_object(&request->object);
        if (status == KILN_C99_OK) {
            kiln_value ignored;
            status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_EMITTER,
                                         request->object, NULL, 0, depth, thrown);
        }
        if (status == KILN_C99_OK)
            status = kiln_c99_define_method(request->object, "write", 5,
                                            kiln_c99_method(KILN_C99_METHOD_NODE_CLIENT_REQUEST_WRITE));
        if (status == KILN_C99_OK)
            status = kiln_c99_define_method(request->object, "end", 3,
                                            kiln_c99_method(KILN_C99_METHOD_NODE_CLIENT_REQUEST_END));
        if (status == KILN_C99_OK)
            status = kiln_c99_node_set_hidden_pointer(request->object, "@@node:client", 13,
                                                      request);
        if (status != KILN_C99_OK) return status;
        request->ended = code == KILN_C99_METHOD_NODE_HTTP_GET;
        status = kiln_realm_http_client_request_id_active(&request->request_id);
        if (status != KILN_C99_OK) return status;
        {
            uint64_t resource = 0;
            int decision = kiln_realm_http_agent_acquire_active(
                (const uint8_t *)request->host_name, request->host_name_len,
                request->port, request->request_id, &resource);
            if (decision == KILN_REALM_HTTP_AGENT_CONNECT) {
                connect = (kiln_node_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_COMPLETION,
                                                                     sizeof *connect);
                if (connect == NULL) return KILN_C99_OUT_OF_MEMORY;
                connect->kind = KILN_REALM_OP_TCP_CONNECT;
                connect->client_request = request;
                status = kiln_c99_node_start(KILN_REALM_OP_TCP_CONNECT, 0, request->port,
                                             request->host_name,
                                             request->host_name_len, NULL, 0, connect,
                                             &request->connect_request);
                if (status != KILN_C99_OK) return status;
            } else if (decision == KILN_REALM_HTTP_AGENT_REUSE) {
                kiln_node_socket *socket = NULL;
                status = kiln_realm_http_client_owner_active(resource, (void **)&socket);
                if (status != KILN_C99_OK || socket == NULL)
                    KILN_C99_REFUSE("the agent offered a socket to reuse that has no owner");
                status = kiln_c99_node_client_attach(request, socket, 0,
                                                     (kiln_value *)thrown);
                if (status != KILN_C99_OK) return status;
            } else if (decision != KILN_REALM_HTTP_AGENT_QUEUED) {
                KILN_C99_REFUSE("an HTTP agent dispatch decision this backend does not model");
            }
        }
        *result = request->object;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_READABLE_SET_ENCODING) {
        kiln_node_client_request *request =
            (kiln_node_client_request *)kiln_c99_node_hidden_pointer(receiver,
                                                                     "@@node:client", 13);
        if (request != NULL) {
            request->utf8 = 1;
        } else if (kiln_c99_node_hidden_pointer(receiver, "@@node:response", 15) == NULL) {
            return kiln_c99_throw_not_a(thrown, receiver, "IncomingMessage");
        }
        *result = receiver;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_CLIENT_REQUEST_WRITE ||
        code == KILN_C99_METHOD_NODE_CLIENT_REQUEST_END) {
        kiln_node_client_request *request =
            (kiln_node_client_request *)kiln_c99_node_hidden_pointer(receiver,
                                                                     "@@node:client", 13);
        kiln_c99_node_byte_span span;
        int status;
        if (request == NULL) return kiln_c99_throw_not_a(thrown, receiver, "ClientRequest");
        if (request->ended)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR,
                                        "write after end");
        status = kiln_c99_node_http_byte_span(
            argc > 0 ? args[0] : kiln_c99_undefined(), &span);
        if (status != KILN_C99_OK) return status;
        if (request->sent) {
            status = code == KILN_C99_METHOD_NODE_CLIENT_REQUEST_END
                         ? kiln_realm_http_client_end_active(
                               request->socket->resource, request->request_id,
                               span.bytes, span.len)
                         : kiln_realm_http_client_write_active(
                               request->socket->resource, request->request_id,
                               span.bytes, span.len);
            kiln_c99_node_http_byte_span_release(&span);
            if (status < KILN_C99_OK) return status;
        } else if (span.len != 0) {
            if (span.len > SIZE_MAX - request->pending_body_len)
                { kiln_c99_node_http_byte_span_release(&span); return KILN_C99_OUT_OF_MEMORY; }
            if (kiln_c99_reserve((void **)&request->pending_body,
                                 &request->pending_body_capacity,
                                 request->pending_body_len + span.len, 1) != KILN_C99_OK)
                { kiln_c99_node_http_byte_span_release(&span); return KILN_C99_OUT_OF_MEMORY; }
            memcpy(request->pending_body + request->pending_body_len, span.bytes, span.len);
            request->pending_body_len += span.len;
            kiln_c99_node_http_byte_span_release(&span);
        } else {
            kiln_c99_node_http_byte_span_release(&span);
        }
        if (code == KILN_C99_METHOD_NODE_CLIENT_REQUEST_WRITE) {
            *result = kiln_c99_boolean(!request->sent || status == KILN_C99_OK);
            return KILN_C99_OK;
        }
        request->ended = 1;
        status = kiln_c99_node_client_try_start(request);
        if (status < KILN_C99_OK) return status;
        *result = receiver;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_REQUEST_HEADERS_GET ||
        code == KILN_C99_METHOD_NODE_REQUEST_HEADERS_SET) {
        /* Both halves end the same way: the accessor is replaced, in place, by the ordinary
         * writable data property the eager build used to leave behind.  So a program sees an
         * accessor only until it touches `headers` once, and every read after the first is a
         * plain property read rather than a call. */
        kiln_value built;
        int status;
        if (receiver.tag != KILN_VALUE_OBJECT)
            return kiln_c99_throw_not_a(thrown, receiver, "IncomingMessage");
        if (code == KILN_C99_METHOD_NODE_REQUEST_HEADERS_SET) {
            built = argc > 0 ? args[0] : kiln_c99_undefined();
        } else {
            status = kiln_c99_node_request_headers_object(response, &built);
            if (status != KILN_C99_OK) return status;
        }
        /* The bytes now live in the object (or were replaced by the program's own): nothing
         * reads them again. */
        if (response != NULL && response->request_headers != NULL) {
            if (response->request_headers_owned) free(response->request_headers);
            response->request_headers = NULL;
            response->request_headers_len = 0;
            response->request_headers_owned = 0;
        }
        /* The accessor lives on the prototype; the first get or set defines the plain, writable,
         * enumerable own data property every read after sees -- a definition, which neither
         * reaches the inherited setter nor moves a shaped request to dictionary mode. */
        status = kiln_c99_define_computed(
            receiver, kiln_c99_static_text("headers", 7), built);
        if (status != KILN_C99_OK) return status;
        *result = built;
        return KILN_C99_OK;
    }
    if (response == NULL) return kiln_c99_throw_not_a(thrown, receiver, "ServerResponse");
    if (code == KILN_C99_METHOD_NODE_RESPONSE_SET_HEADER) {
        int status = kiln_c99_node_out_set_values(response,
                                                  argc > 0 ? args[0] : kiln_c99_undefined(),
                                                  argc > 1 ? args[1] : kiln_c99_undefined());
        if (status != KILN_C99_OK) return status;
        *result = receiver;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_RESPONSE_GET_HEADER ||
        code == KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER ||
        code == KILN_C99_METHOD_NODE_RESPONSE_REMOVE_HEADER) {
        kiln_value name = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
        const char *name_bytes;
        size_t name_len;
        uint8_t *name_owned;
        size_t entry, value_at, end;
        int status;
        if (name.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        status = kiln_c99_node_utf8_bytes(name, &name_bytes, &name_len, &name_owned);
        if (status != KILN_C99_OK) return status;
        if (code == KILN_C99_METHOD_NODE_RESPONSE_REMOVE_HEADER) {
            kiln_c99_node_out_remove(response, name_bytes, name_len);
            *result = kiln_c99_undefined();
        } else if (!kiln_c99_node_out_find(response, name_bytes, name_len, &entry, &value_at,
                                           &end)) {
            *result = code == KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER ? kiln_c99_boolean(0)
                                                                       : kiln_c99_undefined();
        } else if (code == KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER) {
            *result = kiln_c99_boolean(1);
        } else {
            *result = kiln_c99_copy_text(response->out_headers + value_at, end - 1 - value_at);
            if (result->tag != KILN_VALUE_STRING) status = KILN_C99_OUT_OF_MEMORY;
        }
        free(name_owned);
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_RESPONSE_GET_HEADERS) {
        /* A null-prototype dictionary keyed by lowercased name, as the interpreter answers, so a
         * header called `constructor` is a header and never inherited behaviour. */
        kiln_value copy;
        size_t at = 0;
        int status = kiln_c99_new_object(&copy);
        if (status == KILN_C99_OK) status = kiln_c99_set_prototype(copy, kiln_c99_null());
        while (status == KILN_C99_OK && at < response->out_headers_len) {
            const char *name = response->out_headers + at;
            const char *name_end = (const char *)memchr(
                name, '\0', response->out_headers_len - at);
            const char *value;
            const char *value_end;
            char name_buffer[64];
            char *lowered;
            size_t name_len;
            size_t byte;
            kiln_value text;
            kiln_property *slot;
            if (name_end == NULL) break;
            value = name_end + 1;
            value_end = (const char *)memchr(
                value, '\0', response->out_headers_len - (size_t)(value - response->out_headers));
            if (value_end == NULL) break;
            at = (size_t)(value_end - response->out_headers) + 1;
            name_len = (size_t)(name_end - name);
            lowered = name_len <= sizeof name_buffer ? name_buffer : (char *)malloc(name_len);
            if (lowered == NULL) return KILN_C99_OUT_OF_MEMORY;
            for (byte = 0; byte < name_len; ++byte)
                lowered[byte] = (char)tolower((unsigned char)name[byte]);
            text = kiln_c99_copy_text(value, (size_t)(value_end - value));
            if (text.tag != KILN_VALUE_STRING) {
                status = KILN_C99_OUT_OF_MEMORY;
            } else {
                status = kiln_c99_own_slot(copy, lowered, name_len, &slot);
                if (status == KILN_C99_OK) slot->value = text;
            }
            if (lowered != name_buffer) free(lowered);
        }
        if (status != KILN_C99_OK) return status;
        *result = copy;
        return KILN_C99_OK;
    }
    {
        const kiln_node_http_layout *layout;
        int status = kiln_c99_node_http_layout_ready(&layout);
        if (status != KILN_C99_OK) return status;
        if (code == KILN_C99_METHOD_NODE_RESPONSE_WRITE_HEAD) {
            double requested = kiln_c99_as_number(argc > 0 ? args[0] : kiln_c99_undefined());
            if (!(requested >= 100.0 && requested <= 999.0 && floor(requested) == requested))
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "Invalid status code");
            response->status = (uint16_t)requested;
            status = kiln_c99_node_layout_store(response->object, KILN_C99_NODE_RES_STATUS_CODE,
                                                layout->response_keys[KILN_C99_NODE_RES_STATUS_CODE],
                                                kiln_c99_number(requested));
            if (status == KILN_C99_OK)
                status = kiln_c99_node_layout_store(
                    response->object, KILN_C99_NODE_RES_HEADERS_SENT,
                    layout->response_keys[KILN_C99_NODE_RES_HEADERS_SENT], kiln_c99_boolean(1));
            if (status != KILN_C99_OK) return status;
            if (argc > 1 && args[1].tag == KILN_VALUE_OBJECT) {
                /* The object's own enumerable data members in property order, read in place:
                 * an object literal is shaped, and converting it to dictionary mode to walk it
                 * would slow every later read of the program's own object. */
                kiln_value provided = args[1];
                kiln_object *object = kiln_c99_object_of(provided);
                size_t index;
                if (object->shape != NULL &&
                    (object->elements != NULL || object->shape->special_mask != 0))
                    kiln_c99_ensure_dictionary(object);
                if (object->shape != NULL) {
                    for (index = 0; status == KILN_C99_OK &&
                                    index < object->shape->property_count; ++index) {
                        const kiln_string *key = object->shape->keys[index];
                        if (kiln_c99_key_is_reserved(key) || index >= object->slot_capacity)
                            continue;
                        status = kiln_c99_node_out_set_values(
                            response, kiln_c99_stable_key_value(key),
                            kiln_val_to_boundary(object->slots[index]));
                        /* The walk reads the object after a conversion that can allocate, so the
                         * object is re-read rather than trusted across it. */
                        object = kiln_c99_object_of(provided);
                        if (object->shape == NULL) break;
                    }
                }
                if (object->shape == NULL) {
                    for (index = 0; status == KILN_C99_OK && index < object->count; ++index) {
                        kiln_property *property = &object->properties[index];
                        if (!property->enumerable || property->accessor) continue;
                        status = kiln_c99_node_out_set_values(
                            response,
                            kiln_c99_stable_key_value(property->key),
                            property->value);
                        object = kiln_c99_object_of(provided);
                    }
                }
                if (status != KILN_C99_OK) return status;
            }
            *result = receiver;
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_NODE_RESPONSE_WRITE ||
            code == KILN_C99_METHOD_NODE_RESPONSE_END) {
            kiln_value arg = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_c99_node_byte_span span;
            if (response->ended) {
                if (code == KILN_C99_METHOD_NODE_RESPONSE_END) {
                    *result = receiver;
                    return KILN_C99_OK;
                }
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR, "write after end");
            }
            if (!response->started) {
                /* `res.statusCode = 404; res.end()` is how most handlers set a status, so the
                 * head reads the member rather than the last `writeHead`. */
                kiln_realm_http_response framed;
                kiln_value code_value;
                double requested;
                if (!kiln_c99_node_layout_load(response->object, KILN_C99_NODE_RES_STATUS_CODE,
                                               layout->response_keys[KILN_C99_NODE_RES_STATUS_CODE],
                                               &code_value)) {
                    status = kiln_c99_get(host, response->object, "statusCode", 10, &code_value,
                                          depth, thrown);
                    if (status != KILN_C99_OK) return status;
                }
                requested = kiln_c99_as_number(code_value);
                if (!(requested >= 100.0 && requested <= 999.0 && floor(requested) == requested))
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                "Invalid status code");
                response->status = (uint16_t)requested;
                memset(&framed, 0, sizeof framed);
                framed.status = response->status;
                framed.headers = (const uint8_t *)response->out_headers;
                framed.headers_len = response->out_headers_len;
                status = kiln_realm_http_response_start_active(response->socket->resource,
                                                               response->request_id, &framed,
                                                               16384);
                if (status != KILN_C99_OK) return status;
                response->started = 1;
                status = kiln_c99_node_layout_store(
                    response->object, KILN_C99_NODE_RES_HEADERS_SENT,
                    layout->response_keys[KILN_C99_NODE_RES_HEADERS_SENT], kiln_c99_boolean(1));
                if (status != KILN_C99_OK) return status;
            }
            status = kiln_c99_node_http_byte_span(arg, &span);
            if (status != KILN_C99_OK) return status;
            if (code == KILN_C99_METHOD_NODE_RESPONSE_WRITE) {
                status = kiln_realm_http_response_write_active(response->socket->resource,
                                                               response->request_id,
                                                               span.bytes, span.len);
                kiln_c99_node_http_byte_span_release(&span);
                if (status < KILN_C99_OK) return status;
                *result = kiln_c99_boolean(status == KILN_C99_OK);
                return KILN_C99_OK;
            }
            status = kiln_realm_http_response_end_active(response->socket->resource,
                                                         response->request_id, span.bytes,
                                                         span.len);
            kiln_c99_node_http_byte_span_release(&span);
            if (status < KILN_C99_OK) return status;
            response->ended = 1;
            status = kiln_c99_node_layout_store(
                response->object, KILN_C99_NODE_RES_WRITABLE_ENDED,
                layout->response_keys[KILN_C99_NODE_RES_WRITABLE_ENDED], kiln_c99_boolean(1));
            if (status != KILN_C99_OK) return status;
            if (response->socket->close_after_responses &&
                !kiln_c99_node_has_unended_response(response->socket)) {
                response->socket->close_after_responses = 0;
                status = kiln_realm_http_server_close_active(
                    response->socket->resource, KILN_REALM_HTTP_CLOSE_GRACEFUL);
                if (status != KILN_C99_OK) return status;
            }
            *result = receiver;
            return KILN_C99_OK;
        }
    }
    KILN_C99_REFUSE("a Node HTTP method this backend has not implemented");
}
