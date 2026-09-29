/* The handler function a proxy offers for one operation, or undefined when it offers none.
 *
 * Undefined is the *ordinary* answer, not a failure: a handler with no `get` is the common case,
 * and it means the operation runs against the target unchanged. A revoked proxy is the failure,
 * and it is one for every operation alike -- the capability has been withdrawn, and answering
 * from the target would hand it back. */
static int kiln_c99_proxy_trap(kiln_host *host, kiln_value proxy, const char *name,
                               kiln_value *trap, int depth, kiln_value *thrown) {
    kiln_proxy *data = kiln_c99_proxy_of(proxy);
    kiln_value found;
    int status;
    *trap = kiln_c99_undefined();
    if (data->revoked) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot perform operation on a proxy that has been revoked");
    }
    if (data->handler.tag != KILN_VALUE_OBJECT) {
        /* `new Proxy(t, h)` takes any value for the handler and only complains when a trap is
         * looked up on it. This backend has not modelled that, so it says so rather than
         * treating a non-object handler as an empty one. */
        KILN_C99_REFUSE("a Proxy handler that is not an object");
    }
    status = kiln_c99_get(host, data->handler, name, strlen(name), &found, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (kiln_c99_is_callable(found)) {
        *trap = found;
    }
    return KILN_C99_OK;
}

/* `proxy[key]`, through the `get` trap or through the target.
 *
 * The key arrives as a *value* rather than as bytes, because the trap sees what the program
 * wrote: `p[secret]` where `secret` is a Symbol hands the trap that very symbol, and a library
 * hiding its state behind one compares it by identity. Converting it to the reserved slot name
 * first -- which every other read does -- would hand the trap a string it could never match. */
static int kiln_c99_proxy_get(kiln_host *host, kiln_value proxy, kiln_value key,
                              kiln_value receiver, kiln_value *out, int depth,
                              kiln_value *thrown) {
    kiln_value trap;
    kiln_value inner = kiln_c99_proxy_of(proxy)->target;
    kiln_value handler = kiln_c99_proxy_of(proxy)->handler;
    int status = kiln_c99_proxy_trap(host, proxy, "get", &trap, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (trap.tag == KILN_VALUE_UNDEFINED) {
        return kiln_c99_get_with_receiver(host, inner, key, receiver, out, depth, thrown);
    }
    {
        /* The receiver is ordinarily the proxy itself; `Reflect.get` is the one caller that
         * names a different one, and a handler can see the difference. */
        kiln_value arguments[3];
        arguments[0] = inner;
        arguments[1] = key;
        arguments[2] = receiver;
        return kiln_c99_call(host, out, trap, handler, arguments, 3, depth, thrown);
    }
}

/* `[[Get]]` with the receiver named separately.
 *
 * Ordinary bracket access reads with the object itself as the receiver, and the two coincide
 * everywhere except through `Reflect.get`, whose third argument is what an accessor sees as
 * `this` and what a proxy's `get` trap is handed. */
static int kiln_c99_get_with_receiver(kiln_host *host, kiln_value target, kiln_value key,
                                      kiln_value receiver, kiln_value *out, int depth,
                                      kiln_value *thrown) {
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_proxy_get(host, target, key, receiver, out, depth, thrown);
    }
    kiln_value name = kiln_c99_property_key(key);
    if (target.tag == KILN_VALUE_OBJECT &&
        !(receiver.tag == target.tag && receiver.bits == target.bits)) {
        /* Only an accessor can tell the two apart, so only an accessor is looked for here; a
         * data property reads the same either way and goes through the ordinary path. */
        kiln_value current = target;
        size_t steps;
        if (name.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol "
                            "slot");
        }
        for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
            kiln_property property;
            if (current.tag != KILN_VALUE_OBJECT) {
                break;
            }
            if (kiln_c99_property_read_own_value(current, (const kiln_string *)name.ptr,
                                                 NULL, &property)) {
                if (!property.accessor) break;
                if (property.getter.tag == KILN_VALUE_UNDEFINED) {
                    KILN_C99_ABSENT("an accessor with no getter, whose read is undefined by "
                                    "definition",
                                    out);
                }
                return kiln_c99_call(host, out, property.getter, receiver, NULL,
                                     0, depth, thrown);
            }
            current = kiln_c99_object_prototype_read(current);
        }
    }
    /* The keyed fast path is for ordinary objects: elemented targets
     * (arrays above all) read through `get_computed`, which is also what
     * this function did for every target before the headed-key migration
     * added the fast path. Routing an array here skips its elements, so a
     * proxy over one reads every index as undefined. */
    if (name.tag == KILN_VALUE_STRING && target.tag == KILN_VALUE_OBJECT) {
        /* `ikey` is the canonical pointer or NULL (ADR 0047): a computed key is a
         * boundary string the realm may never have stored, and the shaped-slot
         * scan below compares pointers. NULL canonicalizes once for the whole
         * walk inside `kiln_c99_get_own_kind`. */
        return kiln_c99_get_key(host, target, (const kiln_string *)name.ptr, NULL, out,
                                depth, thrown);
    }
    return kiln_c99_get_computed(host, target, key, out, depth, thrown);
}

