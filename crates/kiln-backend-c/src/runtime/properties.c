
/* Doubling, so building an object or an array one member at a time stays linear overall. */
static int kiln_c99_reserve(void **items, size_t *capacity, size_t needed, size_t item_size) {
    size_t grown = *capacity == 0 ? 4 : *capacity;
    void *moved;
    if (needed <= *capacity) {
        return KILN_C99_OK;
    }
    while (grown < needed) {
        if (grown > ((size_t)-1) / 2 / item_size) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        grown *= 2;
    }
    moved = realloc(*items, grown * item_size);
    if (moved == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    *items = moved;
    *capacity = grown;
    return KILN_C99_OK;
}

/* The index a key names, or -1.  A key is an index only when it round-trips through the
 * canonical decimal spelling, which is what keeps `a["01"]` a named property and `a["1"]` an
 * element -- the distinction the language draws and the one a naive `atoi` erases. */
static double kiln_c99_key_index(const char *key, size_t key_len) {
    char buffer[64];
    double number;
    int length;
    /* Every canonical index spelling is one to ten ASCII digits with no sign and no leading
     * space: the largest index is 4294967294, and a number that round-trips cannot begin with
     * anything but a digit.  Both are *necessary* conditions, so failing either is a decision
     * this can make without the provider -- which matters because the provider call is a
     * cross-crate FFI hop and every property write on every object makes one.  Nothing about
     * `a["01"]` or `a["1"]` moves: the round-trip below still decides those. */
    if (key_len == 0 || key_len > 10 ||
        (unsigned char)(key[0] - '0') > 9u) {
        return -1.0;
    }
    number = kiln_string_to_number(key, key_len);
    if (!(number >= 0.0) || floor(number) != number || number > 4294967294.0) {
        return -1.0;
    }
    length = kiln_c99_double_to_js(number, buffer);
    if (length != key_len || memcmp(buffer, key, key_len) != 0) {
        return -1.0;
    }
    return number;
}

static int kiln_c99_same_name(const char *key, size_t key_len, const char *name) {
    size_t len = strlen(name);
    return len == key_len && memcmp(key, name, len) == 0;
}

static int kiln_c99_same_key(const kiln_property *property, const char *key, size_t key_len) {
    if (property->key_len != key_len) {
        return 0;
    }
    if (key_len == 0) {
        return 1;
    }
    /* Two pointers into the same bytes are the same key -- trivially, not by assumption -- and
     * are common: an emitted constant is one static array shared by every read and write of a
     * given literal name, and a property that was itself interned this way keeps that pointer
     * when it is looked up again by the same literal. Different pointers say nothing either way,
     * which is why this falls through to a real comparison rather than refusing on the strength
     * of not matching -- a dynamic key can hold the same bytes as an interned one at some other
     * address. */
    if (property->key == key) {
        return 1;
    }
    if (property->key[0] != key[0]) {
        return 0;
    }
    return memcmp(property->key, key, key_len) == 0;
}

/* The slot on `object` holding the canonical key `ikey`, or `object->count` when there is none.
 * A NULL `ikey` is a spelling the realm never interned, and since every stored key is interned
 * (`kiln_c99_own_slot` is the one place a key is written and it always canonicalizes) that is a
 * key no object has: the scan is skipped rather than run. `KILN_KEYS_CHECK=1` re-runs it by
 * bytes and aborts on a disagreement, which is how "every stored key is interned" is tested
 * rather than assumed. */
static size_t kiln_c99_own_index(const kiln_object *object, const char *ikey, const char *key,
                                 size_t key_len) {
    size_t index;
    size_t found;
    if (object->shape != NULL) {
        kiln_c99_ensure_dictionary((kiln_object *)object);
        if (ikey == NULL) {
            ikey = kiln_c99_interned_in(kiln_c99_gc_heap(), key, key_len);
        }
    }
    found = object->count;
    if (ikey != NULL) {
        for (index = 0; index < object->count; ++index) {
            if (object->properties[index].key == ikey) {
                found = index;
                break;
            }
        }
    } else {
        for (index = 0; index < object->count; ++index) {
            if (object->properties[index].key_len == key_len &&
                memcmp(object->properties[index].key, key, key_len) == 0) {
                found = index;
                break;
            }
        }
    }
    if (kiln_c99_keys_checked()) {
        size_t by_bytes = object->count;
        for (index = 0; index < object->count; ++index) {
            if (kiln_c99_same_key(&object->properties[index], key, key_len)) {
                by_bytes = index;
                break;
            }
        }
        if (by_bytes != found) {
            fputs("kiln: interned-key invariant broken for `", stderr);
            fwrite(key, 1, key_len, stderr);
            fputs(found == object->count ? "`: stored under a non-canonical pointer\n"
                                         : "`: two slots answer one key\n",
                  stderr);
            abort();
        }
    }
    return found;
}

static int kiln_c99_has_inherited_setter_or_readonly(kiln_value proto) {
    kiln_value current = proto;
    size_t steps;
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        kiln_object *walked;
        if (current.tag != KILN_VALUE_OBJECT) {
            break;
        }
        walked = kiln_c99_object_of(current);
        if (walked->accessor_count != 0 || walked->read_only_seen) {
            return 1;
        }
        current = walked->prototype;
    }
    return 0;
}

