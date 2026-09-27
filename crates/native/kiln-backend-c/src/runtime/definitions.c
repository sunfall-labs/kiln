
/* Issue 04 step 3b. A method, a private method or an accessor defined on a shaped object keeps
 * it shaped, with the attributes the list-mode path below would have given the property recorded
 * in its shape (`KILN_C99_ATTR_*`), so a class prototype is shaped and a method cache names it by
 * shape. Returns 1 when handled (`*status` is the answer), 0 to leave the definition to the
 * list-mode path exactly as before. `kind` is 0 for a method (non-enumerable data), 3 for a
 * private method (also read-only); accessors are `kiln_c99_define_accessor_shaped`'s.
 *
 * Only what the list path would do without asking anyone is done here: a new key on an ordinary,
 * extensible, unfrozen object whose prototype chain would not intercept the assignment the list
 * path starts with, or an existing slot that already carries exactly these attributes. */
static int kiln_c99_define_shaped(kiln_value target, const kiln_string *key, kiln_val held,
                                  int kind, int *status) {
    kiln_object *object = kiln_c99_object_of(target);
    const kiln_string *ikey;
    uint32_t attrs;
    uint32_t slot;
    if (object->shape == NULL || object->properties != NULL || object->frozen ||
        object->module_namespace || object->not_extensible ||
        kiln_c99_is_node_hidden_key_key(key) || kiln_c99_same_name_key(key, "__proto__") ||
        kiln_c99_key_index_key(key) >= 0.0) {
        return 0;
    }
    ikey = kiln_c99_intern_key(key);
    if (ikey == NULL) return 0;
    /* The list path's record: `own_slot` makes it enumerable unless the key is reserved, then
     * the method definition clears `enumerable` (and a private one clears `writable`). */
    attrs = kiln_c99_key_is_reserved(ikey) ? 0u : KILN_C99_ATTR_ENUM_FLIP;
    if (kind == 3) attrs |= KILN_C99_ATTR_READONLY;
    if (kiln_c99_shape_find_slot(object->shape, ikey, &slot)) {
        /* Only a slot the list path would leave with these attributes: its assignment writes a
         * writable data property in place and the definition changes nothing else. */
        if (slot >= object->slot_capacity || object->shape->attrs[slot] != attrs ||
            (attrs & KILN_C99_ATTR_READONLY) != 0 ||
            ((object->shape->double_slots_mask & (1u << slot)) && !KILN_VAL_IS_NUMBER(held))) {
            return 0;
        }
        /* A slot whose code the shape records keeps the record only for the same code. */
        if ((object->shape->code_mask & (1u << slot)) != 0) {
            if (!kiln_c99_shape_store_recorded(object, slot, held)) return 0;
            *status = KILN_C99_OK;
            return 1;
        }
        kiln_c99_prototype_touched(object);
        object->slots[slot] = held;
        *status = KILN_C99_OK;
        return 1;
    }
    /* A setter or a read-only property up the chain takes the list path's assignment. */
    if (object->prototype.tag == KILN_VALUE_OBJECT &&
        kiln_c99_has_inherited_setter_or_readonly(object->prototype) &&
        kiln_c99_inherited_write_blocked(object->prototype, ikey, key)) {
        return 0;
    }
    /* Issue 04 step 3c: a method's code is recorded in the shape its definition takes -- not a
     * private method's (read and called by brand, never through a method cache) and not a
     * prototype's `constructor`, which every class prototype defines first: a record there
     * would give the root one branch per class for nothing a method call site reads. */
    return kiln_c99_shape_define_new(
        object, ikey, held, attrs,
        kind == 0 && !kiln_c99_same_name_key(ikey, "constructor") ? kiln_c99_val_code(held) : NULL,
        status);
}

/* The accessor half of the above: `get`/`set` defined on a shaped object keeps it shaped, the pair
 * of functions in one slot (`kiln_c99_accessor_pair_new`). A new key becomes an accessor whose
 * attributes are the list path's (`writable` off, `enumerable` as given); a second half merges
 * into an existing accessor slot in place when that leaves its attributes as they are. */
static int kiln_c99_define_accessor_shaped(kiln_value target, const kiln_string *key,
                                           kiln_value getter, kiln_value setter,
                                           int enumerable, int *status) {
    kiln_object *object = kiln_c99_object_of(target);
    const kiln_string *ikey;
    uint32_t attrs;
    uint32_t slot;
    kiln_val pair;
    if (object->shape == NULL || object->properties != NULL ||
        kiln_c99_is_node_hidden_key_key(key) || kiln_c99_key_index_key(key) >= 0.0) {
        return 0;
    }
    ikey = kiln_c99_intern_key(key);
    if (ikey == NULL) return 0;
    if (kiln_c99_shape_find_slot(object->shape, ikey, &slot)) {
        kiln_value boxed;
        kiln_array *halves;
        uint32_t had = object->shape->attrs[slot];
        int was_enumerable = kiln_c99_shape_slot_enumerable(object->shape, slot);
        /* The merge only ever clears `enumerable`; one that would is a change of attributes. */
        if ((had & KILN_C99_ATTR_ACCESSOR) == 0 || slot >= object->slot_capacity ||
            (was_enumerable && !enumerable)) {
            return 0;
        }
        boxed = kiln_val_to_boundary(object->slots[slot]);
        if (boxed.tag != KILN_VALUE_ARRAY) return 0;
        halves = kiln_c99_array_of(boxed);
        if (halves->count != 2) return 0;
        kiln_c99_prototype_touched(object);
        if (getter.tag != KILN_VALUE_UNDEFINED) halves->elements[0] = kiln_val_from_boundary(getter);
        if (setter.tag != KILN_VALUE_UNDEFINED) halves->elements[1] = kiln_val_from_boundary(setter);
        *status = KILN_C99_OK;
        return 1;
    }
    attrs = KILN_C99_ATTR_ACCESSOR | KILN_C99_ATTR_READONLY;
    if ((enumerable != 0) != !kiln_c99_key_is_reserved(ikey)) attrs |= KILN_C99_ATTR_ENUM_FLIP;
    *status = kiln_c99_accessor_pair_new(getter, setter, &pair);
    if (*status != KILN_C99_OK) return 1;
    return kiln_c99_shape_define_new(object, ikey, pair, attrs, NULL, status);
}

/* Define an accessor half, merging with a half already defined under the same key.
 *
 * Merging matters because `get x()` and `set x()` are written separately and describe one
 * property; defining the second must not discard the first.
 *
 * `enumerable` separates the two syntaxes that reach here: an object literal's accessor is an
 * ordinary property and is enumerated, a class body's is not. */
