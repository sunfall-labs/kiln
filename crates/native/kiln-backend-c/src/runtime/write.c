
/* `kiln_c99_put_mode` with a site. A hit is an own writable data property on an object that is
 * neither frozen nor a module namespace, which is the case the slow path answers by assigning
 * in place after its own-list scan; every other case, a setter on the chain included, is the
 * slow path's. */
static int kiln_c99_put_site_key(kiln_host *host, kiln_value target, const kiln_string *key,
                                 const kiln_string *canonical, kiln_value value, int strict,
                                 int depth, kiln_value *thrown, kiln_c99_site *site) {
    const kiln_string *stable = canonical;
    if (key == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (stable == NULL) {
        stable = kiln_c99_intern_key(key);
        if (stable == NULL) return KILN_C99_OUT_OF_MEMORY;
    }
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    int status;
    if (site == NULL || target.tag != KILN_VALUE_OBJECT) {
        return kiln_c99_put_mode_key(host, target, key, stable, value, strict, depth, thrown);
    }
    {
        kiln_object *object = kiln_c99_object_of(target);
        if (object->sealed != NULL)
            return kiln_c99_put_mode_key(host, target, key, stable, value, strict, depth,
                                         thrown);
        if (object->shape != NULL && !object->frozen && !object->module_namespace) {
            const kiln_c99_pic_entry *entry = kiln_c99_site_entry(site, object->shape);
            if (entry != NULL) {
                /* A published entry never changes; these are its words. */
                const uint32_t slot = entry->slot;
                const kiln_shape *next_shape = entry->transition_to;
                if (next_shape == NULL) {
                    /* Polymorphic existing-property hit */
                    if (slot < object->slot_capacity) {
                        if ((object->shape->double_slots_mask & (1u << slot)) &&
                            value.tag != KILN_VALUE_NUMBER) {
                            kiln_c99_ensure_dictionary(object);
                            return kiln_c99_put_site_key(host, target, key, stable, value, strict, depth, thrown, site);
                        }
                        kiln_c99_heap_store_val(object, &object->slots[slot], kiln_val_from_boundary(value));
                        return KILN_C99_OK;
                    }
                } else if (!object->not_extensible && slot < object->slot_capacity &&
                           kiln_c99_transition_store_clear(site, entry, object, stable, key)) {
                    /* Polymorphic transitioning store hit; a non-double written to a double slot
                     * falls through. */
                    if (!((next_shape->double_slots_mask & (1u << slot)) &&
                          value.tag != KILN_VALUE_NUMBER)) {
                        kiln_c99_heap_store_val(object, &object->slots[slot], kiln_val_from_boundary(value));
                        object->shape = (kiln_shape *)next_shape;
                        return KILN_C99_OK;
                    }
                }
            }
            {
                uint32_t slot;
                if (kiln_c99_shape_find_slot(object->shape, stable, &slot) &&
                    (object->shape->store_mask & (1u << slot)) == 0) {
                    if (slot < object->slot_capacity) {
                        kiln_c99_heap_store_val(object, &object->slots[slot], kiln_val_from_boundary(value));
                        if (kiln_c99_site_count(site) < KILN_C99_PIC_SIZE) {
                            kiln_c99_site_remember_own(site, object->shape, slot);
                        }
                        return KILN_C99_OK;
                    }
                }
            }
        }
        uint32_t own = kiln_c99_site_decode_dictionary_index(kiln_c99_site_own(site));
        if (own < object->count && object->properties != NULL &&
            object->properties[own].key == stable && !object->frozen && !object->module_namespace) {
            kiln_property *property = &object->properties[own];
            if (!property->accessor && property->writable) {
                kiln_c99_heap_store_value(object, &property->value, value);
                return KILN_C99_OK;
            }
        }
        {
            const kiln_shape *orig_shape = object->shape;
            status = kiln_c99_put_mode_key(host, target, key, stable, value, strict, depth,
                                           thrown);
            if (status == KILN_C99_OK && kiln_c99_site_misses(site) < KILN_C99_SITE_GIVE_UP) {
                if (object->shape != NULL) {
                    if (orig_shape != NULL && object->shape != orig_shape) {
                        /* Only the one edge this write took: a setter that ran instead may have
                         * moved the object along some other key. */
                        if (object->shape->parent != orig_shape || object->shape->ikey != stable)
                            return status;
                        if (kiln_c99_site_may_learn(site, orig_shape)) {
                            kiln_c99_site_remember_transition(site, orig_shape, object->shape,
                                                              object->shape->slot_index);
                        }
                    } else {
                        uint32_t slot;
                        if (kiln_c99_shape_find_slot(object->shape, stable, &slot)) {
                            /* A read-only slot or an accessor's pair is never stored into. */
                            if (kiln_c99_site_may_learn(site, object->shape) &&
                                (object->shape->store_mask & (1u << slot)) == 0) {
                                kiln_c99_site_remember_own(site, object->shape, slot);
                            }
                        } else if (!kiln_c99_site_learn(site, object, key, stable)) {
                            kiln_c99_site_missed(site);
                        }
                    }
                } else if (!kiln_c99_site_learn(site, object, key, stable)) {
                    kiln_c99_site_missed(site);
                }
            }
            return status;
        }
    }
}

/* Byte-only callers are fixed-name adapters. The borrowed header is consumed synchronously and
 * the headed implementation interns before it can enter a proxy or retain the key. */
static int kiln_c99_put_site(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                             kiln_value value, int strict, int depth, kiln_value *thrown,
                             kiln_c99_site *site) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    return kiln_c99_put_site_key(host, target, &view, NULL, value, strict, depth, thrown, site);
}

