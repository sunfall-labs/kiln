
static struct kiln_shape *kiln_c99_shape_root(void) {
    return &kiln_c99_root_shape;
}

static int kiln_c99_shape_find_slot(const kiln_shape *shape, const char *ikey, uint32_t *out_slot) {
    uint32_t i;
    if (shape == NULL || ikey == NULL) {
        return 0;
    }
    if (shape->ikey == ikey) {
        if (out_slot) {
            *out_slot = shape->slot_index;
        }
        return 1;
    }
    for (i = 0; i < shape->property_count; ++i) {
        if (shape->keys[i] == ikey) {
            if (out_slot) {
                *out_slot = i;
            }
            return 1;
        }
    }
    return 0;
}

/* The slot of the key spelt `bytes` in `shape`, or UINT32_MAX when the shape has no such key. A
 * spelling the key table has never seen is a key no shape holds, so the lookup never interns. A
 * sunk spread (see `emit_sunk_bridge` in the emitter) learns its source's slots through this,
 * once per shape it meets. */
static uint32_t kiln_c99_shape_slot_named(const kiln_shape *shape, const char *bytes, size_t len) {
    const char *ikey = kiln_c99_interned(bytes, len);
    uint32_t slot;
    if (ikey == NULL || !kiln_c99_shape_find_slot(shape, ikey, &slot)) {
        return UINT32_MAX;
    }
    return slot;
}

static void kiln_c99_pin_interned_key(const char *ikey) {
    kiln_gc_heap *heap;
    kiln_key_entry *entry;
    size_t len;
    size_t i;
    if (ikey == NULL || ikey == kiln_c99_key_empty) return;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return;
    len = kiln_c99_key_len(ikey);
    entry = kiln_c99_key_probe(&heap->keys, ikey, len, kiln_c99_key_hash(ikey, len));
    if (entry != NULL) {
        entry->pinned = 1;
        return;
    }
    for (i = 0; i < heap->keys.capacity; ++i) {
        if (heap->keys.entries[i].bytes == ikey) {
            heap->keys.entries[i].pinned = 1;
            return;
        }
    }
}

static int kiln_c99_shape_transition(kiln_shape *current, const char *ikey, kiln_shape **out_shape) {
    kiln_shape_transition *tr;
    kiln_shape *next_shape;
    size_t i;
    if (current == NULL || ikey == NULL || current->property_count >= KILN_SHAPE_MAX_FAST_PROPERTIES) {
        return 0;
    }
    for (i = 0; i < 2 && i < current->num_transitions; ++i) {
        if (current->inline_trans_ikey[i] == ikey) {
            *out_shape = current->inline_trans_next[i];
            return 1;
        }
    }
    for (tr = current->transitions; tr != NULL; tr = tr->next_transition) {
        if (tr->ikey == ikey) {
            *out_shape = tr->next;
            return 1;
        }
    }
    if (current->num_transitions >= 32) {
        return 0;
    }
    next_shape = (kiln_shape *)calloc(1, sizeof(kiln_shape));
    if (next_shape == NULL) {
        return 0;
    }
    kiln_c99_pin_interned_key(ikey);
    next_shape->parent = current;
    next_shape->ikey = ikey;
    next_shape->has_symbol_key = current->has_symbol_key || (ikey[0] == '@' && ikey[1] == '@');
    next_shape->slot_index = current->property_count;
    next_shape->property_count = current->property_count + 1;
    next_shape->num_transitions = 0;
    memcpy((void *)next_shape->keys, (const void *)current->keys,
           current->property_count * sizeof(const char *));
    next_shape->keys[current->property_count] = ikey;

    if (current->num_transitions < 2) {
        current->inline_trans_ikey[current->num_transitions] = ikey;
        current->inline_trans_next[current->num_transitions] = next_shape;
    } else {
        kiln_shape_transition *new_tr = (kiln_shape_transition *)calloc(1, sizeof(kiln_shape_transition));
        if (new_tr == NULL) {
            free(next_shape);
            return 0;
        }
        new_tr->ikey = ikey;
        new_tr->next = next_shape;
        new_tr->next_transition = current->transitions;
        current->transitions = new_tr;
    }
    current->num_transitions++;

    *out_shape = next_shape;
    return 1;
}