static int kiln_c99_define_accessor_key(kiln_value target, const kiln_string *key,
                                        kiln_value getter, kiln_value setter,
                                        int enumerable) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_object *object;
    kiln_property *property;
    size_t index;
    int status;
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_value store;
        status = kiln_c99_function_store(target, &store);
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_define_accessor_key(store, key, getter, setter, enumerable);
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        KILN_C99_REFUSE("DefineAccessor target has no property store");
    }
    object = kiln_c99_object_of(target);
    if (object->shape != NULL) {
        if (kiln_c99_define_accessor_shaped(target, key, getter, setter, enumerable, &status))
            return status;
        kiln_c99_ensure_dictionary(object);
        object->shape = NULL;
    }
    for (index = 0; index < object->count; ++index) {
        if (!kiln_c99_same_key_view(&object->properties[index], key)) {
            continue;
        }
        if (!object->properties[index].accessor) {
            object->properties[index].accessor = 1;
            object->accessor_count += 1;
            object->properties[index].getter = kiln_c99_undefined();
            object->properties[index].setter = kiln_c99_undefined();
        }
        if (getter.tag != KILN_VALUE_UNDEFINED) {
            object->properties[index].getter = getter;
        }
        if (setter.tag != KILN_VALUE_UNDEFINED) {
            object->properties[index].setter = setter;
        }
        if (!enumerable) {
            object->properties[index].enumerable = 0;
        }
        return KILN_C99_OK;
    }
    /* A definition creates an own slot directly. Going through assignment would invoke an
     * inherited setter (or silently refuse a getter-only property), after which no own property
     * exists to turn into the accessor requested here. */
    status = kiln_c99_own_slot_key(target, key, NULL, &property);
    if (status != KILN_C99_OK) {
        if (status == KILN_C99_CONTROL_ERROR && kiln_c99_diagnostic_detail == NULL)
            kiln_c99_diagnostic_detail = "DefineAccessor could not create the property";
        return status;
    }
    property->accessor = 1;
    object->accessor_count += 1;
    property->getter = getter;
    property->setter = setter;
    property->writable = 0;
    property->enumerable = enumerable;
    property->configurable = 1;
    return KILN_C99_OK;
}

/* Fixed C names use a borrowed Latin-1 view; the headed implementation above owns the merge. */
static int kiln_c99_define_accessor(kiln_value target, const char *key, size_t key_len,
                                    kiln_value getter, kiln_value setter, int enumerable) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    return kiln_c99_define_accessor_key(target, &view, getter, setter, enumerable);
}

/* The store a *definition* lands in.
 *
 * A function keeps its statics in one, an array and a collection keep their non-element
 * properties in one, and an object is its own. Distinct from the read paths, which each know
 * where to look: this is the one place that has to create a store that does not exist yet. */