static int kiln_c99_proxy_set_with_receiver(kiln_host *host, kiln_value proxy, kiln_value key,
                                            kiln_value value, kiln_value receiver, int strict,
                                            int depth, kiln_value *thrown) {
    kiln_value trap;
    kiln_value inner = kiln_c99_proxy_of(proxy)->target;
    kiln_value handler = kiln_c99_proxy_of(proxy)->handler;
    int status = kiln_c99_proxy_trap(host, proxy, "set", &trap, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (trap.tag == KILN_VALUE_UNDEFINED) {
        return kiln_c99_set_with_receiver(host, inner, key, value, receiver, strict, depth, thrown);
    }
    {
        kiln_value arguments[4];
        kiln_value ignored;
        arguments[0] = inner;
        arguments[1] = key;
        arguments[2] = value;
        arguments[3] = receiver;
        status = kiln_c99_call(host, &ignored, trap, handler, arguments, 4, depth, thrown);
        if (status != KILN_C99_OK || !strict || kiln_c99_truthy(ignored)) return status;
        {
            kiln_value text = kiln_c99_property_key(key);
            if (text.tag != KILN_VALUE_STRING) {
                return kiln_c99_refuse("a strict Proxy write refused a key with no string spelling");
            }
            if (kiln_string_is_utf16((const kiln_string *)text.ptr)) {
                return kiln_c99_refuse("a strict Proxy write refused a UTF-16 string key");
            }
            return kiln_c99_throw_read_only(
                thrown, (const char *)kiln_string_latin1((const kiln_string *)text.ptr),
                ((const kiln_string *)text.ptr)->unit_len);
        }
    }
}

static int kiln_c99_set_with_receiver(kiln_host *host, kiln_value target, kiln_value key,
                                      kiln_value value, kiln_value receiver, int strict,
                                      int depth, kiln_value *thrown) {
    if (target.tag == KILN_VALUE_UNDEFINED || target.tag == KILN_VALUE_NULL) {
        return kiln_c99_throw_cannot_set(thrown, target);
    }
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_proxy_set_with_receiver(host, target, key, value, receiver, strict, depth, thrown);
    }
    if (target.tag == KILN_VALUE_OBJECT) {
        kiln_value name = kiln_c99_property_key(key);
        kiln_value current = target;
        size_t steps;
        if (name.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol "
                            "slot");
        }
        for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
            kiln_property property;
            if (current.tag != KILN_VALUE_OBJECT) {
                break;
            }
            if (kiln_c99_property_read_own_value(current, (const kiln_string *)name.ptr,
                                                 NULL, &property)) {
                if (property.accessor) {
                    if (property.setter.tag == KILN_VALUE_UNDEFINED) {
                        if (!strict) return KILN_C99_OK;
                        if (kiln_string_is_utf16((const kiln_string *)name.ptr)) {
                            return kiln_c99_refuse("a strict Proxy write refused a UTF-16 string key");
                        }
                        return kiln_c99_throw_read_only(
                            thrown, (const char *)kiln_string_latin1((const kiln_string *)name.ptr),
                            ((const kiln_string *)name.ptr)->unit_len);
                    }
                    kiln_value ignored;
                    return kiln_c99_call(host, &ignored, property.setter, receiver,
                                         &value, 1, depth, thrown);
                }
                if (!property.writable) {
                    if (!strict) return KILN_C99_OK;
                    if (kiln_string_is_utf16((const kiln_string *)name.ptr)) {
                        return kiln_c99_refuse("a strict Proxy write refused a UTF-16 string key");
                    }
                    return kiln_c99_throw_read_only(
                        thrown, (const char *)kiln_string_latin1((const kiln_string *)name.ptr),
                        ((const kiln_string *)name.ptr)->unit_len);
                }
                break;
            }
            current = kiln_c99_object_prototype_read(current);
        }

        /* Write lands on receiver */
        kiln_value recv_store;
        int store_status = kiln_c99_definition_store(receiver, &recv_store);
        if (store_status != KILN_C99_OK) {
            return strict ? kiln_c99_throw_cannot_set(thrown, receiver) : KILN_C99_OK;
        }
        kiln_object *recv_obj = kiln_c99_object_of(recv_store);
        if (recv_obj->frozen) {
            if (!strict) return KILN_C99_OK;
            if (kiln_string_is_utf16((const kiln_string *)name.ptr)) {
                return kiln_c99_refuse("a strict Proxy write refused a UTF-16 string key");
            }
            return kiln_c99_throw_read_only(
                thrown, (const char *)kiln_string_latin1((const kiln_string *)name.ptr),
                ((const kiln_string *)name.ptr)->unit_len);
        }
        if (recv_obj->shape != NULL) {
            kiln_c99_ensure_dictionary(recv_obj);
        }
        size_t recv_index = kiln_c99_own_index_key(recv_obj, NULL,
                                                   (const kiln_string *)name.ptr);
        if (recv_index != recv_obj->count) {
            if (recv_obj->properties[recv_index].accessor) {
                if (!strict) return KILN_C99_OK;
                if (kiln_string_is_utf16((const kiln_string *)name.ptr)) {
                    return kiln_c99_refuse("a strict Proxy write refused a UTF-16 string key");
                }
                return kiln_c99_throw_read_only(
                    thrown, (const char *)kiln_string_latin1((const kiln_string *)name.ptr),
                    ((const kiln_string *)name.ptr)->unit_len);
            }
            if (!recv_obj->properties[recv_index].writable) {
                if (!strict) return KILN_C99_OK;
                if (kiln_string_is_utf16((const kiln_string *)name.ptr)) {
                    return kiln_c99_refuse("a strict Proxy write refused a UTF-16 string key");
                }
                return kiln_c99_throw_read_only(
                    thrown, (const char *)kiln_string_latin1((const kiln_string *)name.ptr),
                    ((const kiln_string *)name.ptr)->unit_len);
            }
            kiln_c99_heap_store_value(recv_obj, &recv_obj->properties[recv_index].value, value);
            return KILN_C99_OK;
        }
        if (recv_obj->not_extensible) {
            if (!strict) return KILN_C99_OK;
            if (kiln_string_is_utf16((const kiln_string *)name.ptr)) {
                return kiln_c99_refuse("a strict Proxy write refused a UTF-16 string key");
            }
            return kiln_c99_throw_read_only(
                thrown, (const char *)kiln_string_latin1((const kiln_string *)name.ptr),
                ((const kiln_string *)name.ptr)->unit_len);
        }
        kiln_property *slot;
        /* NULL interns here: a computed key is a boundary string, not the
         * canonical pointer `own_slot_key` stores (ADR 0047). */
        int status = kiln_c99_own_slot_key(recv_store, (const kiln_string *)name.ptr,
                                           NULL, &slot);
        if (status != KILN_C99_OK) return status;
        kiln_c99_heap_store_value(kiln_c99_object_of(recv_store), &slot->value, value);
        return KILN_C99_OK;
    }
    return strict ? kiln_c99_throw_cannot_set(thrown, target) : KILN_C99_OK;
}

