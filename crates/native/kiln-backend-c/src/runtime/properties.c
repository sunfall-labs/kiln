
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

static double kiln_c99_key_index_key(const kiln_string *key) {
    char buffer[64];
    size_t index;
    if (key == NULL || key->unit_len == 0 || key->unit_len > 10) return -1.0;
    for (index = 0; index < key->unit_len; ++index) {
        uint32_t unit = kiln_string_unit_at(key, index);
        if (unit > 0x7f) return -1.0;
        buffer[index] = (char)unit;
    }
    return kiln_c99_key_index(buffer, key->unit_len);
}

static int kiln_c99_same_name(const char *key, size_t key_len, const char *name) {
    size_t len = strlen(name);
    return len == key_len && memcmp(key, name, len) == 0;
}

static int kiln_c99_same_name_key(const kiln_string *key, const char *name) {
    size_t len = 0;
    size_t index;
    while (name[len] != '\0') ++len;
    if (key == NULL || key->unit_len != len) return 0;
    if (len >= 2 && name[0] == '@' && name[1] == '@' &&
        !kiln_c99_key_is_reserved(key)) return 0;
    for (index = 0; index < len; ++index) {
        if (kiln_string_unit_at(key, index) != (unsigned char)name[index]) return 0;
    }
    return 1;
}

static int kiln_c99_is_node_hidden_key_key(const kiln_string *key) {
    static const char *const names[] = {
        "@@node:socket", "@@node:server", "@@node:client", "@@node:response",
        "@@kiln:node:is-file", "@@kiln:node:is-directory",
    };
    size_t index;
    for (index = 0; index < sizeof names / sizeof names[0]; ++index) {
        if (kiln_c99_same_name_key(key, names[index])) return 1;
    }
    return 0;
}

static int kiln_c99_same_key_view(const kiln_property *property, const kiln_string *key) {
    return property != NULL && key != NULL &&
           (property->key == key ||
            (kiln_c99_key_domain(property->key) == kiln_c99_key_domain(key) &&
             kiln_string_equal(property->key, key)));
}

static int kiln_c99_same_key(const kiln_property *property, const char *key, size_t key_len) {
    kiln_string view;
    if (key_len > UINT32_MAX) return 0;
    view.unit_len = (uint32_t)key_len;
    view.flags = kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_same_key_view(property, &view);
}

/* The slot on `object` holding the canonical key `ikey`, or `object->count` when there is none.
 * A NULL `ikey` is a spelling the realm never interned, and since every stored key is interned
 * (`kiln_c99_own_slot` is the one place a key is written and it always canonicalizes) that is a
 * key no object has: the scan is skipped rather than run. `KILN_KEYS_CHECK=1` re-runs it by
 * bytes and aborts on a disagreement, which is how "every stored key is interned" is tested
 * rather than assumed. */
static size_t kiln_c99_own_index_key(const kiln_object *object, const kiln_string *ikey,
                                     const kiln_string *key) {
    size_t index;
    size_t found;
    if (object->shape != NULL) {
        kiln_c99_ensure_dictionary((kiln_object *)object);
        if (ikey == NULL) {
            ikey = kiln_c99_interned_key_in(kiln_c99_gc_heap(), key);
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
            if (kiln_c99_same_key_view(&object->properties[index], key)) {
                found = index;
                break;
            }
        }
    }
    if (kiln_c99_keys_checked()) {
        size_t by_bytes = object->count;
        for (index = 0; index < object->count; ++index) {
            if (kiln_c99_same_key_view(&object->properties[index], key)) {
                by_bytes = index;
                break;
            }
        }
        if (by_bytes != found) {
            fputs("kiln: interned-key invariant broken for `", stderr);
            if (key != NULL && !kiln_string_is_utf16(key)) {
                fwrite(kiln_string_latin1(key), 1, key->unit_len, stderr);
            } else {
                fputs("<utf16>", stderr);
            }
            fputs(found == object->count ? "`: stored under a non-canonical pointer\n"
                                         : "`: two slots answer one key\n",
                  stderr);
            abort();
        }
    }
    return found;
}

