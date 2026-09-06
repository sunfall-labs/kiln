
static int kiln_c99_buffer_from(kiln_value *out, kiln_value source, kiln_value *thrown) {
    kiln_value built;
    size_t index;
    int status;
    if (source.tag == KILN_VALUE_STRING) {
        status = kiln_c99_typed_alloc(&built, KILN_TYPED_UINT8, source.len);
        if (status != KILN_C99_OK) return status;
        for (index = 0; index < source.len; ++index) {
            kiln_c99_typed_set(kiln_c99_typed_array_of(built), index,
                               (double)(unsigned char)source.ptr[index]);
        }
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
    if (code == KILN_C99_METHOD_BUFFER_FROM) return kiln_c99_buffer_from(out, first, thrown);
    if (code == KILN_C99_METHOD_BUFFER_ALLOC) {
        size_t length;
        int status = kiln_c99_to_index(first, &length, thrown);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_typed_alloc(out, KILN_TYPED_UINT8, length);
        if (status == KILN_C99_OK) kiln_c99_typed_array_of(*out)->is_buffer = 1;
        return status;
    }
    if (code == KILN_C99_METHOD_BUFFER_IS_BUFFER) {
        *out = kiln_c99_boolean(first.tag == KILN_VALUE_TYPED_ARRAY &&
                                kiln_c99_typed_array_of(first)->is_buffer);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_BUFFER_BYTE_LENGTH) {
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
        status = kiln_c99_typed_alloc(out, KILN_TYPED_UINT8, total);
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
    if (code == KILN_C99_METHOD_BUFFER_TO_STRING) {
        kiln_typed_array *view;
        kiln_value encoding = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_copy_text("utf8", 4);
        if (receiver.tag != KILN_VALUE_TYPED_ARRAY ||
            !(encoding.len == 4 && memcmp(encoding.ptr, "utf8", 4) == 0) )
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "unsupported Buffer encoding");
        view = kiln_c99_typed_array_of(receiver);
        *out = kiln_c99_copy_text((const char *)view->buffer->bytes + view->byte_offset,
                                  view->length);
        return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
    }
    KILN_C99_REFUSE("a Buffer method this backend has not implemented");
}

static int kiln_c99_node_dns_lookup(kiln_host *host, kiln_value *out,
                                    const kiln_value *args, size_t argc, int depth,
                                    kiln_value *thrown) {
    kiln_value name = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_copy_text("localhost", 9);
    kiln_value callback = kiln_c99_undefined();
    kiln_value options = argc > 1 ? args[1] : kiln_c99_undefined();
    kiln_value addresses;
    kiln_value callback_args[3];
    size_t at;
    int all = 0;
    int status;
    for (at = argc; at > 0; --at) {
        if (kiln_c99_is_callable(args[at - 1])) {
            callback = args[at - 1];
            break;
        }
    }
    if (!kiln_c99_is_callable(callback))
        return kiln_c99_throw_not_a(thrown, callback, "function");
    if (name.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    if (options.tag == KILN_VALUE_OBJECT) {
        kiln_value read;
        status = kiln_c99_get(host, options, "all", 3, &read, depth, thrown);
        if (status != KILN_C99_OK) return status;
        all = kiln_c99_truthy(read);
    }
    status = kiln_c99_new_array(&addresses);
    if (status != KILN_C99_OK) return status;
    if ((name.len == 9 && memcmp(name.ptr, "localhost", 9) == 0) ||
        (name.len == 9 && memcmp(name.ptr, "127.0.0.1", 9) == 0)) {
        kiln_value address;
        status = kiln_c99_new_object(&address);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, address, "address", 7,
                                  kiln_c99_copy_text("127.0.0.1", 9), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, address, "family", 6, kiln_c99_number(4.0), 0, NULL);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(addresses, address);
    }
    if ((name.len == 9 && memcmp(name.ptr, "localhost", 9) == 0) ||
        (name.len == 3 && memcmp(name.ptr, "::1", 3) == 0)) {
        kiln_value address;
        status = kiln_c99_new_object(&address);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, address, "address", 7,
                                  kiln_c99_copy_text("::1", 3), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, address, "family", 6, kiln_c99_number(6.0), 0, NULL);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(addresses, address);
    }
    if (status != KILN_C99_OK) return status;
    if (kiln_c99_array_of(addresses)->count == 0) {
        callback_args[0] = kiln_c99_copy_text("ENOTFOUND", 9);
        callback_args[1] = addresses;
        status = kiln_c99_call(host, out, callback, kiln_c99_undefined(), callback_args, 2,
                               depth, thrown);
    } else if (all) {
        callback_args[0] = kiln_c99_null();
        callback_args[1] = addresses;
        status = kiln_c99_call(host, out, callback, kiln_c99_undefined(), callback_args, 2,
                               depth, thrown);
    } else {
        kiln_value first = kiln_val_to_boundary(kiln_c99_array_of(addresses)->elements[0]);
        callback_args[0] = kiln_c99_null();
        status = kiln_c99_get(host, first, "address", 7, &callback_args[1], depth, thrown);
        if (status == KILN_C99_OK)
            status = kiln_c99_get(host, first, "family", 6, &callback_args[2], depth, thrown);
        if (status == KILN_C99_OK)
            status = kiln_c99_call(host, out, callback, kiln_c99_undefined(), callback_args, 3,
                                   depth, thrown);
    }
    if (status == KILN_C99_OK) *out = kiln_c99_undefined();
    return status;
}

