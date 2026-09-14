
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
            else if (code == KILN_C99_METHOD_NODE_EVENT_LISTENERS) return kiln_c99_new_array(result);
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
        "@@node:lazy-headers", "@@kiln:node:is-file", "@@kiln:node:is-directory",
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
    if (slot == NULL || slot->value.tag != KILN_VALUE_OBJECT ||
        slot->value.len != 0xFFFFFFFFu)
        return NULL;
    return (void *)(uintptr_t)slot->value.bits;
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

static int kiln_c99_node_materialize_header(kiln_value headers_obj, const char *key, size_t key_len, kiln_value *out) {
    if (headers_obj.tag != KILN_VALUE_OBJECT) return 0;
    kiln_object *obj = kiln_c99_object_of(headers_obj);
    if (obj->shape != NULL) kiln_c99_ensure_dictionary(obj);
    if (obj->properties == NULL) return 0;
    kiln_node_response *resp = (kiln_node_response *)kiln_c99_node_hidden_pointer(headers_obj, "@@node:lazy-headers", 19);
    size_t at = 0;
    if (resp == NULL || resp->request_headers == NULL) return 0;
    while (at < resp->request_headers_len) {
        const char *hname = resp->request_headers + at;
        size_t hname_len = strlen(hname);
        const char *hval;
        size_t hval_len;
        at += hname_len + 1;
        if (at > resp->request_headers_len) break;
        hval = resp->request_headers + at;
        hval_len = strlen(hval);
        at += hval_len + 1;
        if (at > resp->request_headers_len) break;
        if (kiln_c99_ascii_case_equal(hname, hname_len, key, key_len)) {
            char *owned_name = hname_len == 0 ? NULL : kiln_c99_gc_alloc_text(hname_len);
            if (hname_len != 0 && owned_name != NULL) {
                memcpy(owned_name, hname, hname_len);
            }
            kiln_value val = kiln_c99_copy_text(hval, hval_len);
            kiln_c99_put(NULL, headers_obj, owned_name, hname_len, val, 0, NULL);
            if (out != NULL) *out = val;
            return 1;
        }
    }
    return 0;
}

static void kiln_c99_node_materialize_all_headers(kiln_value headers_obj) {
    if (headers_obj.tag != KILN_VALUE_OBJECT) return;
    kiln_object *obj = kiln_c99_object_of(headers_obj);
    if (obj->shape != NULL) kiln_c99_ensure_dictionary(obj);
    if (obj->properties == NULL) return;
    kiln_node_response *resp = (kiln_node_response *)kiln_c99_node_hidden_pointer(headers_obj, "@@node:lazy-headers", 19);
    size_t at = 0;
    int dummy;
    if (resp == NULL || resp->request_headers == NULL) return;
    kiln_c99_delete(NULL, headers_obj, "@@node:lazy-headers", 19, &dummy, 0, NULL);
    while (at < resp->request_headers_len) {
        const char *hname = resp->request_headers + at;
        size_t hname_len = strlen(hname);
        const char *hval;
        size_t hval_len;
        at += hname_len + 1;
        if (at > resp->request_headers_len) break;
        hval = resp->request_headers + at;
        hval_len = strlen(hval);
        at += hval_len + 1;
        if (at > resp->request_headers_len) break;
        if (kiln_c99_own_index(obj, NULL, hname, hname_len) == obj->count) {
            char *owned_name = hname_len == 0 ? NULL : kiln_c99_gc_alloc_text(hname_len);
            if (hname_len != 0 && owned_name != NULL) {
                memcpy(owned_name, hname, hname_len);
            }
            kiln_c99_put(NULL, headers_obj, owned_name, hname_len,
                         kiln_c99_copy_text(hval, hval_len), 0, NULL);
        }
    }
}