static size_t kiln_c99_own_index(const kiln_object *object, const kiln_string *ikey,
                                 const char *key, size_t key_len) {
    kiln_string view;
    if (key_len > UINT32_MAX) return object->count;
    view.unit_len = (uint32_t)key_len;
    view.flags = ikey != NULL ? KILN_STRING_LATIN1 | kiln_c99_key_domain(ikey)
                              : kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_own_index_key(object, ikey, &view);
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

/* The exact form of the test above, for one key: whether a write of `key` that finds nothing on
 * the receiver itself would be taken by the prototype chain -- a setter (or a getter with none)
 * under that key, or a non-writable data property of that name -- instead of creating an own
 * data property. It walks the chain exactly as `kiln_c99_put_mode_key`'s slow path does, so the
 * two cannot disagree: a level that never held an accessor or a read-only property is skipped by
 * pointer, and a writable data property found first shadows everything above it.
 *
 * The coarse test answers for the whole prototype, which made every instance of a class with a
 * single getter (`Request`, `Response`, `Headers`, every class with `get [Symbol.toStringTag]`)
 * a dictionary object from its first field on. */
#define KILN_C99_LEVEL_NONE 0
#define KILN_C99_LEVEL_DATA 1
#define KILN_C99_LEVEL_READONLY 2
#define KILN_C99_LEVEL_ACCESSOR 3

/* What one level of a prototype chain holds under `key`, for a write that did not find it on the
 * receiver: nothing, a writable data property, a read-only one, or an accessor (whose setter is
 * `*setter`). A shaped level answers from its shape's attributes and stays shaped -- a class
 * prototype with a getter is walked by every first write of every instance's fields. */
static int kiln_c99_level_write_answer(kiln_object *walked, const kiln_string *ikey,
                                       const kiln_string *key, kiln_value *setter) {
    if (walked->shape != NULL) {
        const kiln_string *canonical =
            ikey != NULL ? ikey : kiln_c99_interned_key_in(kiln_c99_gc_heap(), key);
        uint32_t slot;
        uint32_t attrs;
        if (canonical == NULL || !kiln_c99_shape_find_slot(walked->shape, canonical, &slot))
            return KILN_C99_LEVEL_NONE;
        attrs = walked->shape->attrs[slot];
        if (attrs & KILN_C99_ATTR_ACCESSOR) {
            *setter = slot < walked->slot_capacity
                          ? kiln_c99_accessor_pair_half(walked->slots[slot], 1)
                          : kiln_c99_undefined();
            return KILN_C99_LEVEL_ACCESSOR;
        }
        return (attrs & KILN_C99_ATTR_READONLY) ? KILN_C99_LEVEL_READONLY : KILN_C99_LEVEL_DATA;
    }
    {
        size_t index = kiln_c99_own_index_key(walked, ikey, key);
        if (index == walked->count) return KILN_C99_LEVEL_NONE;
        if (walked->properties[index].accessor) {
            *setter = walked->properties[index].setter;
            return KILN_C99_LEVEL_ACCESSOR;
        }
        return walked->properties[index].writable ? KILN_C99_LEVEL_DATA : KILN_C99_LEVEL_READONLY;
    }
}

static int kiln_c99_inherited_write_blocked(kiln_value proto, const kiln_string *ikey,
                                            const kiln_string *key) {
    kiln_value current = proto;
    size_t steps;
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        kiln_object *walked;
        if (current.tag != KILN_VALUE_OBJECT) return 0;
        walked = kiln_c99_object_of(current);
        if (walked->accessor_count != 0 || walked->read_only_seen) {
            kiln_value setter;
            int answer = kiln_c99_level_write_answer(walked, ikey, key, &setter);
            if (answer != KILN_C99_LEVEL_NONE) return answer != KILN_C99_LEVEL_DATA;
        }
        current = walked->prototype;
    }
    return 1;
}