static int kiln_c99_node_channel(kiln_value *out) {
    int status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "hasSubscribers", 14, kiln_c99_boolean(0), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(
            *out, "publish", 7, kiln_c99_method(KILN_C99_METHOD_NODE_DIAGNOSTICS_PUBLISH));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(
            *out, "runStores", 9, kiln_c99_method(KILN_C99_METHOD_NODE_DIAGNOSTICS_RUN_STORES));
    return status;
}

static int kiln_c99_node_diagnostics(kiln_host *host, kiln_value *out, uint64_t code,
                                     const kiln_value *args, size_t argc, int depth,
                                     kiln_value *thrown) {
    if (code == KILN_C99_METHOD_NODE_DIAGNOSTICS_CHANNEL)
        return kiln_c99_node_channel(out);
    if (code == KILN_C99_METHOD_NODE_DIAGNOSTICS_TRACING_CHANNEL) {
        static const char *const names[] = {"start", "end", "asyncStart", "asyncEnd", "error"};
        size_t index;
        int status = kiln_c99_new_object(out);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, *out, "hasSubscribers", 14,
                                  kiln_c99_boolean(0), 0, NULL);
        for (index = 0; status == KILN_C99_OK && index < sizeof names / sizeof *names; ++index) {
            kiln_value channel;
            status = kiln_c99_node_channel(&channel);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, *out, names[index], strlen(names[index]), channel,
                                      0, NULL);
        }
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_DIAGNOSTICS_PUBLISH) {
        KILN_C99_ABSENT("not a member read: diagnostics_channel publish answers undefined", out);
    }
    if (code == KILN_C99_METHOD_NODE_DIAGNOSTICS_RUN_STORES) {
        kiln_value callback = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_value receiver = argc > 2 ? args[2] : kiln_c99_undefined();
        if (!kiln_c99_is_callable(callback))
            return kiln_c99_throw_not_a(thrown, callback, "function");
        return kiln_c99_call(host, out, callback, receiver, argc > 3 ? args + 3 : NULL,
                             argc > 3 ? argc - 3 : 0, depth, thrown);
    }
    KILN_C99_REFUSE("a diagnostics_channel member this backend has not implemented");
}