/* `ToPropertyKey` of a computed key that arrived from the program.
 *
 * Only an object does any work. `kiln_c99_property_key` cannot do it: it answers
 * "[object Object]" for every object, so `o[{toString(){return "hello"}}] = 42` wrote under
 * that name and `o.hello` read back `undefined` -- a silent wrong answer of exactly the kind
 * the native backend must not produce. A symbol is already a key and must not be stringified,
 * and a number stays a number so the array fast path still recognises it.
 *
 * A nullish target passes through unreduced: the read or write that follows throws for it, and
 * the specification performs that check before the key converts. */
static int kiln_c99_reduce_key(kiln_host *host, kiln_value target, kiln_value key,
                               kiln_value *out, int depth, kiln_value *thrown) {
    *out = key;
    if (target.tag == KILN_VALUE_UNDEFINED || target.tag == KILN_VALUE_NULL) {
        return KILN_C99_OK;
    }
    if (!kiln_c99_is_reference(key)) {
        return KILN_C99_OK;
    }
    return kiln_c99_to_primitive(host, key, 1, out, depth, thrown);
}

static int kiln_c99_get_computed(kiln_host *host, kiln_value target, kiln_value key,
                                 kiln_value *out, int depth, kiln_value *thrown) {
    double index;
    {
        int reduced = kiln_c99_reduce_key(host, target, key, &key, depth, thrown);
        if (reduced != KILN_C99_OK) {
            return reduced;
        }
    }
    /* Before the key is reduced to a name: a Symbol reaches the trap as itself. */
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_proxy_get(host, target, key, target, out, depth, thrown);
    }
    index = kiln_c99_array_index(key);
    if (target.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(target);
        if (index < 0.0) {
            /* A symbol names its reserved slot rather than its description, which is the same
             * conversion every other computed access makes. Without it a symbol key was not a
             * string and reached the general read below as a *nameless* one, so
             * `a[Symbol.iterator]` refused where `[].values` -- the very same function -- did
             * not. */
            if (key.tag == KILN_VALUE_SYMBOL) {
                kiln_value name = kiln_c99_property_key(key);
                const kiln_string *name_key;
                if (name.tag != KILN_VALUE_STRING) {
                    KILN_C99_REFUSE("an array read through a symbol key with no reserved slot to "
                                    "name it");
                }
                name_key = (const kiln_string *)name.ptr;
                if (array->properties.tag == KILN_VALUE_OBJECT) {
                    kiln_object *store = kiln_c99_object_of(array->properties);
                    size_t slot;
                    if (store->shape != NULL) {
                        kiln_c99_ensure_dictionary(store);
                    }
                    for (slot = 0; slot < store->count; ++slot) {
                        if (kiln_c99_same_key_view(&store->properties[slot], name_key)) {
                            *out = store->properties[slot].value;
                            return KILN_C99_OK;
                        }
                    }
                }
                /* `@@iterator` is the one reserved slot an array carries of its own, and the
                 * general read answers it. Any other that nobody set is *absent*, and absent is
                 * undefined -- not the refusal an unknown ordinary name gets, which is there to
                 * catch a method this backend has not grown and which a symbol can never be. */
                if (!kiln_c99_same_name_key(name_key, "@@iterator") &&
                    !kiln_c99_same_name_key(name_key, "@@unscopables")) {
                    KILN_C99_ABSENT("a reserved slot nobody set on this object", out);
                }
                return kiln_c99_get_key(host, target, name_key, kiln_c99_property_ikey(key), out,
                                        depth, thrown);
            }
            /* Reserved protocol slots are reached through Symbols. An ordinary string that only
             * happens to have the same spelling remains an ordinary named property. */
            if (key.tag == KILN_VALUE_STRING &&
                kiln_c99_key_is_reserved((const kiln_string *)key.ptr)) {
                if (array->properties.tag == KILN_VALUE_OBJECT) {
                    kiln_object *store = kiln_c99_object_of(array->properties);
                    size_t slot;
                    if (store->shape != NULL) {
                        kiln_c99_ensure_dictionary(store);
                    }
                    for (slot = 0; slot < store->count; ++slot) {
                        if (kiln_c99_same_key_view(&store->properties[slot],
                                                   (const kiln_string *)key.ptr)) {
                            *out = store->properties[slot].value;
                            return KILN_C99_OK;
                        }
                    }
                }
                KILN_C99_ABSENT("a reserved slot nobody set on this array", out);
            }
            /* A negative number is not an element: it names a property ("-2")
             * the array does not have, which reads as undefined rather than
             * failing argument validation. */
            if (key.tag == KILN_VALUE_NUMBER) {
                kiln_value text = kiln_c99_property_key(key);
                if (text.tag != KILN_VALUE_STRING) {
                    KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol "
                                    "slot");
                }
                return kiln_c99_get_key(host, target, (const kiln_string *)text.ptr, NULL,
                                        out, depth, thrown);
            }
            return key.tag == KILN_VALUE_STRING
                       ? kiln_c99_get_key(host, target, (const kiln_string *)key.ptr, NULL, out,
                                          depth, thrown)
                       : KILN_C99_INVALID_ARGUMENT;
        }
        if ((size_t)index < array->count && array->elements[(size_t)index] != KILN_VAL_HOLE) {
            *out = kiln_val_to_boundary(array->elements[(size_t)index]);
            return KILN_C99_OK;
        }
        /* Missing elements must still observe subclass and Array.prototype indices. */
        kiln_value name = kiln_c99_property_key(key);
        if (name.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        return kiln_c99_get_key(host, target, (const kiln_string *)name.ptr, NULL,
                                out, depth, thrown);
    }
    if (target.tag == KILN_VALUE_STRING && index >= 0.0) {
        kiln_c99_utf16_iter iterator = {target.ptr, target.len, 0, 0, 0};
        size_t position = 0;
        uint16_t unit;
        while (kiln_c99_utf16_next(&iterator, &unit)) {
            if (position == (size_t)index) {
                /* One code unit, encoded the way every other single-unit result is.
                 *
                 * `s[i]` and `s.charAt(i)` are the same character by definition, so they cannot
                 * encode it differently: writing a lone surrogate as the raw three-byte form of
                 * its own code point made `pair[1] === pair.charAt(1)` false and gave the result
                 * a `length` of 3, because nothing downstream reads that spelling back as one
                 * unit. `kiln_c99_from_units` is what `charAt` uses and it is what decides how a
                 * lone surrogate is carried, so this defers to it rather than restating it. */
                *out = kiln_c99_from_units(&unit, 1);
                return KILN_C99_OK;
            }
            position += 1;
        }
        KILN_C99_ABSENT("a string index past the end", out);
    }
    if (target.tag == KILN_VALUE_TYPED_ARRAY && index >= 0.0) {
        kiln_typed_array *view = kiln_c99_typed_array_of(target);
        if ((size_t)index < view->length) {
            return kiln_c99_typed_element(view, (size_t)index, out);
        } else {
            *out = kiln_c99_undefined();
        }
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_TYPED_ARRAY && key.tag == KILN_VALUE_STRING &&
        kiln_c99_key_is_reserved((const kiln_string *)key.ptr)) {
        KILN_C99_ABSENT("a reserved slot a typed array does not carry", out);
    }
    if (target.tag == KILN_VALUE_OBJECT && index >= 0.0 && index < 100000.0) {
        const kiln_object *object = kiln_c99_object_of(target);
        if (object->shape != NULL && object->elements != NULL) {
            size_t elem_idx = (size_t)index;
            if (elem_idx < object->element_count && object->elements[elem_idx] != KILN_VAL_HOLE) {
                *out = kiln_val_to_boundary(object->elements[elem_idx]);
                return KILN_C99_OK;
            }
        }
    }
    {
        /* An object key is a string, whatever it was written as -- and a symbol's is the
         * reserved slot it names rather than the way it prints. */
        kiln_value text = kiln_c99_property_key(key);
        if (text.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol "
                            "slot");
        }
        return kiln_c99_get_key(host, target, (const kiln_string *)text.ptr,
                                kiln_c99_property_ikey(key), out, depth, thrown);
    }
}