static int kiln_c99_definition_store(kiln_value target, kiln_value *out) {
    if (target.tag == KILN_VALUE_OBJECT) {
        *out = target;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_FUNCTION) {
        return kiln_c99_function_store(target, out);
    }
    /* A built-in and a method value keep their own properties in the callable store that every
     * read of them consults first (`read.c`, the top of the built-in case), so a definition on
     * one lands where the read will look. Assignment already went there; `defineProperty` refused,
     * which is what a library installing a symbol hook on `Map.prototype` hit, and what the
     * runtime bootstrap hits defining `Error.prototype.name` non-enumerable. */
    if (target.tag == KILN_VALUE_BUILTIN || target.tag == KILN_VALUE_METHOD ||
        target.tag == KILN_VALUE_BOUND) {
        return kiln_c99_callable_store(target, out, 1);
    }
    {
        kiln_value *slot = kiln_c99_exotic_store(target);
        if (slot != NULL) {
            if (slot->tag != KILN_VALUE_OBJECT) {
                int status = kiln_c99_new_object(slot);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *out = *slot;
            return KILN_C99_OK;
        }
    }
    KILN_C99_REFUSE("a definition target with no property store");
}

/* Whether `descriptor` mentions `name` at all, which is a different question from what it says.
 *
 * `{ value: undefined }` and `{}` define different properties, and `{ writable: false }` and a
 * descriptor silent about writability say different things about an existing one -- so every
 * field has to be tested for presence before it is read. */
static int kiln_c99_descriptor_field(kiln_host *host, kiln_value descriptor, const char *name,
                                     kiln_value *out, int *present, int depth,
                                     kiln_value *thrown) {
    size_t len = strlen(name);
    int status = kiln_c99_has_property(host, descriptor, name, len, present, depth, thrown);
    *out = kiln_c99_undefined();
    if (status != KILN_C99_OK || !*present) {
        return status;
    }
    return kiln_c99_get(host, descriptor, name, len, out, depth, thrown);
}

/* `Object.defineProperty(target, key, descriptor)`.
 *
 * Defining is not assigning, and the difference is the whole reason the operation exists. An
 * assignment consults the prototype chain and hands the write to the first setter it finds; a
 * definition ignores the chain entirely and installs an own property. An assignment produces a
 * writable, enumerable, configurable data property; a definition leaves *unmentioned* attributes
 * false on a property it creates. `Object.defineProperty(o, "k", { value: 1 })` and `o.k = 1`
 * differ in all four ways, and the lazy-getter idiom -- a prototype accessor that replaces
 * itself with an own value the first time it is read -- depends on every one of them. */
static int kiln_c99_define_property_key(kiln_host *host, kiln_value target,
                                        const kiln_string *view,
                                        const kiln_string *canonical, kiln_value descriptor,
                                        int depth, kiln_value *thrown) {
    const kiln_string *stable = canonical;
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_value store;
    kiln_property *slot;
    kiln_value getter;
    kiln_value setter;
    kiln_value value;
    kiln_value writable;
    kiln_value enumerable;
    kiln_value configurable;
    int has_getter = 0;
    int has_setter = 0;
    int has_value = 0;
    int has_writable = 0;
    int has_enumerable = 0;
    int has_configurable = 0;
    int existed = 0;
    int status;
    size_t at;
    /* Protocol slots are owned by their native adapters, never by JavaScript definitions. */
    if (kiln_c99_is_node_hidden_key_key(view)) {
        return KILN_C99_OK;
    }
    if (descriptor.tag != KILN_VALUE_OBJECT) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Property description must be an object");
    }
    if (target.tag == KILN_VALUE_PROXY) {
        kiln_value key_value = kiln_c99_key_value(view, &stable);
        if (key_value.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        return kiln_c99_proxy_define(host, target, key_value, descriptor, depth, thrown);
    }
    if (target.tag != KILN_VALUE_OBJECT && target.tag != KILN_VALUE_FUNCTION &&
        target.tag != KILN_VALUE_BUILTIN && target.tag != KILN_VALUE_METHOD &&
        target.tag != KILN_VALUE_BOUND && kiln_c99_exotic_store(target) == NULL) {
        /* References with an own-property store are valid targets. Their intrinsic names still
         * need specialised descriptor handling, but an unrelated key must land in the same store
         * ordinary reads already consult. This is what permits libraries to install a symbol
         * hook on a constructor with `Object.defineProperty`. */
        KILN_C99_REFUSE("a property definition on a value with no property store");
    }
    /* Every field is read before anything is written, because a descriptor may be a program's
     * object with getters on it: one of them running mid-update could reallocate the very
     * property list the slot below points into. */
    status = kiln_c99_descriptor_field(host, descriptor, "get", &getter, &has_getter, depth,
                                       thrown);
    if (status == KILN_C99_OK) {
        status = kiln_c99_descriptor_field(host, descriptor, "set", &setter, &has_setter, depth,
                                           thrown);
    }
    if (status == KILN_C99_OK && (has_getter || has_setter) &&
        (target.tag == KILN_VALUE_BUILTIN || target.tag == KILN_VALUE_METHOD ||
         target.tag == KILN_VALUE_BOUND)) {
        /* A data property in a built-in's store is read wherever the store is consulted, which
         * is what the runtime bootstrap and a library's symbol hook need. An accessor there would
         * not run: an instance answers the built-in's own names -- `promise.constructor`,
         * `re.flags` -- ahead of the prototype's store, so a getter defined on
         * `Promise.prototype` is silently never called, which is worse than stopping here. */
        KILN_C99_REFUSE("a property definition on a value with no property store for an "
                        "accessor: an instance read would not run it");
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_descriptor_field(host, descriptor, "value", &value, &has_value, depth,
                                           thrown);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_descriptor_field(host, descriptor, "writable", &writable, &has_writable,
                                           depth, thrown);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_descriptor_field(host, descriptor, "enumerable", &enumerable,
                                           &has_enumerable, depth, thrown);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_descriptor_field(host, descriptor, "configurable", &configurable,
                                           &has_configurable, depth, thrown);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        /* An array index is an element, not a name. The store below holds `a.total = 1`; putting
         * a definition of `"0"` there recorded a property nothing reads, so
         * `Object.defineProperty(a, "0", { value: 10 })` returned the array unchanged and
         * `a[0]` still answered what it had -- a silent wrong answer, and the shape
         * `arguments` reaches through every ES5 test that redefines a parameter slot.
         *
         * Only the definition an element can actually carry is accepted. There is no per-element
         * attribute table here, so a definition that would make one non-writable, hidden or
         * non-configurable, or turn it into an accessor, is refused by name rather than written
         * as an ordinary element that lies about its own flags. */
        double at = kiln_c99_key_index_key(view);
        if (at >= 0.0) {
            kiln_array *array = kiln_c99_array_of(target);
            size_t slot = (size_t)at;
            size_t fill;
            if (has_getter || has_setter || !has_value || array->frozen ||
                (has_writable && !kiln_c99_truthy(writable)) ||
                (has_enumerable && !kiln_c99_truthy(enumerable)) ||
                (has_configurable && !kiln_c99_truthy(configurable))) {
                KILN_C99_REFUSE("an array index definition that is not a plain writable value");
            }
            status = kiln_c99_array_reserve(array, slot + 1);
            if (status != KILN_C99_OK) {
                return status;
            }
            /* Growing leaves holes, which read as undefined and are not the same thing. */
            for (fill = array->count; fill < slot; ++fill) {
                array->elements[fill] = KILN_VAL_HOLE;
            }
            array->elements[slot] = kiln_val_from_boundary(value);
            if (slot + 1 > array->count) {
                array->count = slot + 1;
            }
            return KILN_C99_OK;
        }
    }
    status = kiln_c99_definition_store(target, &store);
    if (status != KILN_C99_OK) {
        return status;
    }
    {
        kiln_object *object = kiln_c99_object_of(store);
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
            object->shape = NULL;
        }
        for (at = 0; at < object->count; ++at) {
            if (kiln_c99_same_key_view(&object->properties[at], view)) {
                existed = 1;
                break;
            }
        }
        if (object->module_namespace) {
            /* A namespace accepts only a descriptor that makes no semantic change. In
             * particular, export descriptors report writable=true even though [[Set]] rejects;
             * turning one non-writable would make this object look like an ordinary frozen one. */
            if (!existed) {
                return kiln_c99_throw_module_namespace_define_key(thrown, view);
            }
            {
                kiln_property *fixed = &object->properties[at];
                int refused = (has_getter || has_setter) ||
                              (has_value && !kiln_c99_strict_equal(value, fixed->value)) ||
                              (has_writable &&
                               (kiln_c99_truthy(writable) != 0) != (fixed->writable != 0)) ||
                              (has_enumerable &&
                               (kiln_c99_truthy(enumerable) != 0) !=
                                   (fixed->enumerable != 0)) ||
                              (has_configurable && kiln_c99_truthy(configurable));
                if (refused) {
                    return kiln_c99_throw_module_namespace_define_key(thrown, view);
                }
            }
            return KILN_C99_OK;
        }
        /* What a definition is *not allowed* to do, checked before anything is written.
         *
         * A configurable property can be redefined into anything; a non-configurable one is a
         * promise the object has already made, and the language keeps it by throwing rather than
         * by quietly ignoring the request. Accepting silently was the worse half of both worlds:
         * a frozen object read back as though the write had taken, so `Object.freeze` looked as
         * though a later `defineProperty` had undone it. The one redefinition a non-configurable
         * property does allow is the one that changes nothing, which is why each attribute is
         * compared rather than the descriptor being rejected on sight.
         *
         * `frozen` is the object-wide form of the same promise -- freezing is recorded on the
         * object here rather than by rewriting every slot -- so it fails for the same reason. */
        if (existed && (object->frozen || !object->properties[at].configurable)) {
            kiln_property *fixed = &object->properties[at];
            int accessor = (has_getter || has_setter) ? 1 : 0;
            int settled = object->frozen || !fixed->writable;
            int refused = (has_configurable && kiln_c99_truthy(configurable)) ||
                          (has_enumerable &&
                           (kiln_c99_truthy(enumerable) != 0) != (fixed->enumerable != 0));
            if (!refused && fixed->accessor) {
                refused = !accessor ||
                          (has_getter && !kiln_c99_strict_equal(getter, fixed->getter)) ||
                          (has_setter && !kiln_c99_strict_equal(setter, fixed->setter));
            } else if (!refused) {
                /* A writable data property may still take a new value and be made non-writable;
                 * only once it is non-writable is its value settled too. */
                refused = accessor || (settled && has_writable && kiln_c99_truthy(writable)) ||
                          (settled && has_value && !kiln_c99_strict_equal(value, fixed->value));
            }
            if (refused) {
                return kiln_c99_throw_named_key_text(thrown, "Cannot redefine property: ", view, "");
            }
        }
        if (!existed && object->frozen) {
            /* A frozen object is not extensible either, so a name it never had cannot be added.
             * Same promise, the other half of it. */
            return kiln_c99_throw_named_key_text(thrown, "Cannot define property ", view, "");
        }
    }
    status = kiln_c99_own_slot_key(store, view, stable, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (!existed) {
        /* What a descriptor does not say about a *new* property is false, where the fresh slot
         * arrives as what an assignment would have made. */
        slot->enumerable = 0;
        slot->writable = 0;
        slot->configurable = 0;
    }
    if (has_getter || has_setter) {
        if (!slot->accessor) {
            slot->accessor = 1;
            kiln_c99_object_of(store)->accessor_count += 1;
            slot->getter = kiln_c99_undefined();
            slot->setter = kiln_c99_undefined();
            slot->value = kiln_c99_undefined();
        }
        if (has_getter) {
            slot->getter = getter;
        }
        if (has_setter) {
            slot->setter = setter;
        }
    } else if (has_value) {
        if (slot->accessor) {
            kiln_c99_object_of(store)->accessor_count -= 1;
        }
        slot->accessor = 0;
        /* A value defined over a published-and-unanswerable name is a program supplying the
         * member this realm could not. It is now a data property like any other. */
        slot->refused = 0;
        slot->getter = kiln_c99_undefined();
        slot->setter = kiln_c99_undefined();
        slot->value = value;
    }
    if (has_writable) {
        slot->writable = kiln_c99_truthy(writable);
    }
    if (has_enumerable) {
        kiln_value symbol_key;
        if (kiln_c99_symbol_of_key_view(slot->key, &symbol_key)) {
            /* A symbol key's `enumerable` is not the program's to set: holding it at 0 is what
             * keeps the `@@` spelling out of `Object.keys` and `JSON.stringify`. The answer the
             * program gave is kept beside it instead. */
            slot->symbol_hidden = !kiln_c99_truthy(enumerable);
            slot->enumerable = 0;
        } else {
            slot->enumerable = kiln_c99_truthy(enumerable);
        }
    }
    if (has_configurable) {
        slot->configurable = kiln_c99_truthy(configurable);
    }
    if (!slot->accessor && !slot->writable) {
        kiln_c99_object_of(store)->read_only_seen = 1;
    }
    return KILN_C99_OK;
}


/* Fixed C names use a borrowed Latin-1 view; the headed implementation above owns all
 * semantic branches and interns only when a proxy or property slot retains the key. */
static int kiln_c99_define_property(kiln_host *host, kiln_value target, const char *key,
                                    size_t key_len, kiln_value descriptor, int depth,
                                    kiln_value *thrown) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    return kiln_c99_define_property_key(host, target, &view, NULL, descriptor, depth, thrown);
}