/* A write to an existing key replaces it in place, so insertion order is the order of first
 * appearance -- which is the order `Object.keys` reports and the order a console prints. */
static int kiln_c99_put_mode(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                             kiln_value value, int strict, int depth, kiln_value *thrown) {
    return kiln_c99_put_mode_ikey(host, target, key, key_len, NULL, value, strict, depth, thrown);
}

/* `ikey` as for `kiln_c99_get_ikey`: a compiled site's canonical pointer, or NULL to look it
 * up. */
static int kiln_c99_put_mode_ikey(kiln_host *host, kiln_value target, const char *key,
                                  size_t key_len, const char *ikey, kiln_value value, int strict,
                                  int depth, kiln_value *thrown) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_object *object;
    size_t index;
    int status;
    /* Node resource pointers are represented by reserved values in the ordinary property store,
     * but they are not JavaScript properties.  A program can spell the key as a string, so hide
     * and ignore these protocol slots explicitly instead of exposing a C pointer as an object or
     * allowing an assignment to replace it. */
    if (kiln_c99_is_node_hidden_key(key, key_len)) {
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_PROXY) {
        /* Writing to a proxy is a call, exactly as reading one is. */
        return kiln_c99_proxy_set(host, target,
                                  kiln_c99_value(KILN_VALUE_STRING, 0, key, key_len), value,
                                  strict, depth, thrown);
    }
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_closure *closure = kiln_c99_closure_of(target);
        /* `f.prototype = {...}` replaces the object `new f()` links instances to, which is a
         * slot on the closure rather than an ordinary property -- a write that landed in the
         * store would be read back by `f.prototype` and ignored by `new`. */
        if (kiln_c99_same_name(key, key_len, "prototype")) {
            if (closure->properties.tag == KILN_VALUE_OBJECT &&
                kiln_c99_object_of(closure->properties)->frozen) {
                return strict ? kiln_c99_throw_read_only(thrown, key, key_len) : KILN_C99_OK;
            }
            closure->prototype = value;
            return KILN_C99_OK;
        }
        if (closure->properties.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&closure->properties);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_put_mode(host, closure->properties, key, key_len, value, strict, depth,
                                 thrown);
    }
    if (target.tag == KILN_VALUE_METHOD || target.tag == KILN_VALUE_BUILTIN ||
        target.tag == KILN_VALUE_BOUND) {
        kiln_value store;
        status = kiln_c99_callable_store(target, &store, 1);
        if (status != KILN_C99_OK) return status;
        return kiln_c99_put_mode(host, store, key, key_len, value, strict, depth, thrown);
    }
    if (target.tag == KILN_VALUE_REGEXP && kiln_c99_same_name(key, key_len, "lastIndex")) {
        /* The one property of a regexp a program may write, and the reason two evaluations of one
         * literal must not share a `kiln_regexp`. */
        kiln_c99_regexp_of(target)->last_index = kiln_c99_as_number(value);
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        /* An array is not an object here, but it can still carry a property that is not an
         * element -- which is how `exec` reports `index` and `input`. */
        kiln_array *array = kiln_c99_array_of(target);
        if (kiln_c99_same_name(key, key_len, "length")) {
            /* `length` is derived, not stored, so writing it is a resize: truncating drops the
             * tail and growing fills with holes, which are not undefined elements. */
            double wanted = kiln_c99_as_number(value);
            size_t at;
            if (!(wanted >= 0.0) || floor(wanted) != wanted || wanted > 4294967295.0) {
                /* A RangeError rather than a write that quietly does nothing. */
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "Invalid array length");
            }
            if (array->frozen) {
                return strict ? kiln_c99_throw_read_only(thrown, key, key_len) : KILN_C99_OK;
            }
            if ((size_t)wanted <= array->count) {
                array->count = (size_t)wanted;
                return KILN_C99_OK;
            }
            if (array->elements != NULL && !kiln_c99_array_elements_owned(array)) {
                size_t grown = array->capacity < 16 ? 16 : array->capacity;
                while (grown < (size_t)wanted) grown *= 2;
                kiln_val *moved = (kiln_val *)malloc(grown * sizeof(kiln_val));
                if (KILN_UNLIKELY(moved == NULL)) return KILN_C99_OUT_OF_MEMORY;
                memcpy(moved, array->elements, array->count * sizeof(kiln_val));
                array->elements = moved;
                array->capacity = grown;
            } else {
                status = kiln_c99_array_reserve(array, (size_t)wanted);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            for (at = array->count; at < (size_t)wanted; ++at) {
                array->elements[at] = KILN_VAL_HOLE;
            }
            array->count = (size_t)wanted;
            return KILN_C99_OK;
        }
        if (array->frozen) {
            return strict ? kiln_c99_throw_read_only(thrown, key, key_len) : KILN_C99_OK;
        }
        if (array->properties.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&array->properties);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_put_mode(host, array->properties, key, key_len, value, strict, depth,
                                 thrown);
    }
    if (target.tag == KILN_VALUE_COLLECTION) {
        kiln_collection *collection = kiln_c99_collection_of(target);
        if (collection->properties.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&collection->properties);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_put_mode(host, collection->properties, key, key_len, value, strict, depth,
                                 thrown);
    }
    if (target.tag == KILN_VALUE_PROMISE) {
        kiln_promise *promise = kiln_c99_promise_of(target);
        if (promise->properties.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&promise->properties);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_put_mode(host, promise->properties, key, key_len, value, strict, depth,
                                 thrown);
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        /* A primitive has nowhere to put a property, and every module is strict, so the language
         * ends the statement with a catchable TypeError rather than dropping the write. The
         * object-like tags that also land here -- a Date, a bound function -- are a different
         * thing: those *are* objects a program may write to, and this backend simply has no store
         * for them yet, so they keep saying so. */
        if (target.tag == KILN_VALUE_UNDEFINED || target.tag == KILN_VALUE_NULL ||
            target.tag == KILN_VALUE_BOOLEAN || target.tag == KILN_VALUE_NUMBER ||
            target.tag == KILN_VALUE_STRING || target.tag == KILN_VALUE_BIGINT ||
            target.tag == KILN_VALUE_SYMBOL) {
                return strict ? kiln_c99_throw_cannot_set(thrown, target) : KILN_C99_OK;
        }
        KILN_C99_REFUSE("a property write to a value whose store nothing enumerates");
    }
    object = kiln_c99_object_of(target);
    if (object->module_namespace) {
        return strict ? kiln_c99_throw_module_namespace_write(thrown, key, key_len)
                       : KILN_C99_OK;
    }
    if (object->frozen) {
        return strict ? kiln_c99_throw_read_only(thrown, key, key_len) : KILN_C99_OK;
    }
    /* Interned here, once, rather than looked up here and interned again in `own_slot` when
     * the write turns out to create the property: a fresh object being filled by the prelude --
     * a request, a response, a URL -- is a run of new-key writes, and each was paying two
     * hashes. A refused write leaves a weak entry behind, which the next collection drops. */
    if (ikey == NULL) {
        ikey = kiln_c99_intern(key, key_len);
        if (ikey == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    if (kiln_c99_same_name(key, key_len, "__proto__") &&
        object->prototype.tag != KILN_VALUE_NULL) {
        /* The other half of the accessor above: a write relinks the object instead of giving it
         * a property called `__proto__`. A primitive is ignored rather than refused, which is the
         * setter's rule and the literal's -- `Object.setPrototypeOf` is the spelling that
         * complains. */
        if (kiln_c99_can_be_prototype(value)) {
            object->prototype = value;
        }
        return KILN_C99_OK;
    }
    /* A setter found anywhere on the chain takes the write; only when there is none does the
     * assignment create an own data property. That asymmetry is what makes `obj.x = 1` on a
     * class with `set x()` run the setter rather than shadow it.
     *
     * The own list is checked on its own, first: an own *data* property shadows every inherited
     * setter -- per the specification, [[Set]] never consults the prototype once the receiver's
     * own property is found -- so a hit here ends the search without a single prototype touched.
     * That is most writes on a request or response past its first: the key is already there. */
    /* Fast path for shape-based objects (ADR 0050) */
    if (object->shape != NULL) {
        if (object->elements != NULL) {
            double index_num = kiln_c99_key_index(key, key_len);
            if (index_num >= 0.0 && index_num < 100000.0) {
                size_t elem_idx = (size_t)index_num;
                if (elem_idx < object->element_count && object->elements[elem_idx] != KILN_VAL_HOLE) {
                    object->elements[elem_idx] = kiln_val_from_boundary(value);
                    return KILN_C99_OK;
                }
            }
        }
        uint32_t slot;
        if (kiln_c99_shape_find_slot(object->shape, ikey, &slot)) {
            if (slot < object->slot_capacity) {
                if ((object->shape->double_slots_mask & (1u << slot)) &&
                    value.tag != KILN_VALUE_NUMBER) {
                    kiln_c99_ensure_dictionary(object);
                    return kiln_c99_put_mode_ikey(host, target, key, key_len, ikey, value, strict, depth, thrown);
                }
                object->slots[slot] = kiln_val_from_boundary(value);
                return KILN_C99_OK;
            }
        } else if (!object->not_extensible &&
                   (object->prototype.tag == KILN_VALUE_UNDEFINED ||
                    !kiln_c99_has_inherited_setter_or_readonly(object->prototype))) {
            double index_num = kiln_c99_key_index(key, key_len);
            if (index_num < 0.0) {
                kiln_shape *next_shape = NULL;
                int is_double = (value.tag == KILN_VALUE_NUMBER);
                if (kiln_c99_shape_transition(object->shape, ikey, is_double, &next_shape)) {
                    uint32_t new_slot = next_shape->slot_index;
                    if (new_slot >= object->slot_capacity) {
                        size_t new_cap = object->slot_capacity == 0 ? KILN_C99_INLINE_SLOTS : object->slot_capacity * 2;
                        kiln_val *new_slots;
                        while (new_slot >= new_cap) new_cap *= 2;
                        if (object->slots == object->inline_slots) {
                            new_slots = (kiln_val *)malloc(new_cap * sizeof(kiln_val));
                            if (new_slots == NULL) return KILN_C99_OUT_OF_MEMORY;
                            memcpy(new_slots, object->inline_slots, KILN_C99_INLINE_SLOTS * sizeof(kiln_val));
                        } else if (object->slots == NULL) {
                            new_slots = (kiln_val *)malloc(new_cap * sizeof(kiln_val));
                            if (new_slots == NULL) return KILN_C99_OUT_OF_MEMORY;
                        } else {
                            new_slots = (kiln_val *)realloc(object->slots, new_cap * sizeof(kiln_val));
                            if (new_slots == NULL) return KILN_C99_OUT_OF_MEMORY;
                        }
                        {
                            size_t fill_i;
                            for (fill_i = object->slot_capacity; fill_i < new_cap; ++fill_i) {
                                new_slots[fill_i] = KILN_VAL_UNDEFINED;
                            }
                        }
                        object->slots = new_slots;
                        object->slot_capacity = new_cap;
                    }
                    object->slots[new_slot] = kiln_val_from_boundary(value);
                    object->shape = next_shape;
                    return KILN_C99_OK;
                }
            } else if (index_num < 100000.0) {
                size_t elem_idx = (size_t)index_num;
                if (elem_idx >= object->element_capacity) {
                    size_t new_cap = object->element_capacity == 0 ? 8 : object->element_capacity * 2;
                    kiln_val *new_elements;
                    while (elem_idx >= new_cap) new_cap *= 2;
                    new_elements = (kiln_val *)realloc(object->elements, new_cap * sizeof(kiln_val));
                    if (new_elements == NULL) return KILN_C99_OUT_OF_MEMORY;
                    for (size_t fill_i = object->element_capacity; fill_i < new_cap; ++fill_i) {
                        new_elements[fill_i] = KILN_VAL_HOLE;
                    }
                    object->elements = new_elements;
                    object->element_capacity = new_cap;
                }
                object->elements[elem_idx] = kiln_val_from_boundary(value);
                if (elem_idx >= object->element_count) {
                    object->element_count = elem_idx + 1;
                }
                return KILN_C99_OK;
            }
        }
    }
    if (object->shape != NULL) {
        kiln_c99_ensure_dictionary(object);
    }
    index = kiln_c99_own_index(object, ikey, key, key_len);
    if (index != object->count) {
        if (!object->properties[index].accessor) {
            /* A refused write is observable only in strict code. */
            if (object->properties[index].writable) {
                object->properties[index].value = value;
            } else if (strict) {
                return kiln_c99_throw_read_only(thrown, key, key_len);
            }
            return KILN_C99_OK;
        }
        if (object->properties[index].setter.tag == KILN_VALUE_UNDEFINED) {
            if (strict) {
                return kiln_c99_throw_read_only(thrown, key, key_len);
            }
            return KILN_C99_OK;
        }
        {
            kiln_value ignored;
            return kiln_c99_call(host, &ignored, object->properties[index].setter, target, &value,
                                 1, depth, thrown);
        }
    }
    /* Nothing of this name on the object itself. Walk the prototypes looking for a setter to
     * run, or a non-writable data property that blocks creating an own one in its place -- a
     * level with no accessors of its own has no setter to offer, so `accessor_count == 0` skips
     * scanning its property list at all and the level costs one pointer follow instead, unless a
     * data property of that level was ever made read-only (`read_only_seen`), which is the other
     * thing a prototype can do to a write. */
    {
        kiln_value current = object->prototype;
        size_t steps;
        for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
            kiln_object *walked;
            if (current.tag != KILN_VALUE_OBJECT) {
                break;
            }
            walked = kiln_c99_object_of(current);
            if (walked->accessor_count != 0 || walked->read_only_seen) {
                index = kiln_c99_own_index(walked, ikey, key, key_len);
                if (index != walked->count) {
                    if (!walked->properties[index].accessor) {
                        if (!walked->properties[index].writable) {
                            return strict ? kiln_c99_throw_read_only(thrown, key, key_len)
                                           : KILN_C99_OK;
                        }
                        /* A writable data property up the chain: nothing intercepts, and the
                         * write shadows it with an own property below. */
                        break;
                    }
                    if (walked->properties[index].setter.tag == KILN_VALUE_UNDEFINED) {
                        if (strict) {
                            return kiln_c99_throw_read_only(thrown, key, key_len);
                        }
                        return KILN_C99_OK;
                    }
                    {
                        kiln_value ignored;
                        return kiln_c99_call(host, &ignored, walked->properties[index].setter,
                                             target, &value, 1, depth, thrown);
                    }
                }
            }
            current = walked->prototype;
        }
    }
    {
        kiln_property *slot;
        /* A non-extensible object takes a write to a property it already has and refuses to grow.
         * The walk above has already handled every own property, so arriving here with the flag
         * set means this key would be a new one. The wording is the frozen write's on purpose:
         * the interpreter words both the same, and `error.message` is program-visible. */
        if (object->not_extensible) {
            if (kiln_c99_own_index(object, ikey, key, key_len) == object->count) {
                return strict ? kiln_c99_throw_read_only(thrown, key, key_len) : KILN_C99_OK;
            }
        }
        status = kiln_c99_own_slot_ikey(target, key, key_len, ikey, &slot);
        if (status != KILN_C99_OK) {
            return status;
        }
        slot->value = value;
    }
    return KILN_C99_OK;
}