static int kiln_c99_set_computed_mode(kiln_host *host, kiln_value target, kiln_value key,
                                      kiln_value value, int strict, int depth,
                                      kiln_value *thrown) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    double index;
    {
        int reduced = kiln_c99_reduce_key(host, target, key, &key, depth, thrown);
        if (reduced != KILN_C99_OK) {
            return reduced;
        }
    }
    if (target.tag == KILN_VALUE_PROXY) {
        return kiln_c99_proxy_set(host, target, key, value, strict, depth, thrown);
    }
    index = kiln_c99_array_index(key);
    if (target.tag == KILN_VALUE_ARRAY && index >= 0.0) {
        kiln_array *array = kiln_c99_array_of(target);
        size_t slot = (size_t)index;
        size_t fill;
        int status;
        if (array->frozen) {
            kiln_value text = kiln_c99_property_key(key);
            return strict && text.tag == KILN_VALUE_STRING
                       ? kiln_c99_throw_read_only_key(thrown, (const kiln_string *)text.ptr)
                       : KILN_C99_OK;
        }
        status = kiln_c99_array_reserve(array, slot + 1);
        if (status != KILN_C99_OK) {
            return status;
        }
        /* Growing leaves holes, which read as undefined and are not the same thing. */
        if (slot > array->count) {
            array->is_numeric = 0;
        }
        for (fill = array->count; fill < slot; ++fill) {
            kiln_c99_heap_store_val(array, &array->elements[fill], KILN_VAL_HOLE);
        }
        kiln_c99_heap_store_val(array, &array->elements[slot], kiln_val_from_boundary(value));
        if (array->is_numeric && !KILN_VAL_IS_NUMBER(array->elements[slot])) {
            array->is_numeric = 0;
        }
        if (slot + 1 > array->count) {
            array->count = slot + 1;
        }
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_TYPED_ARRAY && index >= 0.0) {
        kiln_typed_array *view = kiln_c99_typed_array_of(target);
        if (value.tag == KILN_VALUE_NUMBER && !kiln_c99_typed_kind_is_bigint(view->kind)) {
            kiln_c99_typed_set(view, (size_t)index, kiln_c99_as_number(value));
            return KILN_C99_OK;
        }
        /* Anything else takes the conversion that can call back into the program: the pure
         * `kiln_c99_as_number` stored NaN for `{ valueOf() { return 7; } }` without calling it,
         * and swallowed what a throwing `valueOf` threw. */
        return kiln_c99_typed_store(host, view, (size_t)index, value, depth, thrown);
    }
    if (target.tag == KILN_VALUE_OBJECT && index >= 0.0 && index < 100000.0) {
        kiln_object *object = kiln_c99_object_of(target);
        if (object->shape != NULL && !object->frozen) {
            size_t elem_idx = (size_t)index;
            if (object->elements != NULL && elem_idx < object->element_count && object->elements[elem_idx] != KILN_VAL_HOLE) {
                kiln_c99_heap_store_val(object, &object->elements[elem_idx], kiln_val_from_boundary(value));
                return KILN_C99_OK;
            }
            if (!object->not_extensible && (object->prototype.tag == KILN_VALUE_UNDEFINED || !kiln_c99_has_inherited_setter_or_readonly(object->prototype))) {
                if (elem_idx >= object->element_capacity) {
                    size_t new_cap = object->element_capacity == 0 ? 8 : object->element_capacity * 2;
                    kiln_val *new_elements;
                    size_t fill_i;
                    while (elem_idx >= new_cap) new_cap *= 2;
                    new_elements = kiln_c99_object_elements_resize(object, new_cap);
                    if (new_elements == NULL) return KILN_C99_OUT_OF_MEMORY;
                    for (fill_i = object->element_capacity; fill_i < new_cap; ++fill_i) {
                        new_elements[fill_i] = KILN_VAL_HOLE;
                    }
                    kiln_c99_heap_store_pointer(object, &object->elements, new_elements);
                    object->element_capacity = new_cap;
                }
                kiln_c99_heap_store_val(object, &object->elements[elem_idx], kiln_val_from_boundary(value));
                if (elem_idx >= object->element_count) {
                    object->element_count = elem_idx + 1;
                }
                return KILN_C99_OK;
            }
        }
    }
    {
        /* Not an index, so it is an ordinary key -- `a["length"] = 0` and `a["tag"] = "x"` are
         * the same write `a.length` and `a.tag` are, reached by a different spelling.
         *
         * A target that is not an object goes the same way rather than being turned away here:
         * `put` is the one place that knows which of those is a primitive the language refuses
         * the write on and which is a store this backend has not grown yet. */
        kiln_value text = kiln_c99_property_key(key);
        if (text.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol "
                            "slot");
        }
        return kiln_c99_put_mode_key(host, target, (const kiln_string *)text.ptr,
                                     kiln_c99_property_ikey(key), value, strict, depth, thrown);
    }
}