/* Every own property of `descriptors`, defined onto `target`. The shared half of
 * `Object.defineProperties` and the two-argument `Object.create`. */
static int kiln_c99_define_properties(kiln_host *host, kiln_value target, kiln_value descriptors,
                                      int depth, kiln_value *thrown) {
    kiln_object *source;
    size_t index;
    if (descriptors.tag == KILN_VALUE_UNDEFINED) {
        return KILN_C99_OK;
    }
    if (descriptors.tag != KILN_VALUE_OBJECT) {
        KILN_C99_REFUSE("Object.defineProperties given descriptors that are not an object");
    }
    source = kiln_c99_object_of(descriptors);
    if (source->shape != NULL) {
        kiln_c99_ensure_dictionary(source);
    }
    for (index = 0; index < source->count; ++index) {
        /* Only the enumerable own ones, which is what the language walks -- and the machine's
         * own `@@` keys are never among them. */
        kiln_value descriptor;
        int status;
        if (!source->properties[index].enumerable) {
            continue;
        }
        status = kiln_c99_get_key(host, descriptors, source->properties[index].key,
                                  source->properties[index].key, &descriptor, depth, thrown);
        if (status == KILN_C99_OK) {
            status = kiln_c99_define_property_key(host, target, source->properties[index].key,
                                                  source->properties[index].key, descriptor, depth, thrown);
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        /* A descriptor's getters may have grown the list; re-reading its length each time would
         * be wrong for the opposite reason, so the snapshot the loop started with stands and a
         * property added during it is not visited. */
        source = kiln_c99_object_of(descriptors);
    }
    return KILN_C99_OK;
}

/* `Object.getOwnPropertyDescriptor`: the property as a plain object, or undefined if the name is
 * not an own property.
 *
 * The field order is the order a console prints, so it is the specification's: value and
 * writable for a data property, get and set for an accessor, then enumerable and configurable
 * for both. */
static int kiln_c99_describe_property_key(kiln_host *host, kiln_value target,
                                          const kiln_string *view,
                                          const kiln_string *canonical, kiln_value *out,
                                          int *found, int depth, kiln_value *thrown) {
    const kiln_string *stable = canonical;
    kiln_value built;
    int status;
    kiln_value store = target;
    *found = 0;
    *out = kiln_c99_undefined();
    if (kiln_c99_is_node_hidden_key_key(view)) {
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_PROXY) {
        kiln_value key_value = kiln_c99_key_value(view, &stable);
        if (key_value.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        return kiln_c99_proxy_describe(host, target, key_value, out, found, depth, thrown);
    }
    if (target.tag == KILN_VALUE_BUILTIN) {
        if (target.bits == KILN_C99_BUILTIN_ARRAY_PROTOTYPE &&
            kiln_c99_same_name_key(view, "@@unscopables")) {
            kiln_value val;
            status = kiln_c99_get_array_unscopables(&val);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_new_object(&built);
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "value", 5, val, 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "writable", 8, kiln_c99_boolean(0), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "enumerable", 10, kiln_c99_boolean(0), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "configurable", 12, kiln_c99_boolean(1), 0, NULL);
            }
            *found = 1;
            *out = built;
            return status;
        }
        if (target.bits == KILN_C99_BUILTIN_ARRAY_PROTOTYPE &&
            kiln_c99_same_name_key(view, "length")) {
            status = kiln_c99_new_object(&built);
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "value", 5, kiln_c99_number(0.0), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "writable", 8, kiln_c99_boolean(1), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "enumerable", 10, kiln_c99_boolean(0), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "configurable", 12, kiln_c99_boolean(0), 0, NULL);
            }
            *found = 1;
            *out = built;
            return status;
        }
        char intrinsic_buffer[256];
        const char *intrinsic_key;
        size_t intrinsic_len;
        if (kiln_c99_intrinsic_ascii_view(view, intrinsic_buffer, &intrinsic_key,
                                          &intrinsic_len)) {
            int owned = 0;
            const kiln_builtin_member *member =
                kiln_c99_builtin_member(target.bits, intrinsic_key, intrinsic_len, &owned);
            if (member != NULL || kiln_c99_namespace_method(target.bits, intrinsic_key, intrinsic_len) != 0) {
                /* A table-driven or namespace member is an own property: writable and
                 * configurable for methods and builtins, non-writable for text and numbers,
                 * never enumerated. */
                kiln_value val;
                int writable = 1;
                if (member != NULL && (member->kind == KILN_C99_MEMBER_TEXT ||
                                       member->kind == KILN_C99_MEMBER_NUMBER ||
                                       member->kind == KILN_C99_MEMBER_SYMBOL)) {
                    writable = 0;
                }
                *found = 1;
                status = kiln_c99_get_key(host, target, view, stable, &val, depth, thrown);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_new_object(&built);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, built, "value", 5, val, 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, built, "writable", 8, kiln_c99_boolean(writable), 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, built, "enumerable", 10, kiln_c99_boolean(0), 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, built, "configurable", 12, kiln_c99_boolean(1), 0, NULL);
                }
                *out = built;
                return status;
            }
        }
    }
    if (target.tag == KILN_VALUE_BUILTIN &&
        target.bits == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE &&
        kiln_c99_same_name_key(view, "@@toStringTag")) {
        /* `%TypedArray%.prototype[@@toStringTag]` is an accessor on the shared prototype, not a
         * string stored on each concrete view. Framework serializers take this getter and call
         * it against arbitrary typed arrays to identify their element kind. */
        status = kiln_c99_new_object(&built);
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(NULL, built, "get", 3,
                                  kiln_c99_method(KILN_C99_METHOD_TYPED_TO_STRING_TAG), 0, NULL);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(NULL, built, "set", 3, kiln_c99_undefined(), 0, NULL);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(NULL, built, "enumerable", 10, kiln_c99_boolean(0), 0, NULL);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(NULL, built, "configurable", 12, kiln_c99_boolean(1), 0, NULL);
        }
        *found = 1;
        *out = built;
        return status;
    }
    if (target.tag == KILN_VALUE_FUNCTION) {
        /* A function's own properties are `name`, `length`, `prototype`, and whatever a program
         * assigned beside them. The first three are read from the closure rather than from a
         * slot list, so they are described from the closure too -- all three non-enumerable,
         * which is why `Object.keys(f)` is empty for a function nobody has decorated.
         *
         * `prototype` is the one still refused: whether it is writable and configurable depends
         * on how the function was written -- a plain function's is writable, a class's is not,
         * and a method has none at all -- and this backend does not record which. Undefined is a
         * real answer here, so guessing would be a wrong answer indistinguishable from a right
         * one. */
        kiln_closure *closure = kiln_c99_closure_of(target);
        int is_name = kiln_c99_same_name_key(view, "name");
        if (is_name || kiln_c99_same_name_key(view, "length")) {
            kiln_value value = is_name
                                   ? (closure->name == NULL
                                          ? kiln_c99_from_units(NULL, 0)
                                          : kiln_c99_value(KILN_VALUE_STRING, 0,
                                                           (const char *)closure->name,
                                                           closure->name->unit_len))
                                   : kiln_c99_number((double)closure->arity);
            status = kiln_c99_new_object(&built);
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "value", 5, value, 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "writable", 8, kiln_c99_boolean(0), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "enumerable", 10, kiln_c99_boolean(0), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "configurable", 12, kiln_c99_boolean(1), 0,
                                      NULL);
            }
            *found = 1;
            *out = built;
            return status;
        }
        if (kiln_c99_same_name_key(view, "prototype")) {
            KILN_C99_REFUSE("the prototype property of a callable has no descriptor to answer "
                            "with");
        }
        if (closure->properties.tag != KILN_VALUE_OBJECT) {
            return KILN_C99_OK;
        }
        store = closure->properties;
        target = closure->properties;
    }
    if (target.tag == KILN_VALUE_BUILTIN) {
        if (!kiln_c99_is_namespace(target.bits) &&
            (kiln_c99_same_name_key(view, "name") ||
             kiln_c99_same_name_key(view, "length"))) {
            const char *bname = kiln_c99_builtin_name(target.bits);
            kiln_value value = kiln_c99_same_name_key(view, "name")
                                   ? kiln_c99_copy_text(bname, strlen(bname))
                                   : kiln_c99_number(1.0);
            status = kiln_c99_new_object(&built);
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "value", 5, value, 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "writable", 8, kiln_c99_boolean(0), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "enumerable", 10, kiln_c99_boolean(0), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "configurable", 12, kiln_c99_boolean(1), 0,
                                      NULL);
            }
            *found = 1;
            *out = built;
            return status;
        }
        {
            char intrinsic_buffer[256];
            const char *intrinsic_key;
            size_t intrinsic_len;
            if (kiln_c99_intrinsic_ascii_view(view, intrinsic_buffer, &intrinsic_key,
                                              &intrinsic_len) &&
                kiln_c99_backlog_names(kiln_c99_builtin_owner_name(target.bits), intrinsic_key,
                                       intrinsic_len)) {
                KILN_C99_REFUSE("a member the language puts on this built-in and this backend has not "
                                "built");
            }
        }
        return KILN_C99_OK;
    }
    if (target.tag != KILN_VALUE_OBJECT && target.tag != KILN_VALUE_ARRAY) {
        KILN_C99_REFUSE("an own-property descriptor of a value whose own properties are not all "
                        "modelled");
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(target);
        double at = kiln_c99_key_index_key(view);
        if (kiln_c99_same_name_key(view, "length")) {
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            /* `length` is real, writable and neither enumerated nor configurable -- the one
             * property of an array that is not an element. */
            status = kiln_c99_put(NULL, built, "value", 5,
                                  kiln_c99_number((double)array->count), 0, NULL);
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "writable", 8, kiln_c99_boolean(1), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "enumerable", 10, kiln_c99_boolean(0), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "configurable", 12, kiln_c99_boolean(0), 0,
                                      NULL);
            }
            *out = built;
            *found = 1;
            return status;
        }
        if (at >= 0.0) {
            if ((size_t)at >= array->count || array->elements[(size_t)at] == KILN_VAL_HOLE) {
                return KILN_C99_OK;
            }
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_put(NULL, built, "value", 5, kiln_val_to_boundary(array->elements[(size_t)at]), 0, NULL);
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "writable", 8,
                                      kiln_c99_boolean(!array->frozen), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "enumerable", 10, kiln_c99_boolean(1), 0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "configurable", 12,
                                      kiln_c99_boolean(!array->frozen), 0, NULL);
            }
            *out = built;
            *found = 1;
            return status;
        }
    }
    {
        kiln_value *slot = kiln_c99_exotic_store(target);
        if (slot != NULL) {
            store = *slot;
        }
    }
    if (store.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_OK;
    }
    {
        kiln_object *object = kiln_c99_object_of(store);
        size_t index;
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
        }
        for (index = 0; index < object->count; ++index) {
            kiln_property property;
            if (!kiln_c99_same_key_view(&object->properties[index], view)) {
                continue;
            }
            property = object->properties[index];
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (property.accessor) {
                status = kiln_c99_put(NULL, built, "get", 3, property.getter, 0, NULL);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, built, "set", 3, property.setter, 0, NULL);
                }
            } else {
                status = kiln_c99_put(NULL, built, "value", 5, property.value, 0, NULL);
                if (status == KILN_C99_OK) {
                    /* A frozen object reports every property as fixed without having rewritten
                     * each one, which is where the flag and the bits have to agree. */
                    status = kiln_c99_put(NULL, built, "writable", 8,
                                          kiln_c99_boolean(property.writable && !object->frozen),
                                          0, NULL);
                }
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "enumerable", 10,
                                      kiln_c99_boolean(kiln_c99_property_is_enumerable(&property)),
                                      0, NULL);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, built, "configurable", 12,
                                      kiln_c99_boolean(property.configurable && !object->frozen),
                                      0, NULL);
            }
            *out = built;
            *found = 1;
            return status;
        }
    }
    return KILN_C99_OK;
}