static int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value value, int depth, kiln_value *thrown) {
    return kiln_c99_put_mode(host, target, key, key_len, value, 0, depth, thrown);
}

/* Find or create the own property named `key`, bypassing the prototype chain entirely.
 *
 * An assignment cannot start here -- a setter anywhere above has first claim, and `kiln_c99_put`
 * walks for one before it arrives -- but a *definition* must: `Object.defineProperty` writes
 * past an inherited setter rather than through it, and reaching this by way of the assignment
 * path would call one.
 *
 * A fresh slot is what an assignment would have made: writable, configurable, enumerable, data.
 * A definition then overwrites exactly the fields its descriptor mentions. */
static int kiln_c99_own_slot(kiln_value target, const char *key, size_t key_len,
                             kiln_property **out) {
    return kiln_c99_own_slot_ikey(target, key, key_len, NULL, out);
}

/* `ikey` is the canonical pointer when the caller holds one that is already in the table -- a
 * compiled site's, or a write path's successful lookup -- or NULL to intern here. */
static int kiln_c99_own_slot_ikey(kiln_value target, const char *key, size_t key_len,
                                  const char *ikey, kiln_property **out) {
    kiln_object *object;
    size_t index;
    int status;
    if (target.tag != KILN_VALUE_OBJECT) {
        KILN_C99_REFUSE("an own-slot lookup on a value that is not an ordinary object");
    }
    object = kiln_c99_object_of(target);
    if (object->shape != NULL) {
        kiln_c99_ensure_dictionary(object);
    }
    /* The one place a key is stored, so the one place interning has to be total (ADR 0047):
     * whatever spelling arrived, the property carries the realm's canonical pointer for it. */
    if (ikey == NULL) {
        ikey = kiln_c99_intern(key, key_len);
        if (ikey == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    index = kiln_c99_own_index(object, ikey, key, key_len);
    if (index != object->count) {
        *out = &object->properties[index];
        return KILN_C99_OK;
    }
    status = kiln_c99_reserve((void **)&object->properties, &object->capacity, object->count + 1,
                              sizeof *object->properties);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* Integer-like keys come first, in ascending numeric order, and everything else follows in
     * insertion order.  That is the order the language specifies for own properties, so it is
     * the order a console prints and the order `Object.keys` would report -- keeping the list
     * sorted on insert means nothing downstream has to re-derive it. */
    {
        double slot_index = kiln_c99_key_index(key, key_len);
        size_t position = object->count;
        if (slot_index >= 0.0) {
            for (index = 0; index < object->count; ++index) {
                double existing = kiln_c99_key_index(object->properties[index].key,
                                                     object->properties[index].key_len);
                if (existing < 0.0 || existing > slot_index) {
                    break;
                }
            }
            position = index;
        }
        for (index = object->count; index > position; --index) {
            object->properties[index] = object->properties[index - 1];
        }
        object->properties[position].key = ikey;
        object->properties[position].key_len = key_len;
        /* A `@@` key is the machine's, not the program's: a private member, an error's kind, a
         * combinator's bookkeeping. Nothing that enumerates own string properties may see one, and
         * marking it here is one place rather than at each of the seven walks. The program cannot
         * spell such a key -- `obj["@@x"]` is a different property only because nothing in the
         * language produces this prefix. */
        object->properties[position].enumerable =
            !(key_len >= 2 && key[0] == '@' && key[1] == '@');
        object->properties[position].writable = 1;
        object->properties[position].configurable = 1;
        object->properties[position].symbol_hidden = 0;
        object->properties[position].accessor = 0;
        object->properties[position].refused = 0;
        object->properties[position].value = kiln_c99_undefined();
        object->properties[position].getter = kiln_c99_undefined();
        object->properties[position].setter = kiln_c99_undefined();
        *out = &object->properties[position];
    }
    object->count += 1;
    object->shape = NULL;
    return KILN_C99_OK;
}

/* Write an own data property the way an assignment would have left it, without asking the
 * prototype chain for permission first.
 *
 * `kiln_c99_put` walks the chain for a setter before every write, because for *program* code a
 * setter above has first claim.  The runtime's own initialisers are not program code: they fill
 * a freshly created object whose prototype is one the runtime just linked, so no setter can
 * exist to claim the write and the walk can only ever fail.  Building a `node:http` request ran
 * that walk twelve times per request over a three-deep prototype chain, for nothing.
 *
 * This is *not* a faster `put`.  It skips the hidden-key guard, the exotic-target dispatch, the
 * frozen and module-namespace refusals and `__proto__`'s relinking, so it is correct only for a
 * plain object the caller just made and a key the caller wrote as a literal.  Program-visible
 * assignment keeps going through `kiln_c99_put`. */
static int kiln_c99_own_data(kiln_value target, const char *key, size_t key_len,
                             kiln_value value) {
    kiln_property *slot;
    int status = kiln_c99_own_slot(target, key, key_len, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot->value = value;
    return KILN_C99_OK;
}

/* The canonical pointer for one of KILN_C99_HOT_KEYS in the active heap, interned on first use.
 *
 * Lazy rather than filled at heap init, so nothing depends on the order in which a realm brings
 * its heap up; a NULL slot simply means "not yet", and the first request through pays for it. */
static const char *kiln_c99_hot_key(int id) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    /* A `@@` name is a reserved slot reached through a Symbol, not a string key, and it is
     * resolved by a path this table knows nothing about. Interning one as a string and handing the
     * canonical pointer to `put` writes a property nothing later finds: measured as 5,793 wrong
     * answers out of 15k when `@@url:query` was briefly listed here. Refusing returns NULL, which
     * every caller already treats as "not interned, do it the ordinary way".
     *
     * The check is here rather than in a comment on the table because the table is the tempting
     * place to add one. */
    if (kiln_c99_hot_key_len[id] >= 2 && kiln_c99_hot_key_text[id][0] == '@' &&
        kiln_c99_hot_key_text[id][1] == '@') {
        return NULL;
    }
    if (heap == NULL) return NULL;
    if (heap->hot_keys[id] == NULL) {
        heap->hot_keys[id] =
            kiln_c99_intern(kiln_c99_hot_key_text[id], kiln_c99_hot_key_len[id]);
    }
    return heap->hot_keys[id];
}

/* `kiln_c99_put` for one of those keys, so a fresh object's members skip the hash too. */
static int kiln_c99_put_hot_key(kiln_host *host, kiln_value target, int id, kiln_value value,
                                int depth, kiln_value *thrown) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    return kiln_c99_put_mode_ikey(host, target, kiln_c99_hot_key_text[id], kiln_c99_hot_key_len[id],
                                  kiln_c99_hot_key(id), value, 0, depth, thrown);
}

/* `kiln_c99_own_data` for a key the HTTP path already knows the canonical pointer for. */
static int kiln_c99_own_data_hot_key(kiln_value target, int id, kiln_value value) {
    kiln_property *slot;
    int status = kiln_c99_own_slot_ikey(target, kiln_c99_hot_key_text[id],
                                        kiln_c99_hot_key_len[id], kiln_c99_hot_key(id), &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot->value = value;
    return KILN_C99_OK;
}