static int kiln_c99_ensure_dictionary(kiln_object *object) {
    kiln_c99_prototype_touched(object);
    kiln_shape *shape;
    size_t i;
    int status;
    size_t active_elements = 0;
    size_t total_properties;
    size_t pos = 0;
    if (object == NULL) {
        return KILN_C99_OK;
    }
    shape = object->shape;
    if (shape == NULL) {
        return KILN_C99_OK;
    }
    if (object->elements != NULL) {
        for (i = 0; i < object->element_count; ++i) {
            if (object->elements[i] != KILN_VAL_HOLE) {
                active_elements++;
            }
        }
    }
    total_properties = shape->property_count + active_elements;
    if (total_properties > object->capacity) {
        status = kiln_c99_reserve((void **)&object->properties, &object->capacity,
                                  total_properties, sizeof *object->properties);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    if (object->elements != NULL) {
        for (i = 0; i < object->element_count; ++i) {
            if (object->elements[i] == KILN_VAL_HOLE) continue;
            char num_buf[32];
            int nlen = kiln_number_to_js_string((double)i, num_buf, sizeof num_buf);
            const char *ikey = kiln_c99_intern(num_buf, (size_t)nlen);
            object->properties[pos].key = ikey;
            object->properties[pos].key_len = (size_t)nlen;
            object->properties[pos].enumerable = 1;
            object->properties[pos].writable = 1;
            object->properties[pos].configurable = 1;
            object->properties[pos].symbol_hidden = 0;
            object->properties[pos].accessor = 0;
            object->properties[pos].refused = 0;
            object->properties[pos].value = kiln_val_to_boundary(object->elements[i]);
            object->properties[pos].getter = kiln_c99_undefined();
            object->properties[pos].setter = kiln_c99_undefined();
            pos++;
        }
        free(object->elements);
        object->elements = NULL;
        object->element_count = 0;
        object->element_capacity = 0;
    }
    for (i = 0; i < shape->property_count; ++i) {
        const char *key = shape->keys[i];
        size_t key_len = kiln_c99_key_len(key);
        object->properties[pos].key = key;
        object->properties[pos].key_len = key_len;
        object->properties[pos].enumerable = !(key_len >= 2 && key[0] == '@' && key[1] == '@');
        object->properties[pos].writable = 1;
        object->properties[pos].configurable = 1;
        object->properties[pos].symbol_hidden = 0;
        object->properties[pos].accessor = 0;
        object->properties[pos].refused = 0;
        object->properties[pos].value = (object->slots != NULL && i < object->slot_capacity)
                                          ? kiln_val_to_boundary(object->slots[i])
                                          : kiln_c99_undefined();
        object->properties[pos].getter = kiln_c99_undefined();
        object->properties[pos].setter = kiln_c99_undefined();
        pos++;
    }
    object->count = pos;
    if (object->slots != NULL && object->slots != object->inline_slots) {
        free(object->slots);
    }
    object->slots = NULL;
    object->slot_capacity = 0;
    object->shape = NULL;
    return KILN_C99_OK;
}

static int kiln_c99_new_object(kiln_value *out) {
    /* Every field is written below before anything can observe the object, so the block is
     * taken unzeroed: the zeroing was a second pass over two hundred bytes per object. */
    kiln_object *object = (kiln_object *)kiln_c99_gc_alloc_pointer_free(KILN_GC_OBJECT, sizeof *object);
    if (object == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    object->properties = NULL;
    object->count = 0;
    object->capacity = 0;
    object->prototype = kiln_c99_undefined();
    object->frozen = 0;
    object->not_extensible = 0;
    object->module_namespace = 0;
    object->arguments_brand = 0;
    object->error_brand = 0;
    object->cache_holder = 0;
    object->accessor_count = 0;
    object->read_only_seen = 0;
    object->shape = &kiln_c99_root_shape;
    object->slots = object->inline_slots;
    object->slot_capacity = KILN_C99_INLINE_SLOTS;
    for (size_t slot_i = 0; slot_i < KILN_C99_INLINE_SLOTS; ++slot_i) {
        object->inline_slots[slot_i] = KILN_VAL_UNDEFINED;
    }
    object->elements = NULL;
    object->element_count = 0;
    object->element_capacity = 0;
    *out = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)object, NULL, 0);
    return KILN_C99_OK;
}

/* An object whose property array is sized once, for a caller that already knows how many members
 * it is about to add.
 *
 * `kiln_c99_reserve` doubles from four, which is the right policy for a program building an
 * object a member at a time and the wrong one for the runtime filling a fixed template: a
 * `node:http` request with twelve own properties walked 4 -> 8 -> 16, paying a `realloc` and a
 * copy at each step to arrive at the capacity it could have asked for. The capacity is a hint,
 * not a limit -- passing a number too small only restores the doubling. */
static int kiln_c99_new_object_with_capacity(kiln_value *out, size_t capacity) {
    kiln_object *object;
    int status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) {
        return status;
    }
    object = kiln_c99_object_of(*out);
    if (capacity > KILN_C99_INLINE_SLOTS) {
        kiln_val *new_slots = (kiln_val *)malloc(capacity * sizeof(kiln_val));
        size_t i;
        if (new_slots == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (i = 0; i < capacity; ++i) {
            new_slots[i] = KILN_VAL_UNDEFINED;
        }
        object->slots = new_slots;
        object->slot_capacity = capacity;
    }
    return KILN_C99_OK;
}

static int kiln_c99_new_object_tenured(kiln_value *out) {
    kiln_object *object = (kiln_object *)kiln_c99_gc_alloc_tenured(KILN_GC_OBJECT, sizeof *object);
    if (object == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    object->properties = NULL;
    object->count = 0;
    object->capacity = 0;
    object->prototype = kiln_c99_undefined();
    object->frozen = 0;
    object->not_extensible = 0;
    object->module_namespace = 0;
    object->arguments_brand = 0;
    object->error_brand = 0;
    object->cache_holder = 0;
    object->accessor_count = 0;
    object->read_only_seen = 0;
    object->shape = &kiln_c99_root_shape;
    object->slots = object->inline_slots;
    object->slot_capacity = KILN_C99_INLINE_SLOTS;
    for (size_t slot_i = 0; slot_i < KILN_C99_INLINE_SLOTS; ++slot_i) {
        object->inline_slots[slot_i] = KILN_VAL_UNDEFINED;
    }
    object->elements = NULL;
    object->element_count = 0;
    object->element_capacity = 0;
    *out = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)object, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_new_object_with_capacity_tenured(kiln_value *out, size_t capacity) {
    kiln_object *object;
    int status = kiln_c99_new_object_tenured(out);
    if (status != KILN_C99_OK) {
        return status;
    }
    object = kiln_c99_object_of(*out);
    if (capacity > KILN_C99_INLINE_SLOTS) {
        kiln_val *new_slots = (kiln_val *)malloc(capacity * sizeof(kiln_val));
        size_t i;
        if (new_slots == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (i = 0; i < capacity; ++i) {
            new_slots[i] = KILN_VAL_UNDEFINED;
        }
        object->slots = new_slots;
        object->slot_capacity = capacity;
    }
    return KILN_C99_OK;
}