/* Fixed C names use a borrowed Latin-1 view; the headed implementation above owns the semantic
 * descriptor walk and keeps non-ASCII and UTF-16 keys in unit space. */
static int kiln_c99_describe_property(kiln_host *host, kiln_value target, const char *key,
                                      size_t key_len, kiln_value *out, int *found, int depth,
                                      kiln_value *thrown) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    return kiln_c99_describe_property_key(host, target, &view, NULL, out, found, depth, thrown);
}

/* Whether `key` is an own property of `target` -- the single question behind both
 * `Object.prototype.hasOwnProperty.call(x, k)` and `Object.hasOwn(x, k)`.
 *
 * WHY THE TWO SHARE THIS. They were two implementations of one question and they had drifted:
 * `Object.hasOwn` already answered for a function and for a proxy, `hasOwnProperty` refused both,
 * and only one of them converted a symbol key to the slot a symbol actually keys. A bundled
 * `lodash`, `immer` and `mobx` all stopped on the older of the two, at three different lines, and
 * every one of them reached it through `.call` off `Object.prototype` -- which is how a
 * transpiled module always spells this and therefore the spelling that has to work.
 *
 * WHY IT IS NOT `kiln_c99_describe_property` ALONE. That one has to produce a descriptor, so it
 * refuses wherever the *attributes* are not modelled even when the presence plainly is: a
 * function's `prototype` is unmistakably own and this backend does not record whether it is
 * writable. Presence is the weaker question and can be answered in cases the stronger one cannot.
 *
 * WHY SOME KINDS STILL REFUSE. `false` here is a real answer -- a feature-detect reads it as "the
 * property is not there" and takes the other branch -- so it may only be given where the whole of
 * the receiver's own storage is modelled. A built-in carries members this backend has not grown
 * yet, and a string, a regular expression, a buffer and a view have own properties the two
 * engines do not yet agree about; for those, refusing names the gap instead of inventing an
 * answer to it. */