static int kiln_c99_node_util(kiln_host *host, kiln_value *out, uint64_t code,
                              const kiln_value *args, size_t argc, int depth,
                              kiln_value *thrown) {
    size_t index;
    if (code == KILN_C99_METHOD_NODE_UTIL_DEBUGLOG) {
        *out = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_NOOP);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_UTIL_NOOP) {
        KILN_C99_ABSENT("not a member read: the util no-op answers undefined", out);
    }
    if (code == KILN_C99_METHOD_NODE_UTIL_DEPRECATE) {
        *out = argc > 0 ? args[0] : kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_UTIL_INSPECT) {
        *out = kiln_c99_inspect(argc > 0 ? args[0] : kiln_c99_undefined());
        return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
    }
    if (code == KILN_C99_METHOD_NODE_UTIL_INHERITS) {
        kiln_value constructor = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value parent = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_value constructor_prototype;
        kiln_value parent_prototype;
        int status;
        if (!kiln_c99_is_callable(constructor) || !kiln_c99_is_callable(parent))
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "The constructor and super constructor must be functions");
        status = kiln_c99_callable_prototype(constructor, &constructor_prototype);
        if (status == KILN_C99_OK)
            status = kiln_c99_callable_prototype(parent, &parent_prototype);
        if (status == KILN_C99_OK)
            status = kiln_c99_set_prototype(constructor_prototype, parent_prototype);
        *out = kiln_c99_undefined();
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_UTIL_FORMAT) {
        kiln_text built = {NULL, 0, 0};
        size_t next = 1;
        if (argc == 0) {
            *out = kiln_c99_copy_text("", 0);
            return KILN_C99_OK;
        }
        if (args[0].tag != KILN_VALUE_STRING) {
            for (index = 0; index < argc; ++index) {
                kiln_value shown = kiln_c99_inspect(args[index]);
                if ((index != 0 && kiln_c99_text_append_c(&built, " ") != KILN_C99_OK) ||
                    shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_text_append(&built, shown.ptr, shown.len) != KILN_C99_OK) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
        } else {
            for (index = 0; index < args[0].len; ++index) {
                char character = args[0].ptr[index];
                if (character != '%' || index + 1 >= args[0].len) {
                    if (kiln_c99_text_append(&built, &character, 1) != KILN_C99_OK) goto format_oom;
                    continue;
                }
                character = args[0].ptr[++index];
                if (character == '%') {
                    if (kiln_c99_text_append_c(&built, "%") != KILN_C99_OK) goto format_oom;
                    continue;
                }
                if (next >= argc) {
                    if (kiln_c99_text_append_c(&built, "%") != KILN_C99_OK ||
                        kiln_c99_text_append(&built, &character, 1) != KILN_C99_OK) goto format_oom;
                    continue;
                }
                {
                    kiln_value value = args[next++];
                    kiln_value shown = character == 's' ? kiln_c99_text(value) :
                                       (character == 'd' || character == 'i' || character == 'f') ?
                                           kiln_c99_text(kiln_c99_number(kiln_c99_as_number(value))) :
                                       (character == 'j' || character == 'o' || character == 'O') ?
                                           kiln_c99_inspect(value) : kiln_c99_undefined();
                    if (shown.tag == KILN_VALUE_UNDEFINED) {
                        if (kiln_c99_text_append_c(&built, "%") != KILN_C99_OK ||
                            kiln_c99_text_append(&built, &character, 1) != KILN_C99_OK) goto format_oom;
                        next -= 1;
                    } else if (shown.tag != KILN_VALUE_STRING ||
                               kiln_c99_text_append(&built, shown.ptr, shown.len) != KILN_C99_OK) {
                        goto format_oom;
                    }
                }
            }
            while (next < argc) {
                kiln_value shown = kiln_c99_inspect(args[next++]);
                if (kiln_c99_text_append_c(&built, " ") != KILN_C99_OK ||
                    shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_text_append(&built, shown.ptr, shown.len) != KILN_C99_OK)
                    goto format_oom;
            }
        }
        *out = kiln_c99_text_value(&built);
        return KILN_C99_OK;
format_oom:
        free(built.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    (void)host;
    (void)depth;
    KILN_C99_REFUSE("a util member this backend has not implemented");
}