/* Reflect.set reports whether [[Set]] accepted a write; unlike an assignment it never turns a
 * normal refusal into an exception. The ordinary write path intentionally keeps sloppy refusal
 * silent, so probe the descriptor shape first and only then perform that path. */
static int kiln_c99_write_would_accept_key(kiln_value target, const kiln_string *key,
                                           const kiln_string *canonical) {
    kiln_value current = target;
    size_t steps;
    const kiln_string *stable = canonical;
    if (key == NULL) return 0;
    if (stable == NULL) {
        stable = kiln_c99_intern_key(key);
        if (stable == NULL) return 0;
    }

    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_closure *closure = kiln_c99_closure_of(target);
        if (kiln_c99_same_name_key(key, "prototype")) {
            if (closure->is_class) return 0;
            return kiln_c99_closure_store(closure).tag != KILN_VALUE_OBJECT ||
                   !kiln_c99_object_of(kiln_c99_closure_store(closure))->frozen;
        }
        current = kiln_c99_closure_store(closure);
    } else if (target.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(target);
        if (array->frozen) {
            return 0;
        }
        /* An array's elements and its `length` are the element store; they are not entries in
         * the property table, which an array grows only when a *named* key is written to it. So
         * the table cannot answer for them -- and an array nothing has ever named a property on
         * has no table to ask, which used to make the answer below "no". `Reflect.set(a, "3", 4,
         * a)` then reported false and dropped the element, which is how a vue reactive effect
         * stopped seeing `push`: vue's array instrumentation forwards through the set trap with
         * `Reflect.set`, so the refusal swallowed the write and the dependency never fired. */
        if (kiln_c99_same_name_key(key, "length") || kiln_c99_key_index_key(key) >= 0.0) {
            return 1;
        }
        current = array->properties;
    } else if (target.tag == KILN_VALUE_GENERATOR) {
        current = kiln_c99_generator_of(target)->properties;
    } else if (target.tag == KILN_VALUE_REGEXP) {
        kiln_regexp *regexp = kiln_c99_regexp_of(target);
        if (kiln_c99_same_name_key(key, "lastIndex")) return regexp->last_index_writable;
        current = regexp->properties;
    }

    if (current.tag != KILN_VALUE_OBJECT) {
        /* Having no property table yet is not a refusal: a function and an array both grow one
         * on their first named write. Only a value with nowhere at all to put a property -- a
         * primitive, or an object-like tag this backend has no store for -- answers no. */
        if (target.tag == KILN_VALUE_FUNCTION || target.tag == KILN_VALUE_ARRAY ||
            target.tag == KILN_VALUE_GENERATOR || target.tag == KILN_VALUE_REGEXP) {
            return 1;
        }
        return 0;
    }
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        kiln_object *object = kiln_c99_object_of(current);
        kiln_property property;
        if (steps == 0 && object->module_namespace) {
            return 0;
        }
        if (steps == 0 && object->frozen) {
            return 0;
        }
        if (kiln_c99_property_read_own_value(current, key, stable, &property)) {
            if (property.accessor) {
                return property.setter.tag != KILN_VALUE_UNDEFINED;
            }
            return property.writable;
        }
        if (object->prototype.tag != KILN_VALUE_OBJECT) {
            break;
        }
        current = object->prototype;
    }
    return 1;
}