static int kiln_c99_own_property_present(kiln_host *host, kiln_value target, kiln_value key,
                                         int *found, int depth, kiln_value *thrown) {
    /* The slot a key actually names, not the text it prints as: `obj[Symbol.iterator]` lives
     * under the symbol's reserved key, and converting through `kiln_c99_text` instead asked after
     * a property spelled `Symbol(Symbol.iterator)` -- which is never the one that was stored. */
    kiln_value name = kiln_c99_property_key(key);
    kiln_value descriptor;
    kiln_object *object;
    size_t index;
    *found = 0;
    /* `ToObject(this)` comes first, and there is no object to convert these two to. */
    if (target.tag == KILN_VALUE_UNDEFINED || target.tag == KILN_VALUE_NULL) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot convert undefined or null to object");
    }
    if (name.tag != KILN_VALUE_STRING) {
        KILN_C99_REFUSE("an own-property question about a key with no string spelling");
    }
    const kiln_string *view = (const kiln_string *)name.ptr;
    const kiln_string *canonical = NULL;
    if (kiln_c99_is_node_hidden_key_key(view)) {
        return KILN_C99_OK;
    }
    switch (target.tag) {
    case KILN_VALUE_PROXY:
        /* `[[GetOwnProperty]]` is a trap, so the handler answers and no store is read. */
        return kiln_c99_proxy_describe(host, target, name, &descriptor, found, depth, thrown);
    case KILN_VALUE_OBJECT:
        object = kiln_c99_object_of(target);
        if (object->shape != NULL) {
            uint32_t slot;
            const kiln_string *ikey = kiln_c99_interned_key_in(kiln_c99_gc_heap(), view);
            if (ikey != NULL && kiln_c99_shape_find_slot(object->shape, ikey, &slot)) {
                *found = 1;
                return KILN_C99_OK;
            }
            if (object->elements != NULL) {
                double index_num = kiln_c99_key_index_key(view);
                if (index_num >= 0.0 && index_num < 100000.0) {
                    size_t elem_idx = (size_t)index_num;
                    if (elem_idx < object->element_count &&
                        object->elements[elem_idx] != KILN_VAL_HOLE) {
                        *found = 1;
                        return KILN_C99_OK;
                    }
                }
            }
            if (object->properties == NULL) {
                *found = 0;
                return KILN_C99_OK;
            }
            kiln_c99_ensure_dictionary(object);
        }
        for (index = 0; index < object->count && !*found; ++index) {
            *found = kiln_c99_same_key_view(&object->properties[index], view);
        }
        return KILN_C99_OK;
    case KILN_VALUE_ARRAY:
        /* Indices, `length` and the store beside them -- the three the descriptor path already
         * knows about, so it answers rather than a fourth restatement of them. */
        return kiln_c99_describe_property_key(host, target, view, canonical, &descriptor, found,
                                          depth, thrown);
    case KILN_VALUE_FUNCTION: {
        kiln_closure *closure = kiln_c99_closure_of(target);
        if (kiln_c99_same_name_key(view, "prototype")) {
            if (!closure->arrow &&
                (!closure->asynchronous || closure->async_generator) &&
                (!closure->is_method || closure->generator)) {
                *found = 1;
                return KILN_C99_OK;
            }
            if (closure->properties.tag == KILN_VALUE_OBJECT) {
                kiln_object *store = kiln_c99_object_of(closure->properties);
                size_t index;
                if (store->shape != NULL) {
                    kiln_c99_ensure_dictionary(store);
                }
                for (index = 0; index < store->count; ++index) {
                    if (kiln_c99_same_key_view(&store->properties[index], view)) {
                        *found = 1;
                        return KILN_C99_OK;
                    }
                }
            }
            *found = 0;
            return KILN_C99_OK;
        }
        return kiln_c99_describe_property_key(host, target, view, canonical, &descriptor, found,
                                          depth, thrown);
    }
    case KILN_VALUE_METHOD:
    case KILN_VALUE_BOUND:
    case KILN_VALUE_SETTLER:
    case KILN_VALUE_COMMONJS_REQUIRE: {
        /* `name` and `length` are derived from the callable rather than stored, and everything
         * else a program hung on it lives in the store the same helper a read uses hands back --
         * so the two cannot disagree about where a bound function keeps its properties. Reading
         * `b.tag` and asking `Object.hasOwn(b, "tag")` were answering 1 and false, which is the
         * silent shape: the value is plainly there and the question about it says no. */
        kiln_value store;
        int stored;
        *found = kiln_c99_same_name_key(view, "name") ||
                 kiln_c99_same_name_key(view, "length");
        if (*found) {
            return KILN_C99_OK;
        }
        stored = kiln_c99_callable_store(target, &store, 0);
        if (stored != KILN_C99_OK) {
            return stored;
        }
        if (store.tag == KILN_VALUE_OBJECT) {
            kiln_object *held = kiln_c99_object_of(store);
            if (held->shape != NULL) {
                kiln_c99_ensure_dictionary(held);
            }
            for (index = 0; index < held->count && !*found; ++index) {
                *found = kiln_c99_same_key_view(&held->properties[index], view);
            }
        }
        return KILN_C99_OK;
    }
    case KILN_VALUE_TYPED_ARRAY: {
        int enumerable;
        kiln_c99_typed_own_key(target, view, found, &enumerable);
        return KILN_C99_OK;
    }
    case KILN_VALUE_COLLECTION:
    case KILN_VALUE_PROMISE:
    case KILN_VALUE_DATE:
    case KILN_VALUE_GENERATOR:
    case KILN_VALUE_TEMPORAL:
    case KILN_VALUE_NUMBER:
    case KILN_VALUE_BOOLEAN:
    case KILN_VALUE_BIGINT:
    case KILN_VALUE_SYMBOL:
        /* Every member of these lives on the prototype -- `size`, `description`, `getTime` --
         * and none of them can be given one of its own, so `false` is the complete answer rather
         * than an admission. It is the answer node gives, and the answer the interpreter gives,
         * and the one `lodash` needs when it asks a `Map` for `Symbol.toStringTag`. */
        return KILN_C99_OK;
    case KILN_VALUE_BUILTIN: {
        int enumerable;
        kiln_value names;
        if (kiln_c99_typed_prototype_own_key(target, view, NULL, found, &enumerable)) {
            return KILN_C99_OK;
        }
        /* A kiln extension this process withholds (ADR 0068) is a member the description names
         * and the realm left out, so its absence is known rather than a gap: `false`, as Node. */
        if (!kiln_string_is_utf16(view) &&
            kiln_c99_extension_withheld(target.bits, (const char *)kiln_string_latin1(view),
                                        view->unit_len)) {
            return KILN_C99_OK;
        }
        /* The names the table answers to, asked the same way `getOwnPropertyNames` asks so the
         * two cannot disagree. A hit is a member the program can reach and is therefore own; a
         * miss is not proof of absence, because the table is still growing, so it refuses. */
        if (kiln_c99_builtin_own_keys(target.bits, &names) == KILN_C99_OK &&
            kiln_c99_keys_contain(names, view)) {
            *found = 1;
            return KILN_C99_OK;
        }
        KILN_C99_REFUSE("an own-property question about a built-in member the surface table does "
                        "not list");
    }
    default:
        /* A string, a regular expression, a buffer and a view all have own properties -- indices,
         * `lastIndex`, `byteLength` -- and the two engines do not yet answer alike about them.
         * Refusing says so; answering would pick one of the two disagreements at random. */
        KILN_C99_REFUSE("an own-property question about a value the two engines do not answer "
                        "alike about");
    }
}