/* `proxy[key] = value`, through the `set` trap or through the target. The trap's answer is
 * discarded: a strict-mode assignment would throw on a falsy one, and neither engine does. */
static int kiln_c99_proxy_set(kiln_host *host, kiln_value proxy, kiln_value key, kiln_value value,
                              int strict, int depth, kiln_value *thrown) {
    return kiln_c99_proxy_set_with_receiver(host, proxy, key, value, proxy, strict, depth, thrown);
}

/* `key in proxy`, `delete proxy[key]`, `Object.keys(proxy)` and
 * `Object.getOwnPropertyDescriptor(proxy, key)` -- the four remaining operations a handler can
 * intercept, each with the same shape: consult the trap, and run against the target when there
 * is none.
 *
 * `ownKeys` is the one that does more than forward. The trap reports *every* own key, including
 * the ones that are not enumerated, because that is what `Reflect.ownKeys` means -- and
 * `Object.keys` and `for...in` want only the enumerable ones. So each key the trap named is put
 * back through `[[GetOwnProperty]]`, which is a second trip through the handler and is exactly
 * what the language specifies. */
static int kiln_c99_proxy_has(kiln_host *host, kiln_value proxy, kiln_value key, int *answer,
                              int depth, kiln_value *thrown) {
    kiln_value trap;
    kiln_value inner = kiln_c99_proxy_of(proxy)->target;
    kiln_value handler = kiln_c99_proxy_of(proxy)->handler;
    int status = kiln_c99_proxy_trap(host, proxy, "has", &trap, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (trap.tag == KILN_VALUE_UNDEFINED) {
        return kiln_c99_has_computed(host, inner, key, answer, depth, thrown);
    }
    {
        kiln_value arguments[2];
        kiln_value answered;
        arguments[0] = inner;
        arguments[1] = key;
        status = kiln_c99_call(host, &answered, trap, handler, arguments, 2, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        *answer = kiln_c99_truthy(answered);
        return KILN_C99_OK;
    }
}

static int kiln_c99_proxy_delete(kiln_host *host, kiln_value proxy, kiln_value key, int *answer,
                                 int depth, kiln_value *thrown) {
    kiln_value trap;
    kiln_value inner = kiln_c99_proxy_of(proxy)->target;
    kiln_value handler = kiln_c99_proxy_of(proxy)->handler;
    int status = kiln_c99_proxy_trap(host, proxy, "deleteProperty", &trap, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (trap.tag == KILN_VALUE_UNDEFINED) {
        return kiln_c99_delete_computed(host, inner, key, answer, depth, thrown);
    }
    {
        kiln_value arguments[2];
        kiln_value answered;
        arguments[0] = inner;
        arguments[1] = key;
        status = kiln_c99_call(host, &answered, trap, handler, arguments, 2, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        *answer = kiln_c99_truthy(answered);
        return KILN_C99_OK;
    }
}

static int kiln_c99_proxy_describe(kiln_host *host, kiln_value proxy, kiln_value key,
                                   kiln_value *out, int *found, int depth, kiln_value *thrown) {
    kiln_value trap;
    kiln_value inner = kiln_c99_proxy_of(proxy)->target;
    kiln_value handler = kiln_c99_proxy_of(proxy)->handler;
    int status = kiln_c99_proxy_trap(host, proxy, "getOwnPropertyDescriptor", &trap, depth,
                                     thrown);
    *found = 0;
    *out = kiln_c99_undefined();
    if (status != KILN_C99_OK) {
        return status;
    }
    if (trap.tag == KILN_VALUE_UNDEFINED) {
        kiln_value name = kiln_c99_property_key(key);
        if (name.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol "
                            "slot");
        }
        return kiln_c99_describe_property_key(host, inner, (const kiln_string *)name.ptr,
                                              (const kiln_string *)name.ptr, out, found, depth,
                                              thrown);
    }
    {
        kiln_value arguments[2];
        arguments[0] = inner;
        arguments[1] = key;
        status = kiln_c99_call(host, out, trap, handler, arguments, 2, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        /* Undefined means the name is not an own property -- the same answer the operation gives
         * for an ordinary object, and the reason presence is reported separately. */
        *found = out->tag != KILN_VALUE_UNDEFINED;
        return KILN_C99_OK;
    }
}

static int kiln_c99_proxy_define(kiln_host *host, kiln_value proxy, kiln_value key,
                                 kiln_value descriptor, int depth, kiln_value *thrown) {
    kiln_value trap;
    kiln_value inner = kiln_c99_proxy_of(proxy)->target;
    kiln_value handler = kiln_c99_proxy_of(proxy)->handler;
    int status = kiln_c99_proxy_trap(host, proxy, "defineProperty", &trap, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (trap.tag == KILN_VALUE_UNDEFINED) {
        kiln_value name = kiln_c99_property_key(key);
        if (name.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol "
                            "slot");
        }
        return kiln_c99_define_property_key(host, inner, (const kiln_string *)name.ptr,
                                            (const kiln_string *)name.ptr, descriptor, depth,
                                            thrown);
    }
    {
        kiln_value arguments[3];
        kiln_value ignored;
        arguments[0] = inner;
        arguments[1] = key;
        arguments[2] = descriptor;
        return kiln_c99_call(host, &ignored, trap, handler, arguments, 3, depth, thrown);
    }
}

/* Every own key a proxy reports, unfiltered -- what `Reflect.ownKeys` answers with. */
static int kiln_c99_proxy_all_keys(kiln_host *host, kiln_value proxy, kiln_value *out, int depth,
                                   kiln_value *thrown) {
    kiln_value trap;
    kiln_value inner = kiln_c99_proxy_of(proxy)->target;
    kiln_value handler = kiln_c99_proxy_of(proxy)->handler;
    int status = kiln_c99_proxy_trap(host, proxy, "ownKeys", &trap, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (trap.tag == KILN_VALUE_UNDEFINED) {
        return kiln_c99_all_own_keys(host, inner, out, depth, thrown);
    }
    status = kiln_c99_call(host, out, trap, handler, &inner, 1, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (out->tag != KILN_VALUE_ARRAY) {
        /* The specification requires a list; a handler that returns anything else is a
         * TypeError there and a gap here rather than a silent empty answer. */
        KILN_C99_REFUSE("a Proxy ownKeys trap that answered something other than an array");
    }
    return KILN_C99_OK;
}

static int kiln_c99_proxy_own_keys(kiln_host *host, kiln_value proxy, kiln_value *out, int depth,
                                   kiln_value *thrown) {
    kiln_value listed;
    kiln_value kept;
    size_t index;
    int status = kiln_c99_proxy_all_keys(host, proxy, &listed, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_new_array(&kept);
    if (status != KILN_C99_OK) {
        return status;
    }
    for (index = 0; index < kiln_c99_array_of(listed)->count; ++index) {
        kiln_value descriptor;
        kiln_value enumerable;
        int found;
        /* Re-read each time: a `getOwnPropertyDescriptor` trap is a program function and may
         * have grown the list the handler handed back. */
        kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(listed)->elements[index]);
        if (key.tag == KILN_VALUE_SYMBOL) {
            /* `Reflect.ownKeys` reports symbols and this does not: the enumerable string keys
             * are what `Object.keys` and `for...in` walk. */
            continue;
        }
        status = kiln_c99_proxy_describe(host, proxy, key, &descriptor, &found, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (!found) {
            continue;
        }
        status = kiln_c99_get(host, descriptor, "enumerable", 10, &enumerable, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (!kiln_c99_truthy(enumerable)) {
            continue;
        }
        if (key.tag == KILN_VALUE_STRING) {
            const kiln_string *view = (const kiln_string *)key.ptr;
            key = kiln_c99_key_value(view, NULL);
            if (key.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        }
        status = kiln_c99_array_push(kept, key);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    *out = kept;
    return KILN_C99_OK;
}