static int kiln_c99_write_would_accept(kiln_value target, const char *key, size_t key_len) {
    kiln_string view;
    if (key_len > UINT32_MAX) return 0;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    return kiln_c99_write_would_accept_key(target, &view, NULL);
}

static int kiln_c99_reflect_set(kiln_host *host, kiln_value target, kiln_value key,
                                kiln_value value, int *accepted, int depth,
                                kiln_value *thrown) {
    kiln_value text;
    if (target.tag == KILN_VALUE_PROXY) {
        kiln_value trap;
        kiln_value inner = kiln_c99_proxy_of(target)->target;
        kiln_value handler = kiln_c99_proxy_of(target)->handler;
        int status = kiln_c99_proxy_trap(host, target, "set", &trap, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (trap.tag == KILN_VALUE_UNDEFINED) {
            return kiln_c99_reflect_set(host, inner, key, value, accepted, depth, thrown);
        }
        {
            kiln_value arguments[4];
            kiln_value answered;
            arguments[0] = inner;
            arguments[1] = key;
            arguments[2] = value;
            arguments[3] = target;
            status = kiln_c99_call(host, &answered, trap, handler, arguments, 4, depth, thrown);
            if (status == KILN_C99_OK) {
                *accepted = kiln_c99_truthy(answered);
            }
            return status;
        }
    }
    text = kiln_c99_property_key(key);
    if (text.tag != KILN_VALUE_STRING) {
        KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol slot");
    }
    if (target.tag == KILN_VALUE_SHARED) {
        *accepted = 0;
        return KILN_C99_OK;
    }
    if (!kiln_c99_write_would_accept_key(target, (const kiln_string *)text.ptr, NULL)) {
        *accepted = 0;
        return KILN_C99_OK;
    }
    {
        int status = kiln_c99_set_computed_mode(host, target, key, value, 0, depth, thrown);
        *accepted = status == KILN_C99_OK;
        return status;
    }
}

static int kiln_c99_reflect_define_property(kiln_host *host, kiln_value target,
                                            kiln_value key, kiln_value descriptor,
                                            int *accepted, int depth,
                                            kiln_value *thrown) {
    if (target.tag == KILN_VALUE_PROXY) {
        kiln_value trap;
        kiln_value inner = kiln_c99_proxy_of(target)->target;
        kiln_value handler = kiln_c99_proxy_of(target)->handler;
        int status = kiln_c99_proxy_trap(host, target, "defineProperty", &trap, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (trap.tag == KILN_VALUE_UNDEFINED) {
            return kiln_c99_reflect_define_property(host, inner, key, descriptor, accepted, depth,
                                                    thrown);
        }
        {
            kiln_value arguments[3];
            kiln_value answered;
            arguments[0] = inner;
            arguments[1] = key;
            arguments[2] = descriptor;
            status = kiln_c99_call(host, &answered, trap, handler, arguments, 3, depth, thrown);
            if (status == KILN_C99_OK) {
                *accepted = kiln_c99_truthy(answered);
            }
            return status;
        }
    }
    kiln_value text = kiln_c99_property_key(key);
    if (text.tag != KILN_VALUE_STRING) {
        KILN_C99_REFUSE("a computed property key that is neither a string nor a named symbol slot");
    }
    const kiln_string *name_key = (const kiln_string *)text.ptr;
    int status =
        kiln_c99_define_property_key(host, target, name_key, NULL, descriptor, depth, thrown);
    if (status == KILN_C99_OK) {
        *accepted = 1;
        return KILN_C99_OK;
    }
    if (status == KILN_C99_CONTROL_ERROR && thrown != NULL && thrown->tag != KILN_VALUE_UNDEFINED) {
        *thrown = kiln_c99_undefined();
        *accepted = 0;
        return KILN_C99_OK;
    }
    *accepted = 0;
    return status;
}

static int kiln_c99_set_computed(kiln_host *host, kiln_value target, kiln_value key,
                                 kiln_value value, int depth, kiln_value *thrown) {
    return kiln_c99_set_computed_mode(host, target, key, value, 0, depth, thrown);
}

/* One `key: value` of an object literal.
 *
 * A literal *defines* its members; it does not assign them. The difference is invisible on a
 * fresh object for almost every key, and decisive for one: `{ ["__proto__"]: v }` must produce an
 * own property called `__proto__`, where `o.__proto__ = v` relinks the object -- the
 * specification draws the line at whether the key was computed, not at what it evaluated to. Any
 * inherited setter is bypassed for the same reason, which is what "define" means.
 *
 * Only an object literal reaches this, so only an ordinary object is handled; anything else is
 * the caller having built something this cannot be asked about. */
static int kiln_c99_define_computed(kiln_value target, kiln_value key, kiln_value value) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_value text = kiln_c99_property_key(key);
    kiln_object *object;
    kiln_property *slot;
    int status;
    if (text.tag != KILN_VALUE_STRING || target.tag != KILN_VALUE_OBJECT) {
        KILN_C99_REFUSE("a computed definition needs a string key and an ordinary object");
    }
    object = kiln_c99_object_of(target);
    if (object->shape != NULL) {
        const kiln_string *text_key = (const kiln_string *)text.ptr;
        double index_num = kiln_c99_key_index_key(text_key);
        if (index_num >= 0.0 && index_num < 100000.0) {
            size_t elem_idx = (size_t)index_num;
            if (elem_idx >= object->element_capacity) {
                size_t new_cap = object->element_capacity == 0 ? 8 : object->element_capacity * 2;
                kiln_val *new_elements;
                size_t fill_i;
                while (elem_idx >= new_cap) new_cap *= 2;
                new_elements = kiln_c99_object_elements_resize(object, new_cap);
                if (new_elements == NULL) return KILN_C99_OUT_OF_MEMORY;
                for (fill_i = object->element_capacity; fill_i < new_cap; ++fill_i) {
                    new_elements[fill_i] = KILN_VAL_HOLE;
                }
                kiln_c99_heap_store_pointer(object, &object->elements, new_elements);
                object->element_capacity = new_cap;
            }
            kiln_c99_heap_store_val(object, &object->elements[elem_idx], kiln_val_from_boundary(value));
            if (elem_idx >= object->element_count) {
                object->element_count = elem_idx + 1;
            }
            return KILN_C99_OK;
        } else {
            const kiln_string *ikey = key.tag == KILN_VALUE_SYMBOL
                                          ? kiln_c99_property_ikey(key)
                                          : kiln_c99_intern_key((const kiln_string *)text.ptr);
            uint32_t slot_idx;
            kiln_shape *next_shape;
            if (ikey == NULL) return KILN_C99_OUT_OF_MEMORY;
            if (kiln_c99_shape_find_slot(object->shape, ikey, &slot_idx)) {
                /* Redefining an accessor or an attribute-bearing property is the list's job. */
                if (object->shape->special_mask & (1u << slot_idx)) goto fallback_dict;
                if (slot_idx < object->slot_capacity) {
                    if ((object->shape->double_slots_mask & (1u << slot_idx)) &&
                        value.tag != KILN_VALUE_NUMBER) {
                        kiln_c99_ensure_dictionary(object);
                        goto fallback_dict;
                    }
                    kiln_c99_heap_store_val(object, &object->slots[slot_idx], kiln_val_from_boundary(value));
                    return KILN_C99_OK;
                }
            }
            int is_double = (value.tag == KILN_VALUE_NUMBER);
            if (kiln_c99_shape_transition(object->shape, ikey, is_double, &next_shape)) {
                uint32_t new_slot = next_shape->slot_index;
                if (new_slot >= object->slot_capacity) {
                    size_t new_cap = object->slot_capacity == 0 ? KILN_C99_INLINE_SLOTS : object->slot_capacity * 2;
                    kiln_val *new_slots;
                    size_t fill_i;
                    while (new_slot >= new_cap) new_cap *= 2;
                    new_slots = kiln_c99_object_slots_resize(object, new_cap);
                    if (new_slots == NULL) return KILN_C99_OUT_OF_MEMORY;
                    for (fill_i = object->slot_capacity; fill_i < new_cap; ++fill_i) {
                        new_slots[fill_i] = KILN_VAL_UNDEFINED;
                    }
                    kiln_c99_heap_store_pointer(object, &object->slots, new_slots);
                    object->slot_capacity = new_cap;
                }
                kiln_c99_heap_store_val(object, &object->slots[new_slot], kiln_val_from_boundary(value));
                object->shape = next_shape;
                return KILN_C99_OK;
            }
        }
    fallback_dict:
        kiln_c99_ensure_dictionary(object);
    }
    status = kiln_c99_own_slot_key(target, (const kiln_string *)text.ptr, NULL, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* `own_slot` may have handed back an existing property -- a duplicate key in the literal,
     * `{ get x() {}, x: 1 }` among them -- so this is not always the fresh, already-zero slot it
     * would be for a first-seen key. */
    if (slot->accessor) {
        kiln_c99_object_of(target)->accessor_count -= 1;
    }
    slot->accessor = 0;
    slot->refused = 0;
    kiln_c99_heap_store_value(kiln_c99_object_of(target), &slot->value, value);
    slot->writable = 1;
    slot->enumerable = key.tag != KILN_VALUE_SYMBOL;
    slot->configurable = 1;
    return KILN_C99_OK;
}

/* Publish a name this realm cannot answer, so that reading it refuses instead of reading
 * `undefined`. See `kiln_property::refused`. */
static int kiln_c99_put_refused(kiln_value target, const char *key, size_t key_len) {
    kiln_property *slot;
    int status = kiln_c99_own_slot(target, key, key_len, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (slot->accessor) {
        kiln_c99_object_of(target)->accessor_count -= 1;
    }
    slot->accessor = 0;
    slot->refused = 1;
    /* Not enumerated. `Object.getOwnPropertyNames` still reports the name -- the name is really
     * there -- but a spread or a `JSON.stringify` reads every enumerable value, and refusing the
     * whole of those for one member nobody asked for would trade a wrong answer for a worse
     * one. Asking for the member by name is the question that gets the honest refusal. */
    slot->enumerable = 0;
    slot->writable = 0;
    slot->configurable = 0;
    slot->value = kiln_c99_undefined();
    return KILN_C99_OK;
}

static int kiln_c99_array_push(kiln_value target, kiln_value value) {
    kiln_array *array = kiln_c99_array_of(target);
    int status = kiln_c99_array_reserve(array, array->count + 1);
    if (status != KILN_C99_OK) {
        return status;
    }
    kiln_val v = kiln_val_from_boundary(value);
    if (array->is_numeric && !KILN_VAL_IS_NUMBER(v)) {
        array->is_numeric = 0;
    }
    kiln_c99_heap_store_val(array, &array->elements[array->count], v);
    array->count += 1;
    return KILN_C99_OK;
}

static int kiln_c99_array_push_val(kiln_value target, kiln_val value) {
    kiln_array *array = kiln_c99_array_of(target);
    int status = kiln_c99_array_reserve(array, array->count + 1);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (array->is_numeric && !KILN_VAL_IS_NUMBER(value)) {
        array->is_numeric = 0;
    }
    kiln_c99_heap_store_val(array, &array->elements[array->count], value);
    array->count += 1;
    return KILN_C99_OK;
}

/* What a console shows, which is not what `String(x)` gives: `String({})` is "[object Object]"
 * and a console prints the contents.  The interpreter does this on the host side; the native
 * path has to do it inside the generated unit, because a reference never crosses the embedding
 * boundary -- so this is the one place the two engines have to agree character for character.
 *
 * Node's format, which the interpreter already matches: quoted strings only when nested, keys
 * quoted unless they are identifiers, and a placeholder past two levels of nesting. */
#define KILN_C99_INSPECT_DEPTH 2