static int kiln_c99_node_has_lazy_header(kiln_value headers_obj, const char *key, size_t key_len) {
    if (headers_obj.tag != KILN_VALUE_OBJECT) return 0;
    kiln_object *obj = kiln_c99_object_of(headers_obj);
    if (obj->shape != NULL) kiln_c99_ensure_dictionary(obj);
    if (obj->properties == NULL) return 0;
    kiln_node_response *resp = (kiln_node_response *)kiln_c99_node_hidden_pointer(headers_obj, "@@node:lazy-headers", 19);
    size_t at = 0;
    if (resp == NULL || resp->request_headers == NULL) return 0;
    while (at < resp->request_headers_len) {
        const char *hname = resp->request_headers + at;
        size_t hname_len = strlen(hname);
        const char *hval;
        size_t hval_len;
        at += hname_len + 1;
        if (at > resp->request_headers_len) break;
        hval = resp->request_headers + at;
        hval_len = strlen(hval);
        at += hval_len + 1;
        if (at > resp->request_headers_len) break;
        if (kiln_c99_ascii_case_equal(hname, hname_len, key, key_len)) {
            return 1;
        }
    }
    return 0;
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
    if (len == 1 && bytes[0] == '/') {
        return kiln_c99_static_text(kiln_c99_slash, 1);
    }
    return kiln_c99_copy_text(bytes, len);
}

/* A request method as a value, without copying bytes that are always one of nine spellings.
 *
 * Every method a server sees is a token from a fixed registry, so the string a program reads
 * back can be this unit's own literal rather than a fresh block per request. An extension method
 * -- the registry is open -- still copies, so nothing depends on the list being complete. */
static kiln_value kiln_c99_node_http_method_text(const char *bytes, size_t len) {
    static const char *const known[] = {"GET",     "POST",    "PUT",  "DELETE", "HEAD",
                                        "OPTIONS", "PATCH",   "CONNECT", "TRACE"};
    size_t index;
    for (index = 0; index < sizeof known / sizeof known[0]; ++index) {
        const char *name = known[index];
        if (name[0] == bytes[0] && strlen(name) == len && memcmp(name, bytes, len) == 0) {
            return kiln_c99_static_text(name, len);
        }
    }
    return kiln_c99_copy_text(bytes, len);
}

typedef struct {
    const uint8_t *bytes;
    size_t len;
    /* Keeps a fallback ToString result rooted for the duration of the host operation. */
    kiln_value text;
} kiln_c99_node_byte_span;

/* Borrow bytes from strings and typed views without coercing a Buffer/Uint8Array through the
 * language string representation (which would turn [0, 1, 2] into the text "0,1,2"). */