#define KILN_C99_PATH_NONE 0
#define KILN_C99_PATH_FOUND 1
#define KILN_C99_PATH_ABSENT 2

/* The path from `object` for canonical `key`, stated as a cache entry states it (ADR 0071):
 * `entry`'s `depth` and levels are filled in as the walk goes. FOUND: the key is a data property
 * of the level at `depth` (1 or 2), whose slot -- or, in list mode, index, where the record must
 * carry the canonical pointer -- is `proto_slot`, and `*holder` is that level. ABSENT: no level up
 * to `depth` has the key and the last one's prototype link is not an object; `absent` is set.
 * NONE for anything else: an accessor or a refused record, a level that is not an ordinary
 * object, a key deeper than two steps, a level that cannot be named, a reserved key unless
 * `symbol` vouches for it. `object`'s own properties are the caller's to have checked. */
static int kiln_c99_pic_learn_path_as(kiln_c99_pic_entry *entry, const kiln_object *object,
                                      const kiln_string *key, const kiln_object **holder,
                                      int symbol) {
    const kiln_object *level = object;
    uint32_t d;
    if (key == NULL || (!symbol && kiln_c99_key_is_reserved(key)) ||
        kiln_c99_same_name_key(key, "__proto__")) {
        return KILN_C99_PATH_NONE;
    }
    entry->key = key;
    for (d = 0; d < 2; ++d) {
        const kiln_object *candidate;
        if (level->prototype.tag != KILN_VALUE_OBJECT) break;
        candidate = (const kiln_object *)(uintptr_t)level->prototype.bits;
        if (candidate == NULL || kiln_c99_gc_kind_of_ptr((void *)candidate) != KILN_GC_OBJECT) {
            return KILN_C99_PATH_NONE;
        }
        entry->depth = (uint8_t)(d + 1);
        if (candidate->shape != NULL) {
            uint32_t slot;
            entry->level_shape[d] = candidate->shape;
            entry->level_token[d] = 0;
            if (kiln_c99_shape_find_slot(candidate->shape, key, &slot)) {
                /* An accessor's slot holds its pair: reading it is a call, never a cached read. */
                if (slot >= candidate->slot_capacity ||
                    (candidate->shape->accessor_mask & (1u << slot)) != 0)
                    return KILN_C99_PATH_NONE;
                entry->proto_slot = slot;
                *holder = candidate;
                return KILN_C99_PATH_FOUND;
            }
        } else {
            size_t i;
            for (i = 0; i < candidate->count && candidate->properties != NULL; ++i) {
                const kiln_property *property = &candidate->properties[i];
                if (property->key == key || kiln_string_equal(property->key, key)) {
                    if (property->key != key || property->accessor || property->refused) {
                        return KILN_C99_PATH_NONE;
                    }
                    /* The holder is named by the key its record carries, not by a token. */
                    entry->level_shape[d] = NULL;
                    entry->level_token[d] = 0;
                    entry->proto_slot = (uint32_t)i;
                    *holder = candidate;
                    return KILN_C99_PATH_FOUND;
                }
            }
            /* A list-mode level the key is not on: only its layout token can say it has not
             * gained the key since. */
            if (!kiln_c99_pic_name_level(candidate, &entry->level_shape[d], &entry->level_token[d])) {
                return KILN_C99_PATH_NONE;
            }
        }
        level = candidate;
    }
    if (level->prototype.tag == KILN_VALUE_OBJECT) return KILN_C99_PATH_NONE;
    entry->absent = 1;
    return KILN_C99_PATH_ABSENT;
}

/* A named site's path. A reserved (`@@`) key is refused: a named site never carries a symbol's,
 * and the runtime's own slots have hiding rules the slow path keeps. `symbol` in the form above
 * is a symbol-keyed site's (`kiln_c99_val_read_symbol`), whose key is a symbol's by
 * construction. */