/* Define a method: a data property that is reachable by name and never enumerated.
 *
 * `kiln_c99_put` is an assignment and creates an enumerated property, which is right for an
 * object literal and wrong for a class body -- `for (const k in new C())` visits the fields the
 * constructor assigned and nothing the class declared. */
/* `SetFunctionName(function, key, prefix)`.
 *
 * A method, a getter and a setter are each created anonymous and named by the definition that
 * installs them, which is also the only place a computed key is known: `{ get [k]() {} }` can
 * only be named once `k` has been evaluated. The name is written onto the closure because that
 * is where `f.name` is read from; the store beside it holds what a program assigned.
 *
 * A symbol names the property after its description, in brackets, and one with no description
 * names it nothing -- while the prefix is applied to that nothing all the same. A private name
 * is spelled the way the program wrote it, `#m`, rather than as the scope-numbered reservation
 * the store keys it under. */
static int kiln_c99_set_function_name(kiln_value function, kiln_value key, const char *prefix) {
    static const char private_prefix[] = "@@private:";
    kiln_closure *closure;
    kiln_value named = kiln_c99_from_units(NULL, 0);
    kiln_value part;
    int no_description_symbol = 0;
    if (function.tag != KILN_VALUE_FUNCTION) {
        return KILN_C99_OK;
    }
    if (key.tag == KILN_VALUE_SYMBOL) {
        kiln_symbol *symbol = kiln_c99_symbol_of(key);
        kiln_value description;
        if (symbol->description_value.tag == KILN_VALUE_STRING || symbol->description_len != 0) {
            description = symbol->description_value.tag == KILN_VALUE_STRING
                              ? symbol->description_value
                              : kiln_c99_copy_text(symbol->description, symbol->description_len);
            if (description.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
            named = kiln_c99_concat(kiln_c99_static_text("[", 1), description);
            if (named.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
            named = kiln_c99_concat(named, kiln_c99_static_text("]", 1));
            if (named.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        } else {
            no_description_symbol = 1;
        }
    } else {
        kiln_value name = kiln_c99_property_key(key);
        const kiln_string *key_string;
        if (name.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a function name taken from a key with no string spelling");
        }
        key_string = (const kiln_string *)name.ptr;
        if (kiln_string_starts_ascii(key_string, private_prefix, sizeof(private_prefix) - 1)) {
            size_t index = sizeof(private_prefix) - 1;
            while (index < key_string->unit_len && kiln_string_unit_at(key_string, index) != ':') {
                ++index;
            }
            if (index < key_string->unit_len) {
                ++index;
            }
            kiln_string suffix = *key_string;
            suffix.unit_len -= (uint32_t)index;
            if (kiln_string_is_utf16(key_string)) {
                suffix.data = (const uint16_t *)key_string->data + index;
            } else {
                suffix.data = (const uint8_t *)key_string->data + index;
            }
            name = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)&suffix,
                                  suffix.unit_len);
            named = kiln_c99_concat(kiln_c99_static_text("#", 1), name);
        } else {
            named = name;
        }
        if (named.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    }
    if (prefix != NULL) {
        part = kiln_c99_concat(kiln_c99_static_text(prefix, strlen(prefix)),
                               kiln_c99_static_text(" ", 1));
        if (part.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
        named = kiln_c99_concat(part, named);
        if (named.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    }
    closure = kiln_c99_closure_of(function);
    /* A no-description symbol with no prefix remains the anonymous/null spelling; an empty
     * computed string key still gets the canonical empty header, preserving its inferred-name
     * identity for consumers that distinguish anonymous from explicitly empty names. */
    closure->name = named.len == 0 && prefix == NULL && no_description_symbol
                        ? NULL
                        : (const kiln_string *)named.ptr;
    return KILN_C99_OK;
}

/* Install a private method or accessor half, carrying the kind the class body declared.
 *
 * A method is stored not-writable, which is what tells it from a private *field* on a later
 * write: the language refuses the one and allows the other, and a plain slot cannot say which it
 * is. An accessor half goes through the ordinary accessor definition, which merges `get #x` with
 * the `set #x` written beside it -- two definitions describing one element. */
static int kiln_c99_define_private(kiln_value target, const char *key, size_t key_len,
                                   kiln_value function, int kind) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_property *slot;
    int status;
    if (kind != 0) {
        return kiln_c99_define_accessor(target, key, key_len,
                                        kind == 1 ? function : kiln_c99_undefined(),
                                        kind == 2 ? function : kiln_c99_undefined(), 0);
    }
    if (target.tag == KILN_VALUE_OBJECT && key_len <= UINT32_MAX) {
        kiln_string view;
        view.unit_len = (uint32_t)key_len;
        view.flags = KILN_STRING_LATIN1;
        view.data = key;
        if (kiln_c99_define_shaped(target, &view, kiln_val_from_boundary(function), 3, &status))
            return status;
    }
    status = kiln_c99_define_method(target, key, key_len, function);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot = kiln_c99_own_private(target, key, key_len);
    if (slot == NULL) {
        KILN_C99_REFUSE("a private definition that left no property slot");
    }
    slot->writable = 0;
    /* The flag belongs to the object that holds the property: a static private method's is the
     * class's property store, and a function value is not a `kiln_object` -- written through
     * `kiln_c99_object_of(target)` it landed in the closure itself. */
    {
        kiln_value holder =
            target.tag == KILN_VALUE_FUNCTION ? kiln_c99_closure_of(target)->properties : target;
        if (holder.tag == KILN_VALUE_OBJECT) kiln_c99_object_of(holder)->read_only_seen = 1;
    }
    return KILN_C99_OK;
}

/* The headed form, routing each kind through the keyed definition its byte form
 * reaches for. */
static int kiln_c99_define_private_key(kiln_value target, const kiln_string *key,
                                       kiln_value function, int kind) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_property *slot;
    int status;
    if (kind != 0) {
        return kiln_c99_define_accessor_key(target, key,
                                            kind == 1 ? function : kiln_c99_undefined(),
                                            kind == 2 ? function : kiln_c99_undefined(), 0);
    }
    if (target.tag == KILN_VALUE_OBJECT &&
        kiln_c99_define_shaped(target, key, kiln_val_from_boundary(function), 3, &status))
        return status;
    status = kiln_c99_define_method_key(target, key, function);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot = kiln_c99_own_private_key(target, key);
    if (slot == NULL) {
        KILN_C99_REFUSE("a private definition that left no property slot");
    }
    slot->writable = 0;
    /* The flag belongs to the object that holds the property: a static private method's is the
     * class's property store, and a function value is not a `kiln_object` -- written through
     * `kiln_c99_object_of(target)` it landed in the closure itself. */
    {
        kiln_value holder =
            target.tag == KILN_VALUE_FUNCTION ? kiln_c99_closure_of(target)->properties : target;
        if (holder.tag == KILN_VALUE_OBJECT) kiln_c99_object_of(holder)->read_only_seen = 1;
    }
    return KILN_C99_OK;
}

static int kiln_c99_define_method_key(kiln_value target, const kiln_string *key,
                                      kiln_value method) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_object *object;
    size_t index;
    int status;
    if (target.tag == KILN_VALUE_FUNCTION || target.tag == KILN_VALUE_METHOD ||
        target.tag == KILN_VALUE_BUILTIN) {
        kiln_value store;
        status = kiln_c99_callable_store(target, &store, 1);
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_define_method_key(store, key, method);
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        KILN_C99_REFUSE("a method definition on a value with no property store");
    }
    {
        int shaped = kiln_c99_define_shaped(target, key, kiln_val_from_boundary(method), 0,
                                            &status);
        if (shaped) return status;
    }
    status = kiln_c99_put_mode_key(NULL, target, key, NULL, method, 0, 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    object = kiln_c99_object_of(target);
    if (object->shape != NULL) {
        kiln_c99_ensure_dictionary(object);
        object->shape = NULL;
    }
    for (index = 0; index < object->count; ++index) {
        if (kiln_c99_same_key_view(&object->properties[index], key)) {
            object->properties[index].enumerable = 0;
            return KILN_C99_OK;
        }
    }
    KILN_C99_REFUSE("a method definition that left no property slot");
}

/* Fixed C names use a borrowed Latin-1 view; the headed implementation above owns the write. */
static int kiln_c99_define_method(kiln_value target, const char *key, size_t key_len,
                                  kiln_value method) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_INVALID_ARGUMENT;
    view.unit_len = (uint32_t)key_len;
    view.flags = KILN_STRING_LATIN1;
    view.data = key;
    return kiln_c99_define_method_key(target, &view, method);
}

/* JSON.
 *
 * Serialising can *call*: `toJSON` runs before anything else is decided about a value, and a
 * property may be an accessor. So this takes a host and a depth like every other read does. */