static int kiln_c99_node_http_byte_span(kiln_value value, kiln_c99_node_byte_span *out) {
    out->bytes = (const uint8_t *)"";
    out->len = 0;
    out->text = kiln_c99_undefined();
    if (value.tag == KILN_VALUE_STRING) {
        out->bytes = (const uint8_t *)value.ptr;
        out->len = value.len;
        return KILN_C99_OK;
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
    out->bytes = (const uint8_t *)out->text.ptr;
    out->len = out->text.len;
    return KILN_C99_OK;
}

/* Materialize a borrowed HTTP byte span directly as a Buffer. The realm owns the event bytes
 * only for the duration of this callback, so the copy into the GC-managed typed-array storage
 * must finish before dispatch returns. Keeping this representation step here avoids the old
 * copy_text -> Buffer.from path, which allocated an intermediate string and converted it one byte
 * at a time. */
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
    if (event->kind == 1) {
        int recycled = 0;
        if (socket->recycled_response != NULL && socket->recycled_response->ended) {
            response = socket->recycled_response;
            socket->recycled_response = NULL;
            recycled = 1;
        } else {
            response = (kiln_node_response *)kiln_c99_gc_calloc(KILN_GC_NODE_RESPONSE,
                                                                sizeof *response);
            if (response == NULL) return KILN_C99_OUT_OF_MEMORY;
        }
        if (recycled) {
            if (response->request_headers_owned && response->request_headers != NULL) {
                free(response->request_headers);
                response->request_headers = NULL;
                response->request_headers_owned = 0;
            }
            response->socket = socket;
            response->request_id = event->id;
            response->status = 200;
            response->started = 0;
            response->ended = 0;
            response->url_used = 0;
            response->target_ptr = (const char *)event->target;
            response->target_len = (size_t)event->target_len;
            request = response->request;

            response->request_headers = (char *)event->headers;
            response->request_headers_len = event->headers_len;
            response->request_headers_owned = 0;

            status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_METHOD,
                                       kiln_c99_node_http_method_text(
                                           (const char *)event->method,
                                           (size_t)event->method_len));
            if (status == KILN_C99_OK)
                status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_URL,
                                           kiln_c99_node_http_target_text(
                                               (const char *)event->target,
                                               (size_t)event->target_len));
            if (status == KILN_C99_OK && !event->version_minor) {
                status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_HTTP_VERSION,
                                                   kiln_c99_static_text(kiln_c99_http_version_10, 3));
                if (status == KILN_C99_OK)
                    status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_HTTP_VERSION_MINOR,
                                                       kiln_c99_number(0.0));
            }
            if (status == KILN_C99_OK) {
                int dummy;
                kiln_c99_delete(NULL, request, "headers", 7, &dummy, 0, NULL);
                kiln_c99_delete(NULL, request, "complete", 8, &dummy, 0, NULL);
                kiln_c99_delete(NULL, request, "httpVersion", 11, &dummy, 0, NULL);
                kiln_c99_delete(NULL, request, "httpVersionMinor", 16, &dummy, 0, NULL);
                kiln_c99_delete(NULL, request, "@@node:events", 13, &dummy, 0, NULL);
                kiln_c99_delete(NULL, response->object, "statusCode", 10, &dummy, 0, NULL);
                kiln_c99_delete(NULL, response->object, "headersSent", 11, &dummy, 0, NULL);
                kiln_c99_delete(NULL, response->object, "writableEnded", 13, &dummy, 0, NULL);
                kiln_c99_delete(NULL, response->object, "@@node:events", 13, &dummy, 0, NULL);
            }

            if (status == KILN_C99_OK && response->headers.tag == KILN_VALUE_OBJECT) {
                kiln_object *hdr_obj = kiln_c99_object_of(response->headers);
                hdr_obj->count = 0;
                hdr_obj->shape = &kiln_c99_root_shape;
                hdr_obj->slots = hdr_obj->inline_slots;
                hdr_obj->slot_capacity = KILN_C99_INLINE_SLOTS;
                for (size_t slot_i = 0; slot_i < KILN_C99_INLINE_SLOTS; ++slot_i) {
                    hdr_obj->inline_slots[slot_i] = KILN_VAL_UNDEFINED;
                }
            }
        } else {
            response->socket = socket;
            response->request_id = event->id;
            response->status = 200;
            response->url_used = 0;
            response->target_ptr = (const char *)event->target;
            response->target_len = (size_t)event->target_len;
            status = kiln_c99_new_object_with_capacity_tenured(&request, 6);
            if (status == KILN_C99_OK) {
                kiln_value prototype;
                status = kiln_c99_node_incoming_message_prototype(&prototype);
                if (status == KILN_C99_OK) status = kiln_c99_set_prototype(request, prototype);
            }
            if (status == KILN_C99_OK)
                status = kiln_c99_node_set_hidden_pointer(request, "@@node:response", 15, response);
            if (status == KILN_C99_OK)
                status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_METHOD,
                                           kiln_c99_node_http_method_text(
                                               (const char *)event->method,
                                               (size_t)event->method_len));
            if (status == KILN_C99_OK)
                status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_URL,
                                           kiln_c99_node_http_target_text(
                                               (const char *)event->target,
                                               (size_t)event->target_len));
            if (status == KILN_C99_OK && !event->version_minor) {
                status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_HTTP_VERSION,
                                                   kiln_c99_static_text(kiln_c99_http_version_10, 3));
                if (status == KILN_C99_OK)
                    status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_HTTP_VERSION_MINOR,
                                                       kiln_c99_number(0.0));
            }
            if (status == KILN_C99_OK)
                status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_SOCKET, socket->object);
            if (status == KILN_C99_OK)
                status = kiln_c99_own_data_hot_key(request, KILN_C99_HOT_KEY_CONNECTION, socket->object);
            if (status == KILN_C99_OK && event->headers_len != 0) {
                response->request_headers = (char *)event->headers;
                response->request_headers_len = event->headers_len;
                response->request_headers_owned = 0;
            }
            if (status == KILN_C99_OK)
                status = kiln_c99_new_object_with_capacity_tenured(&response->object, 6);
            if (status == KILN_C99_OK) status = kiln_c99_new_object_tenured(&response->headers);
            if (status == KILN_C99_OK) {
                kiln_value prototype;
                status = kiln_c99_callable_prototype(
                    kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE), &prototype);
                if (status == KILN_C99_OK)
                    status = kiln_c99_set_prototype(response->object, prototype);
            }
            if (status == KILN_C99_OK)
                status = kiln_c99_node_set_hidden_pointer(response->object, "@@node:response", 15,
                                                          response);
            if (status == KILN_C99_OK)
                status = kiln_c99_own_data(response->object, "req", 3, request);
            if (status == KILN_C99_OK)
                status = kiln_c99_own_data_hot_key(response->object, KILN_C99_HOT_KEY_SOCKET, socket->object);
            if (status == KILN_C99_OK)
                status = kiln_c99_own_data_hot_key(response->object, KILN_C99_HOT_KEY_CONNECTION, socket->object);
            if (status == KILN_C99_OK) {
                status = kiln_c99_new_object_with_capacity_tenured(&response->url_object, 4);
                if (status == KILN_C99_OK)
                    status = kiln_c99_new_object_with_capacity_tenured(&response->search_params_object, 4);
                if (status == KILN_C99_OK)
                    status = kiln_c99_put_hot_key(NULL, response->search_params_object, KILN_C99_HOT_KEY_GET,
                                                  kiln_c99_method(KILN_C99_METHOD_URL_SEARCH_PARAMS_GET), 0, NULL);
                if (status == KILN_C99_OK)
                    status = kiln_c99_put_hot_key(NULL, response->url_object, KILN_C99_HOT_KEY_SEARCH_PARAMS,
                                                  response->search_params_object, 0, NULL);
            }
        }
        if (status != KILN_C99_OK) return status;
        response->request = request;
        response->next = socket->responses;
        socket->responses = response;
        {
            kiln_value arguments[2] = {request, response->object};
            if (!kiln_c99_is_callable(socket->server->connection_listener))
                return KILN_C99_OK;
            kiln_c99_active_http_response = response;
            status = kiln_c99_call(socket->host, &ignored, socket->server->connection_listener,
                                  socket->server->object, arguments, 2, 0, exception);
            kiln_c99_active_http_response = NULL;
            return status;
        }
    }
    if (event->kind == 2 || event->kind == 3) {
        for (response = socket->responses; response != NULL; response = response->next) {
            if (response->request_id == event->id) {
                if (event->kind == 2) {
                    kiln_value data;
                    status = kiln_c99_buffer_from_bytes(&data, event->data, event->data_len);
                    if (status != KILN_C99_OK) return status;
                    return kiln_c99_node_emit(socket->host, response->request, "data", data, 1,
                                              0, exception);
                }
                status = kiln_c99_put(NULL, response->request, "complete", 8,
                                      kiln_c99_boolean(1), 0, NULL);
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
                if (socket->recycled_response != NULL &&
                    socket->recycled_response != response) {
                    if (socket->recycled_response->request_headers_owned &&
                        socket->recycled_response->request_headers != NULL) {
                        free(socket->recycled_response->request_headers);
                        socket->recycled_response->request_headers = NULL;
                        socket->recycled_response->request_headers_owned = 0;
                    }
                }
                socket->recycled_response = response;
                kiln_c99_own_data_hot_key(response->request, KILN_C99_HOT_KEY_URL,
                                          kiln_c99_static_text(kiln_c99_slash, 1));
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
        int status = kiln_c99_buffer_from_bytes(&data_value, event->data, event->data_len);
        if (status != KILN_C99_OK) return status;
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
        socket->recycled_response = NULL;
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
            int status = kiln_c99_buffer_from(&data, bytes, exception);
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
            if (status == KILN_C99_OK && server->protocol_http)
                status = kiln_realm_http_server_attach_with_options_active(
                    socket->resource, kiln_c99_truthy(allow_half_close),
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
            socket->recycled_response = NULL;
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
    if ((encoding.len == 4 && tolower((unsigned char)encoding.ptr[0]) == 'u' &&
         tolower((unsigned char)encoding.ptr[1]) == 't' &&
         tolower((unsigned char)encoding.ptr[2]) == 'f' &&
         encoding.ptr[3] == '8') ||
        (encoding.len == 5 && tolower((unsigned char)encoding.ptr[0]) == 'u' &&
         tolower((unsigned char)encoding.ptr[1]) == 't' &&
         tolower((unsigned char)encoding.ptr[2]) == 'f' && encoding.ptr[3] == '-' &&
         encoding.ptr[4] == '8')) {
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
    size_t width;
    int utf8 = 0;
    int status;
    (void)receiver;
    if (code == KILN_C99_METHOD_NODE_STAT_IS_FILE ||
        code == KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY) {
        kiln_property *marker;
        const char *key = code == KILN_C99_METHOD_NODE_STAT_IS_FILE
                              ? "@@kiln:node:is-file"
                              : "@@kiln:node:is-directory";
        size_t key_len = code == KILN_C99_METHOD_NODE_STAT_IS_FILE ? 19 : 24;
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
            bytes = (const uint8_t *)data.ptr;
            bytes_len = data.len;
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
    if (status != KILN_C99_OK) return status;
    work = (kiln_node_fs_completion *)kiln_c99_gc_calloc(KILN_GC_NODE_FS_COMPLETION, sizeof *work);
    if (work == NULL) return KILN_C99_OUT_OF_MEMORY;
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
    operation.text = (const uint8_t *)path.ptr;
    operation.text_len = path.len;
    operation.bytes = bytes;
    operation.bytes_len = bytes_len;
    if (code == KILN_C99_METHOD_NODE_FS_MKDIR && width == 1) {
        operation.argument = 1;
    }
    status = kiln_realm_start_active_host_op(&operation, kiln_c99_node_fs_complete,
                                             kiln_c99_node_drop_fs_completion, work, &request);
    if (status != KILN_C99_OK) {
        kiln_c99_gc_release(work);
        return status;
    }
    *result = promise;
    return KILN_C99_OK;
}

static int kiln_c99_node_net(kiln_host *host, kiln_value *result, uint64_t code,
                             kiln_value receiver, const kiln_value *args, size_t argc,
                             int depth, kiln_value *thrown) {
    if (code == KILN_C99_METHOD_NODE_NET_CONNECT) {
        kiln_node_socket *socket;
        kiln_node_completion *work;
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
        status = kiln_c99_node_start(work->kind, 0, (uint64_t)port,
                                     host_value.ptr, host_value.len, NULL, 0, work,
                                     &socket->connect_request);
        if (status != KILN_C99_OK) return status;
        *result = socket->object;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_NET_CREATE_SERVER ||
        code == KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER ||
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
            status = kiln_c99_node_start(KILN_REALM_OP_TCP_LISTEN, 0, (uint64_t)port,
                                         host_value.ptr, host_value.len, NULL, 0, work,
                                         &server->listen_request);
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
            if (!((encoding.len == 4 && memcmp(encoding.ptr, "utf8", 4) == 0) ||
                  (encoding.len == 5 && memcmp(encoding.ptr, "utf-8", 5) == 0)))
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
            work->kind = KILN_REALM_OP_TCP_WRITE;
            status = kiln_c99_node_start(KILN_REALM_OP_TCP_WRITE, socket->resource, 0, NULL, 0,
                                         (const uint8_t *)text.ptr, text.len, work,
                                         &socket->write_request);
            if (status != KILN_C99_OK) return status;
            *result = kiln_c99_boolean(1);
            return KILN_C99_OK;
        }
        if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED) {
            kiln_value text = kiln_c99_text(args[0]);
            work->kind = KILN_REALM_OP_TCP_WRITE;
            socket->shutdown_after_write = 1;
            status = kiln_c99_node_socket_ready_state(socket, "readOnly", 8);
            if (status != KILN_C99_OK) {
                kiln_c99_gc_release(work);
                return status;
            }
            status = kiln_c99_node_start(KILN_REALM_OP_TCP_WRITE, socket->resource, 0, NULL, 0,
                                         (const uint8_t *)text.ptr, text.len, work,
                                         &socket->write_request);
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
    for (index = 0; index < headers->count; ++index)
        needed += headers->properties[index].key_len +
                  kiln_c99_text(headers->properties[index].value).len + 2;
    if (needed == 0) return KILN_C99_OK;
    bytes = (char *)malloc(needed);
    if (bytes == NULL) return KILN_C99_OUT_OF_MEMORY;
    for (index = 0; index < headers->count; ++index) {
        kiln_property *property = &headers->properties[index];
        kiln_value value = kiln_c99_text(property->value);
        if (at + property->key_len + value.len + 2 > needed) {
            free(bytes);
            KILN_C99_REFUSE("a header buffer that outgrew the size it was measured at");
        }
        if (property->key_len != 0) memcpy(bytes + at, property->key, property->key_len);
        at += property->key_len;
        bytes[at++] = '\0';
        if (value.len != 0) memcpy(bytes + at, value.ptr, value.len);
        at += value.len;
        bytes[at++] = '\0';
    }
    out->bytes = bytes;
    out->len = at;
    out->capacity = needed;
    return KILN_C99_OK;
}

static int kiln_c99_node_http(kiln_host *host, kiln_value *result, uint64_t code,
                              kiln_value receiver, const kiln_value *args, size_t argc,
                              int depth, kiln_value *thrown) {
    kiln_node_response *response = (kiln_node_response *)kiln_c99_node_hidden_pointer(
        receiver, "@@node:response", 15);
    (void)depth;
    if (code == KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER ||
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
        request->host_name = (char *)malloc(host_value.len);
        request->path = (char *)malloc(path_value.len);
        request->method = (char *)malloc(method_value.len);
        if ((host_value.len != 0 && request->host_name == NULL) ||
            (path_value.len != 0 && request->path == NULL) ||
            (method_value.len != 0 && request->method == NULL)) return KILN_C99_OUT_OF_MEMORY;
        memcpy(request->host_name, host_value.ptr, host_value.len);
        memcpy(request->path, path_value.ptr, path_value.len);
        memcpy(request->method, method_value.ptr, method_value.len);
        request->host_name_len = host_value.len;
        request->path_len = path_value.len;
        request->method_len = method_value.len;
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
            if (status < KILN_C99_OK) return status;
        } else if (span.len != 0) {
            if (kiln_c99_reserve((void **)&request->pending_body,
                                 &request->pending_body_capacity,
                                 request->pending_body_len + span.len, 1) != KILN_C99_OK)
                return KILN_C99_OUT_OF_MEMORY;
            memcpy(request->pending_body + request->pending_body_len, span.bytes, span.len);
            request->pending_body_len += span.len;
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
        kiln_property *slot;
        kiln_value built;
        int status;
        if (receiver.tag != KILN_VALUE_OBJECT)
            return kiln_c99_throw_not_a(thrown, receiver, "IncomingMessage");
        if (code == KILN_C99_METHOD_NODE_REQUEST_HEADERS_SET) {
            built = argc > 0 ? args[0] : kiln_c99_undefined();
        } else {
            status = kiln_c99_new_object_with_capacity(&built, 8);
            if (status != KILN_C99_OK) return status;
            if (response != NULL && response->request_headers != NULL &&
                response->request_headers_len != 0) {
                status = kiln_c99_node_set_hidden_pointer(built, "@@node:lazy-headers", 19,
                                                          response);
                if (status != KILN_C99_OK) return status;
                kiln_c99_node_materialize_all_headers(built);
            }
        }
        status = kiln_c99_own_slot(receiver, "headers", 7, &slot);
        if (status != KILN_C99_OK) return status;
        /* This slot is the accessor `define_accessor` installed when the request was built; the
         * first get or set reaches here and replaces it with the plain data property every read
         * after sees. */
        if (slot->accessor) {
            kiln_c99_object_of(receiver)->accessor_count -= 1;
        }
        slot->accessor = 0;
        slot->getter = kiln_c99_undefined();
        slot->setter = kiln_c99_undefined();
        slot->writable = 1;
        slot->configurable = 1;
        slot->enumerable = 1;
        slot->value = built;
        *result = built;
        return KILN_C99_OK;
    }
    if (response == NULL) return kiln_c99_throw_not_a(thrown, receiver, "ServerResponse");
    if (code == KILN_C99_METHOD_NODE_RESPONSE_SET_HEADER) {
        kiln_value name = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
        kiln_value value = kiln_c99_text(argc > 1 ? args[1] : kiln_c99_undefined());
        char *normalized = name.len == 0 ? NULL : kiln_c99_gc_alloc_text(name.len);
        size_t index;
        int status;
        if (name.len != 0 && normalized == NULL) return KILN_C99_OUT_OF_MEMORY;
        for (index = 0; index < name.len; ++index)
            normalized[index] = (char)tolower((unsigned char)name.ptr[index]);
        status = kiln_c99_put(NULL, response->headers, normalized, name.len, value, 0, NULL);
        *result = receiver;
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_RESPONSE_GET_HEADER ||
        code == KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER ||
        code == KILN_C99_METHOD_NODE_RESPONSE_REMOVE_HEADER) {
        kiln_value name = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
        kiln_object *headers = kiln_c99_object_of(response->headers);
        size_t index;
        size_t byte;
        char *normalized = name.len == 0 ? NULL : kiln_c99_gc_alloc_text(name.len);
        if (headers->shape != NULL) {
            kiln_c99_ensure_dictionary(headers);
            if (code == KILN_C99_METHOD_NODE_RESPONSE_REMOVE_HEADER) {
                headers->shape = NULL;
            }
        }
        if (name.len != 0 && normalized == NULL) return KILN_C99_OUT_OF_MEMORY;
        for (byte = 0; byte < name.len; ++byte)
            normalized[byte] = (char)tolower((unsigned char)name.ptr[byte]);
        for (index = 0; index < headers->count; ++index) {
            kiln_property *property = &headers->properties[index];
            if (property->key_len == name.len &&
                (name.len == 0 || memcmp(property->key, normalized, name.len) == 0)) {
                if (code == KILN_C99_METHOD_NODE_RESPONSE_REMOVE_HEADER) {
                    headers->shape = NULL;
                    if (index + 1 < headers->count)
                        memmove(&headers->properties[index], &headers->properties[index + 1],
                                (headers->count - index - 1) * sizeof *headers->properties);
                    headers->count -= 1;
                    *result = kiln_c99_undefined();
                } else if (code == KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER) {
                    *result = kiln_c99_boolean(1);
                } else {
                    *result = property->value;
                }
                return KILN_C99_OK;
            }
        }
        *result = code == KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER
                      ? kiln_c99_boolean(0)
                      : kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_RESPONSE_GET_HEADERS) {
        kiln_object *headers = kiln_c99_object_of(response->headers);
        kiln_value copy;
        size_t index;
        int status;
        if (headers->shape != NULL) {
            kiln_c99_ensure_dictionary(headers);
        }
        status = kiln_c99_new_object(&copy);
        for (index = 0; status == KILN_C99_OK && index < headers->count; ++index) {
            kiln_property *property = &headers->properties[index];
            status = kiln_c99_put(NULL, copy, property->key, property->key_len,
                                  property->value, 0, NULL);
        }
        if (status != KILN_C99_OK) return status;
        *result = copy;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_RESPONSE_WRITE_HEAD) {
        double status = kiln_c99_as_number(argc > 0 ? args[0] : kiln_c99_undefined());
        if (!(status >= 100.0 && status <= 999.0 && floor(status) == status))
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "Invalid status code");
        response->status = (uint16_t)status;
        status = kiln_c99_put(NULL, response->object, "statusCode", 10,
                              kiln_c99_number((double)response->status), 0, NULL);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_put(NULL, response->object, "headersSent", 11,
                              kiln_c99_boolean(1), 0, NULL);
        if (status != KILN_C99_OK) return status;
        if (argc > 1 && args[1].tag == KILN_VALUE_OBJECT) {
            kiln_object *provided = kiln_c99_object_of(args[1]);
            size_t index;
            if (provided->shape != NULL) {
                kiln_c99_ensure_dictionary(provided);
            }
            for (index = 0; index < provided->count; ++index) {
                kiln_property *property = &provided->properties[index];
                if (property->enumerable) {
                    status = kiln_c99_put(NULL, response->headers, property->key,
                                          property->key_len,
                                          kiln_c99_text(property->value), 0, NULL);
                    if (status != KILN_C99_OK) return status;
                }
            }
        }
        /* Node's writeHead commits the header block, so a later end(body) can no longer declare a
         * length and Node writes Transfer-Encoding: chunked into the block instead. Recording it
         * keeps the committed head identical to Node's; Hyper still owns the encoding and drops
         * the header where no body is allowed. */
        if (response->headers.tag == KILN_VALUE_OBJECT) {
            kiln_object *headers_obj = kiln_c99_object_of(response->headers);
            if (headers_obj->shape != NULL) {
                kiln_c99_ensure_dictionary(headers_obj);
            }
        }
        *result = receiver;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_RESPONSE_WRITE) {
        kiln_value arg = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_c99_node_byte_span span;
        int status;
        status = kiln_c99_node_http_byte_span(arg, &span);
        if (status != KILN_C99_OK) return status;
        if (response->ended)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR,
                                        "write after end");
        if (!response->started) {
            kiln_text flattened;
            kiln_realm_http_response framed;
            status = kiln_c99_node_flatten_headers(kiln_c99_object_of(response->headers),
                                                   &flattened);
            if (status != KILN_C99_OK) return status;
            memset(&framed, 0, sizeof framed);
            framed.status = response->status;
            framed.headers = (const uint8_t *)flattened.bytes;
            framed.headers_len = flattened.len;
            status = kiln_realm_http_response_start_active(response->socket->resource,
                                                           response->request_id, &framed, 16384);
            free(flattened.bytes);
            if (status != KILN_C99_OK) return status;
            response->started = 1;
            status = kiln_c99_put(NULL, response->object, "headersSent", 11,
                                  kiln_c99_boolean(1), 0, NULL);
            if (status != KILN_C99_OK) return status;
        }
        status = kiln_realm_http_response_write_active(response->socket->resource,
                                                       response->request_id,
                                                       span.bytes, span.len);
        if (status < KILN_C99_OK) return status;
        *result = kiln_c99_boolean(status == KILN_C99_OK);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_RESPONSE_END) {
        kiln_object *headers;
        kiln_text flattened = {NULL, 0, 0};
        kiln_realm_http_response framed;
        int status;
        if (response->ended) { *result = receiver; return KILN_C99_OK; }
        if (!response->started) {
            headers = kiln_c99_object_of(response->headers);
            status = kiln_c99_node_flatten_headers(headers, &flattened);
            if (status != KILN_C99_OK) return status;
            memset(&framed, 0, sizeof framed);
            framed.status = response->status;
            framed.headers = (const uint8_t *)flattened.bytes;
            framed.headers_len = flattened.len;
            status = kiln_realm_http_response_start_active(response->socket->resource,
                                                           response->request_id, &framed, 16384);
            free(flattened.bytes);
            if (status != KILN_C99_OK) return status;
            response->started = 1;
            status = kiln_c99_put(NULL, response->object, "headersSent", 11,
                                  kiln_c99_boolean(1), 0, NULL);
            if (status != KILN_C99_OK) return status;
        }
        {
            kiln_value arg = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_c99_node_byte_span span;
            status = kiln_c99_node_http_byte_span(arg, &span);
            if (status != KILN_C99_OK) return status;
            status = kiln_realm_http_response_end_active(response->socket->resource,
                                                         response->request_id,
                                                         span.bytes, span.len);
        }
        if (status < KILN_C99_OK) return status;
        response->ended = 1;
        status = kiln_c99_put(NULL, response->object, "writableEnded", 13,
                              kiln_c99_boolean(1), 0, NULL);
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
    KILN_C99_REFUSE("a Node HTTP method this backend has not implemented");
}