static int kiln_c99_pic_learn_path(kiln_c99_pic_entry *entry, const kiln_object *object,
                                   const kiln_string *key, const kiln_object **holder) {
    return kiln_c99_pic_learn_path_as(entry, object, key, holder, 0);
}

/* Whether a transitioning store an inline-cache entry remembers may run on `object`: nothing on
 * its prototype chain intercepts the entry's key. The coarse walk answers for most objects. For
 * one whose chain holds accessors, the exact answer is remembered in the entry as a path (ADR
 * 0071): the key is on none of the levels up to where the chain ends, each named by shape or
 * layout token and checked live from the receiver. `key` NULL means the caller has no canonical
 * key at hand and takes the coarse answer. */
static __attribute__((noinline)) int kiln_c99_transition_store_clear_slow(
    kiln_c99_site *site, const kiln_c99_pic_entry *entry, const kiln_object *object,
    const kiln_string *ikey, const kiln_string *key);

static inline int kiln_c99_transition_store_clear(kiln_c99_site *site,
                                                  const kiln_c99_pic_entry *entry,
                                                  const kiln_object *object,
                                                  const kiln_string *ikey,
                                                  const kiln_string *key) {
    kiln_value proto = object->prototype;
    if (proto.tag != KILN_VALUE_OBJECT) return 1;
    if (entry->absent) {
        const kiln_object *end = kiln_c99_pic_path(entry, object);
        if (KILN_LIKELY(end != NULL && end->prototype.tag != KILN_VALUE_OBJECT)) {
#ifdef KILN_IC_CHECK
            if (key != NULL && kiln_c99_inherited_write_blocked(proto, ikey, key)) {
                fputs("kiln: inline-cache oracle: a cached transitioning store is intercepted along an unchanged path; a mutation path forgot kiln_c99_prototype_touched\n", stderr);
                __builtin_trap();
            }
#endif
            return 1;
        }
    }
    if (!kiln_c99_has_inherited_setter_or_readonly(proto)) return 1;
    return kiln_c99_transition_store_clear_slow(site, entry, object, ikey, key);
}

static __attribute__((noinline)) int kiln_c99_transition_store_clear_slow(
    kiln_c99_site *site, const kiln_c99_pic_entry *entry, const kiln_object *object,
    const kiln_string *ikey, const kiln_string *key) {
    kiln_c99_pic_entry path;
    const kiln_object *holder = NULL;
    if (key == NULL) return 0;
    if (kiln_c99_inherited_write_blocked(object->prototype, ikey, key)) return 0;
    /* Clear. Remember why when the key is absent along a path of at most two levels: the same
     * transition, restated with the path, replaces the entry (a published entry never changes). */
    memset(&path, 0, sizeof path);
    if (ikey != NULL &&
        kiln_c99_pic_learn_path_as(&path, object, ikey, &holder, entry->keyed) ==
            KILN_C99_PATH_ABSENT) {
        path.shape = object->shape;
        path.transition_to = entry->transition_to;
        path.slot = entry->slot;
        path.keyed = entry->keyed;
        kiln_c99_site_remember(site, &path);
    }
    return 1;
}

/* A write to an existing key replaces it in place, so insertion order is the order of first
 * appearance -- which is the order `Object.keys` reports and the order a console prints. */
/* `ikey` as for `kiln_c99_get_ikey`: a compiled site's canonical pointer, or NULL to look it
 * up. The headed form is the semantic implementation; fixed C names use the adapter below. */
static int kiln_c99_own_slot_key(kiln_value target, const kiln_string *key,
                                 const kiln_string *ikey, kiln_property **out);

static int kiln_c99_put_mode_key(kiln_host *host, kiln_value target, const kiln_string *key,
                                 const kiln_string *ikey, kiln_value value, int strict,
                                 int depth, kiln_value *thrown);

static int kiln_c99_put_mode(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                             kiln_value value, int strict, int depth, kiln_value *thrown) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_OUT_OF_MEMORY;
    view.unit_len = (uint32_t)key_len;
    view.flags = kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_put_mode_key(host, target, &view, NULL, value, strict, depth, thrown);
}

/* Byte callers above are fixed-name adapters. They never hand their borrowed view to user code:
 * the headed implementation interns before a proxy trap or a property is retained. */
static int kiln_c99_put_mode_key(kiln_host *host, kiln_value target, const kiln_string *key,
                                 const kiln_string *ikey, kiln_value value, int strict,
                                 int depth, kiln_value *thrown) {
    const kiln_string *stable_key = ikey;
    if (target.tag == KILN_VALUE_SHARED)
        return strict ? kiln_c99_throw_read_only_key(thrown, key) : KILN_C99_OK;
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    kiln_object *object;
    size_t index;
    int status;
    /* Node resource pointers are represented by reserved values in the ordinary property store,
     * but they are not JavaScript properties.  A program can spell the key as a string, so hide
     * and ignore these protocol slots explicitly instead of exposing a C pointer as an object or
     * allowing an assignment to replace it. */
    if (kiln_c99_is_node_hidden_key_key(key)) {
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_PROXY) {
        /* Writing to a proxy is a call, exactly as reading one is. */
        if (stable_key == NULL) stable_key = kiln_c99_intern_key(key);
        if (stable_key == NULL) return KILN_C99_OUT_OF_MEMORY;
        return kiln_c99_proxy_set(host, target,
                                  kiln_c99_value(KILN_VALUE_STRING, 0,
                                                 (const char *)stable_key, stable_key->unit_len),
                                  value, strict, depth, thrown);
    }
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_closure *closure = kiln_c99_closure_of(target);
        /* `f.prototype = {...}` replaces the object `new f()` links instances to, which is a
         * slot on the closure rather than an ordinary property -- a write that landed in the
         * store would be read back by `f.prototype` and ignored by `new`. */
        if (kiln_c99_same_name_key(key, "prototype")) {
            kiln_value store = kiln_c99_closure_store(closure);
            kiln_value *slot;
            if (store.tag == KILN_VALUE_OBJECT && kiln_c99_object_of(store)->frozen) {
                return strict ? kiln_c99_throw_read_only_key(thrown, key) : KILN_C99_OK;
            }
            slot = kiln_c99_closure_prototype_slot(closure);
            if (slot == NULL) return KILN_C99_OUT_OF_MEMORY;
            /* The slot's undefined means "not created yet" (`kiln_c99_prototype_object`), so an
             * assigned undefined is kept as a hole, which that function reads back as the
             * undefined the program wrote rather than as a request to make one. */
            *slot = value.tag == KILN_VALUE_UNDEFINED
                        ? kiln_c99_value(KILN_VALUE_HOLE, 0, NULL, 0)
                        : value;
            return KILN_C99_OK;
        }
        {
            kiln_value store;
            status = kiln_c99_function_store(target, &store);
            if (status != KILN_C99_OK) {
                return status;
            }
            return kiln_c99_put_mode_key(host, store, key, ikey, value, strict, depth, thrown);
        }
    }
    if (target.tag == KILN_VALUE_METHOD || target.tag == KILN_VALUE_BUILTIN ||
        target.tag == KILN_VALUE_BOUND) {
        kiln_value store;
        status = kiln_c99_callable_store(target, &store, 1);
        if (status != KILN_C99_OK) return status;
        return kiln_c99_put_mode_key(host, store, key, ikey, value, strict, depth, thrown);
    }
    if (target.tag == KILN_VALUE_REGEXP && kiln_c99_same_name_key(key, "lastIndex")) {
        /* The one property of a regexp a program may write, and the reason two evaluations of one
         * literal must not share a `kiln_regexp`. */
        kiln_c99_regexp_of(target)->last_index = kiln_c99_as_number(value);
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        /* An array is not an object here, but it can still carry a property that is not an
         * element -- which is how `exec` reports `index` and `input`. */
        kiln_array *array = kiln_c99_array_of(target);
        if (kiln_c99_same_name_key(key, "length")) {
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
                return strict ? kiln_c99_throw_read_only_key(thrown, key) : KILN_C99_OK;
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
            return strict ? kiln_c99_throw_read_only_key(thrown, key) : KILN_C99_OK;
        }
        if (array->properties.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&array->properties);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_put_mode_key(host, array->properties, key, ikey, value, strict, depth,
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
        return kiln_c99_put_mode_key(host, collection->properties, key, ikey, value, strict, depth,
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
        return kiln_c99_put_mode_key(host, promise->properties, key, ikey, value, strict, depth,
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
        return strict ? kiln_c99_throw_module_namespace_write_key(thrown, key)
                       : KILN_C99_OK;
    }
    if (object->frozen) {
        return strict ? kiln_c99_throw_read_only_key(thrown, key) : KILN_C99_OK;
    }
    /* Interned here, once, rather than looked up here and interned again in `own_slot` when
     * the write turns out to create the property: a fresh object being filled by the prelude --
     * a request, a response, a URL -- is a run of new-key writes, and each was paying two
     * hashes. A refused write leaves a weak entry behind, which the next collection drops. */
    if (ikey == NULL) {
        ikey = kiln_c99_intern_key(key);
        if (ikey == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    if (kiln_c99_same_name_key(key, "__proto__") &&
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
            double index_num = kiln_c99_key_index_key(key);
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
            if (KILN_UNLIKELY(object->shape->store_mask & (1u << slot))) {
                /* An own read-only property refuses the write; an own accessor takes it -- as
                 * the list-mode records below would, without leaving shape mode. */
                kiln_value setter;
                /* A writable method slot whose code the shape records (step 3c): stored in
                 * place for the same code, otherwise after dropping the record. */
                if ((object->shape->code_mask & (1u << slot)) != 0) {
                    if (kiln_c99_shape_store_recorded(object, slot, kiln_val_from_boundary(value)))
                        return KILN_C99_OK;
                    return kiln_c99_put_mode_key(host, target, key, ikey, value, strict, depth,
                                                 thrown);
                }
                if ((object->shape->accessor_mask & (1u << slot)) == 0 ||
                    slot >= object->slot_capacity) {
                    return strict ? kiln_c99_throw_read_only_key(thrown, key) : KILN_C99_OK;
                }
                setter = kiln_c99_accessor_pair_half(object->slots[slot], 1);
                if (setter.tag == KILN_VALUE_UNDEFINED) {
                    return strict ? kiln_c99_throw_read_only_key(thrown, key) : KILN_C99_OK;
                }
                {
                    kiln_value ignored;
                    return kiln_c99_call(host, &ignored, setter, target, &value, 1, depth, thrown);
                }
            }
            if (slot < object->slot_capacity) {
                if ((object->shape->double_slots_mask & (1u << slot)) &&
                    value.tag != KILN_VALUE_NUMBER) {
                    kiln_c99_ensure_dictionary(object);
                    return kiln_c99_put_mode_key(host, target, key, ikey, value, strict, depth, thrown);
                }
                object->slots[slot] = kiln_val_from_boundary(value);
                return KILN_C99_OK;
            }
        } else if (!object->not_extensible &&
                   (object->prototype.tag == KILN_VALUE_UNDEFINED ||
                    !kiln_c99_has_inherited_setter_or_readonly(object->prototype) ||
                    (ikey != NULL &&
                     !kiln_c99_inherited_write_blocked(object->prototype, ikey, key)))) {
            double index_num = kiln_c99_key_index_key(key);
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
        index = kiln_c99_own_index_key(object, ikey, key);
    if (index != object->count) {
        if (!object->properties[index].accessor) {
            /* A refused write is observable only in strict code. */
            if (object->properties[index].writable) {
                object->properties[index].value = value;
            } else if (strict) {
                return kiln_c99_throw_read_only_key(thrown, key);
            }
            return KILN_C99_OK;
        }
        if (object->properties[index].setter.tag == KILN_VALUE_UNDEFINED) {
            if (strict) {
                return kiln_c99_throw_read_only_key(thrown, key);
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
                kiln_value setter;
                int answer = kiln_c99_level_write_answer(walked, ikey, key, &setter);
                if (answer != KILN_C99_LEVEL_NONE) {
                    if (answer == KILN_C99_LEVEL_READONLY) {
                        return strict ? kiln_c99_throw_read_only_key(thrown, key)
                                       : KILN_C99_OK;
                    }
                    if (answer == KILN_C99_LEVEL_DATA) {
                        /* A writable data property up the chain: nothing intercepts, and the
                         * write shadows it with an own property below. */
                        break;
                    }
                    if (setter.tag == KILN_VALUE_UNDEFINED) {
                        if (strict) {
                            return kiln_c99_throw_read_only_key(thrown, key);
                        }
                        return KILN_C99_OK;
                    }
                    {
                        kiln_value ignored;
                        return kiln_c99_call(host, &ignored, setter, target, &value, 1, depth,
                                             thrown);
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
            if (kiln_c99_own_index_key(object, ikey, key) == object->count) {
                return strict ? kiln_c99_throw_read_only_key(thrown, key) : KILN_C99_OK;
            }
        }
        status = kiln_c99_own_slot_key(target, key, ikey, &slot);
        if (status != KILN_C99_OK) {
            return status;
        }
        slot->value = value;
    }
    return KILN_C99_OK;
}

static int kiln_c99_put_mode_ikey(kiln_host *host, kiln_value target, const char *key,
                                  size_t key_len, const kiln_string *ikey, kiln_value value,
                                  int strict, int depth, kiln_value *thrown) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_OUT_OF_MEMORY;
    view.unit_len = (uint32_t)key_len;
    view.flags = ikey != NULL ? KILN_STRING_LATIN1 | kiln_c99_key_domain(ikey)
                              : kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_put_mode_key(host, target, &view, ikey, value, strict, depth, thrown);
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
static int kiln_c99_own_slot_key(kiln_value target, const kiln_string *key,
                                 const kiln_string *ikey, kiln_property **out);

static int kiln_c99_own_slot(kiln_value target, const char *key, size_t key_len,
                             kiln_property **out) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_OUT_OF_MEMORY;
    view.unit_len = (uint32_t)key_len;
    view.flags = kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_own_slot_key(target, &view, NULL, out);
}

/* `ikey` is the canonical pointer when the caller holds one that is already in the table -- a
 * compiled site's, or a write path's successful lookup -- or NULL to intern here. */
static int kiln_c99_own_slot_ikey(kiln_value target, const char *key, size_t key_len,
                                  const kiln_string *ikey, kiln_property **out) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_OUT_OF_MEMORY;
    view.unit_len = (uint32_t)key_len;
    view.flags = ikey != NULL ? KILN_STRING_LATIN1 | kiln_c99_key_domain(ikey)
                              : kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_own_slot_key(target, &view, ikey, out);
}

static int kiln_c99_own_slot_key(kiln_value target, const kiln_string *key,
                                 const kiln_string *ikey, kiln_property **out) {
    kiln_object *object;
    size_t index;
    int status;
    size_t key_len = key == NULL ? 0 : key->unit_len;
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
        ikey = kiln_c99_intern_key(key);
        if (ikey == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    index = kiln_c99_own_index_key(object, ikey, key);
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
        double slot_index = kiln_c99_key_index_key(key);
        size_t position = object->count;
        if (slot_index >= 0.0) {
            for (index = 0; index < object->count; ++index) {
                double existing = kiln_c99_key_index_key(object->properties[index].key);
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
        /* The key's domain, rather than its text, says whether it is an internal slot. A
         * program string that happens to begin with `@@` remains an ordinary enumerable key. */
        object->properties[position].enumerable = !kiln_c99_key_is_reserved(ikey);
        object->properties[position].writable = 1;
        object->properties[position].configurable = 1;
        object->properties[position].symbol_hidden = 0;
        object->properties[position].accessor = 0;
        object->properties[position].refused = 0;
        object->properties[position].value = kiln_c99_undefined();
        *out = &object->properties[position];
    }
    object->count += 1;
    object->shape = NULL;
    return KILN_C99_OK;
}

/* Commit one complete own data descriptor. Bootstrap callers can supply the final attributes
 * when the key is inserted instead of taking a writable record pointer and fixing it later.
 * The ordinary assignment path still handles inherited setters and read-only properties. */
static int kiln_c99_commit_own_data_key(kiln_value target, const kiln_string *key,
                                        kiln_value value, int enumerable, int writable,
                                        int configurable) {
    kiln_property *slot;
    kiln_object *object;
    int status = kiln_c99_own_slot_key(target, key, NULL, &slot);
    if (status != KILN_C99_OK) return status;
    object = kiln_c99_object_of(target);
    kiln_c99_prototype_touched(object);
    if (slot->accessor && object->accessor_count != 0) object->accessor_count -= 1;
    slot->accessor = 0;
    slot->refused = 0;
    slot->value = value;
    slot->enumerable = enumerable && !kiln_c99_key_is_reserved(slot->key);
    slot->writable = writable;
    slot->configurable = configurable;
    slot->symbol_hidden = 0;
    if (!enumerable && kiln_c99_key_is_reserved(slot->key)) {
        kiln_value symbol;
        if (kiln_c99_symbol_of_key_view(slot->key, &symbol)) slot->symbol_hidden = 1;
    }
    if (!writable) object->read_only_seen = 1;
    return KILN_C99_OK;
}

static int kiln_c99_commit_own_data(kiln_value target, const char *key, size_t key_len,
                                    kiln_value value, int enumerable, int writable,
                                    int configurable) {
    kiln_string view;
    if (key_len > UINT32_MAX) return KILN_C99_OUT_OF_MEMORY;
    view.unit_len = (uint32_t)key_len;
    view.flags = kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_commit_own_data_key(target, &view, value, enumerable, writable,
                                         configurable);
}

/* Preserve DefineMethod's assignment semantics, including a setter on the prototype or an
 * existing read-only own property, while keeping its record update inside property storage.
 * A future direct descriptor/value store can implement this boundary without exposing a
 * writable kiln_property pointer to the definition caller. */
static int kiln_c99_commit_method_key(kiln_value target, const kiln_string *key,
                                      kiln_value method) {
    kiln_object *object;
    size_t index;
    int status = kiln_c99_put_mode_key(NULL, target, key, NULL, method, 0, 0, NULL);
    if (status != KILN_C99_OK) return status;
    object = kiln_c99_object_of(target);
    if (object->shape != NULL) {
        kiln_c99_ensure_dictionary(object);
        object->shape = NULL;
    }
    for (index = 0; index < object->count; ++index) {
        kiln_property *slot = &object->properties[index];
        kiln_value symbol;
        if (!kiln_c99_same_key_view(slot, key)) continue;
        slot->enumerable = 0;
        if (kiln_c99_key_is_reserved(slot->key) &&
            kiln_c99_symbol_of_key_view(slot->key, &symbol))
            slot->symbol_hidden = 1;
        return KILN_C99_OK;
    }
    KILN_C99_REFUSE("a method definition that left no property slot");
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
static const kiln_string *kiln_c99_hot_key(int id) {
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
