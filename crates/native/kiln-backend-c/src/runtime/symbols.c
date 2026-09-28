
static kiln_symbol *kiln_c99_symbol_of(kiln_value value) {
    return (kiln_symbol *)(uintptr_t)value.ptr;
}

/* The specification's own symbols. One instance each, so equality is the pointer's exactly as it
 * is for a unique symbol -- there is no second kind of comparison to keep in step. Their keys are
 * static headers too: a property slot stores the same canonical shape as a user key, and no
 * reserved-name path smuggles a payload pointer into the store. */
#define KILN_C99_STATIC_SYMBOL_KEY(name, text) \
    static const char name##_bytes[] = text; \
    static const kiln_string name = {sizeof(text) - 1, KILN_STRING_LATIN1 | KILN_STRING_SYMBOL_KEY, name##_bytes}
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_iterator, "@@iterator");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_async_iterator, "@@asyncIterator");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_has_instance, "@@hasInstance");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_to_string_tag, "@@toStringTag");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_to_primitive, "@@toPrimitive");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_is_concat_spreadable, "@@isConcatSpreadable");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_match, "@@match");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_match_all, "@@matchAll");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_replace, "@@replace");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_search, "@@search");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_species, "@@species");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_split, "@@split");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_unscopables, "@@unscopables");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_dispose, "@@dispose");
KILN_C99_STATIC_SYMBOL_KEY(kiln_c99_symbol_key_async_dispose, "@@asyncDispose");
#undef KILN_C99_STATIC_SYMBOL_KEY

#define KILN_C99_WELL_KNOWN(key, description, description_len) \
    {&key, description, description_len, {KILN_VALUE_UNDEFINED, 0, {0}}, 0}
static const kiln_symbol kiln_c99_well_known[] = {
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_iterator, "Symbol.iterator", 15),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_async_iterator, "Symbol.asyncIterator", 20),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_has_instance, "Symbol.hasInstance", 18),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_to_string_tag, "Symbol.toStringTag", 18),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_to_primitive, "Symbol.toPrimitive", 18),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_is_concat_spreadable, "Symbol.isConcatSpreadable", 25),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_match, "Symbol.match", 12),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_match_all, "Symbol.matchAll", 15),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_replace, "Symbol.replace", 14),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_search, "Symbol.search", 13),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_species, "Symbol.species", 14),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_split, "Symbol.split", 12),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_unscopables, "Symbol.unscopables", 18),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_dispose, "Symbol.dispose", 14),
    KILN_C99_WELL_KNOWN(kiln_c99_symbol_key_async_dispose, "Symbol.asyncDispose", 19),
};
#undef KILN_C99_WELL_KNOWN

/* Every symbol this run has made, by the value that *is* it.
 *
 * A symbol-keyed property stores only the reserved key, so nothing on the object can hand the
 * symbol back -- and `Reflect.ownKeys` has to answer with the same symbol, by identity, not a
 * fresh one carrying the same description. The whole `kiln_value` is kept rather than the
 * `kiln_symbol *` inside it, because `Symbol.for` and `Symbol()` fill `bits` differently and
 * equality reads what was stored.
 *
 * Held for the life of the realm, because the last reference to a symbol can legitimately be
 * the property key it is stored under: `o[Symbol("k")] = 1` keeps nothing else, and the key must
 * still name a symbol afterwards. The registry is the realm's (`kiln_c99_realm_state.symbols`):
 * marked by its heap and freed with it. */
static int kiln_c99_remember_symbol(kiln_value symbol) {
    kiln_c99_realm_state *realm = kiln_c99_realm();
    if (realm->symbol_count == realm->symbol_capacity) {
        size_t wanted = realm->symbol_capacity == 0 ? 8 : realm->symbol_capacity * 2;
        kiln_value *grown =
            (kiln_value *)realloc(realm->symbols, wanted * sizeof *realm->symbols);
        if (grown == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        realm->symbols = grown;
        realm->symbol_capacity = wanted;
    }
    realm->symbols[realm->symbol_count++] = symbol;
    return KILN_C99_OK;
}

/* The symbol a reserved store key names, reporting whether the key names one at all.
 *
 * Both halves are the same question. A symbol key and an internal slot are both `@@` strings
 * here -- `@@iterator` beside `@@error`, `@@symbol:3` beside `@@iterpart` -- and reflection has
 * to report the first kind and must never report the second. This is the one place that line is
 * drawn on this side. */
static int kiln_c99_symbol_of_key_view(const kiln_string *key, kiln_value *out) {
    size_t index;
    size_t known = sizeof kiln_c99_well_known / sizeof *kiln_c99_well_known;
    if (key == NULL || out == NULL || !kiln_c99_key_is_symbol(key)) return 0;
    for (index = 0; index < known; ++index) {
        if (kiln_string_equal(kiln_c99_well_known[index].key, key)) {
            *out = kiln_c99_value(KILN_VALUE_SYMBOL,
                                  (uint64_t)(uintptr_t)&kiln_c99_well_known[index],
                                  (const char *)(uintptr_t)&kiln_c99_well_known[index], 0);
            return 1;
        }
    }
    {
        const kiln_c99_realm_state *realm = kiln_c99_realm();
        for (index = 0; index < realm->symbol_count; ++index) {
            const kiln_symbol *symbol = kiln_c99_symbol_of(realm->symbols[index]);
            if (kiln_string_equal(symbol->key, key)) {
                *out = realm->symbols[index];
                return 1;
            }
        }
    }
    return 0;
}

/* Fixed C names still arrive as bytes. This adapter is intentionally one-way: it builds a
 * borrowed Latin-1 view and immediately delegates to the unit comparison above. */
int kiln_c99_symbol_of_key(const char *key, size_t key_len, kiln_value *out) {
    kiln_string view;
    if (key_len > UINT32_MAX) return 0;
    view.unit_len = (uint32_t)key_len;
    view.flags = kiln_c99_fixed_key_flags(key, key_len);
    view.data = key;
    return kiln_c99_symbol_of_key_view(&view, out);
}

/* What the program would see as this property's `enumerable`. For a string key that is the stored
 * bit. For a symbol key the stored bit is held at 0 so the walks over own string keys skip the
 * `@@` spelling, and the program's own answer lives in `symbol_hidden` -- see `kiln_property`. */
static int kiln_c99_property_is_enumerable(const kiln_property *property) {
    kiln_value symbol;
    if (kiln_c99_symbol_of_key_view(property->key, &symbol)) {
        return !property->symbol_hidden;
    }
    return property->enumerable;
}

/* The string a value names a property under.
 *
 * Distinct from `kiln_c99_text`: a symbol keys a property under its reserved slot, not under its
 * description, and conflating the two would make `obj[Symbol.iterator]` and
 * `obj["Symbol(Symbol.iterator)"]` the same property. */
static kiln_value kiln_c99_property_key(kiln_value key) {
    if (key.tag == KILN_VALUE_SYMBOL) {
        kiln_symbol *symbol = kiln_c99_symbol_of(key);
        return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)symbol->key,
                              symbol->key->unit_len);
    }
    return kiln_c99_text(key);
}

/* The canonical key (ADR 0047) a symbol names a property under, or NULL when the key table cannot
 * grow. A symbol the program made carries its canonical key already -- `Symbol()` pins a
 * realm-local one, `Symbol.for` a shared one -- so the property paths skip the hash and probe
 * `kiln_c99_intern_key` would spend finding the pointer they were handed (realms issue 17). A
 * well-known symbol's key is a static header the prelude spelt; its canonical pointer is the
 * shared key `kiln_c99_register_builtin_keys` made of that spelling before the first realm, looked
 * up once per process and kept beside the table. Well-known symbols are the ones with a byte
 * description; a program's keep theirs as a value. */
static const kiln_string *kiln_c99_well_known_ikeys[sizeof kiln_c99_well_known /
                                                    sizeof *kiln_c99_well_known];

static __attribute__((noinline)) const kiln_string *kiln_c99_well_known_ikey(
    const kiln_symbol *symbol) {
    size_t known = sizeof kiln_c99_well_known / sizeof *kiln_c99_well_known;
    uintptr_t first = (uintptr_t)&kiln_c99_well_known[0];
    uintptr_t at = (uintptr_t)symbol;
    const kiln_string *ikey;
    if (at >= first && at < (uintptr_t)&kiln_c99_well_known[known]) {
        size_t index = (at - first) / sizeof *kiln_c99_well_known;
        ikey = __atomic_load_n(&kiln_c99_well_known_ikeys[index], __ATOMIC_ACQUIRE);
        if (ikey != NULL) return ikey;
        ikey = kiln_c99_intern_key(symbol->key);
        /* One word, the same in every realm: only a shared key may be kept process-wide. */
        if (ikey != NULL && kiln_c99_key_is_shared(ikey))
            __atomic_store_n(&kiln_c99_well_known_ikeys[index], ikey, __ATOMIC_RELEASE);
        return ikey;
    }
    return kiln_c99_intern_key(symbol->key);
}

static inline __attribute__((always_inline)) const kiln_string *kiln_c99_symbol_ikey(const kiln_symbol *symbol) {
    if (KILN_LIKELY(symbol->description == NULL)) return symbol->key;
    return kiln_c99_well_known_ikey(symbol);
}

/* A computed key's canonical pointer when it is a symbol, else NULL: the `ikey` argument of the
 * headed property paths, which intern the spelling themselves when handed NULL. */
static const kiln_string *kiln_c99_property_ikey(kiln_value key) {
    return key.tag == KILN_VALUE_SYMBOL ? kiln_c99_symbol_ikey(kiln_c99_symbol_of(key)) : NULL;
}

/* `Symbol(description)`. Two calls make two different symbols even given the same description,
 * so the identity is the allocation's and the reserved key carries a serial. */
static int kiln_c99_new_symbol(kiln_value *out, kiln_value description) {
    /* Process-wide, so two realms never mint one serial twice into either's keys. */
    static uint64_t kiln_c99_symbol_serial;
    uint64_t serial;
    kiln_symbol *symbol = (kiln_symbol *)kiln_c99_gc_alloc(KILN_GC_SYMBOL, sizeof *symbol);
    kiln_text key = {NULL, 0, 0};
    char digits[32];
    size_t written;
    if (symbol == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    symbol->description = NULL;
    symbol->description_len = 0;
    symbol->description_value = kiln_c99_undefined();
    serial = __atomic_add_fetch(&kiln_c99_symbol_serial, 1u, __ATOMIC_RELAXED);
    written = kiln_c99_double_to_js((double)serial, digits);
    if (kiln_c99_text_append_c(&key, "@@symbol:") != KILN_C99_OK ||
        kiln_c99_text_append(&key, digits, written) != KILN_C99_OK) {
        free(key.bytes);
        kiln_c99_gc_release(symbol);
        return KILN_C99_OUT_OF_MEMORY;
    }
    /* The reserved key stays malloc-backed and is never released: a symbol lives as long as the
     * realm does (the realm's symbol registry roots every one of them), and the key bytes are borrowed by
     * every property stored under this symbol. The tracer still marks both `key` and
     * `description` as candidate blocks, so a description that came from a collected string --
     * `Symbol(String(x))` -- is kept alive by the symbol that carries it. */
    /* Registered as immortal: the interned copy is pinned for the realm's life, and a property
     * stored under the symbol carries that canonical pointer. The builder's malloc has done its
     * job once the copy exists. */
    {
        kiln_string view;
        if (key.len > UINT32_MAX) {
            free(key.bytes);
            kiln_c99_gc_release(symbol);
            return KILN_C99_OUT_OF_MEMORY;
        }
        view.unit_len = (uint32_t)key.len;
        view.flags = KILN_STRING_LATIN1 | KILN_STRING_SYMBOL_KEY;
        view.data = key.bytes;
        symbol->key = kiln_c99_intern_pinned_key(&view);
        if (symbol->key == NULL) {
            free(key.bytes);
            kiln_c99_gc_release(symbol);
            return KILN_C99_OUT_OF_MEMORY;
        }
        free(key.bytes);
        key.bytes = NULL;
    }
    if (description.tag == KILN_VALUE_UNDEFINED) {
        /* `Symbol()` has no description, which prints as `Symbol()` rather than
         * `Symbol(undefined)`. */
        symbol->description = NULL;
        symbol->description_len = 0;
        symbol->description_value = kiln_c99_undefined();
    } else {
        kiln_value text = kiln_c99_text(description);
        if (text.tag != KILN_VALUE_STRING) {
            free(key.bytes);
            kiln_c99_gc_release(symbol);
            return KILN_C99_OUT_OF_MEMORY;
        }
        symbol->description = NULL;
        symbol->description_len = 0;
        symbol->description_value = text;
    }
    symbol->registry_id = 0;
    *out = kiln_c99_value(KILN_VALUE_SYMBOL, 0, (const char *)(uintptr_t)symbol, 0);
    return kiln_c99_remember_symbol(*out);
}

/* Provider strings cross the C boundary as the same header used by the heap. Copying them to a
 * value is deliberately unit based; UTF-8 is never used as a registry identity. */
static kiln_value kiln_c99_copy_symbol_key(const kiln_string *source) {
    uint16_t *units;
    kiln_value result;
    size_t index;
    if (source == NULL || source->unit_len > SIZE_MAX / sizeof *units) {
        return kiln_c99_undefined();
    }
    units = source->unit_len == 0
                ? NULL
                : (uint16_t *)malloc((size_t)source->unit_len * sizeof *units);
    if (source->unit_len != 0 && units == NULL) return kiln_c99_undefined();
    for (index = 0; index < source->unit_len; ++index) {
        units[index] = (uint16_t)kiln_string_unit_at(source, index);
    }
    result = kiln_c99_from_units(units, source->unit_len);
    free(units);
    return result;
}

static int kiln_c99_symbol_for(kiln_value *out, kiln_value key) {
    kiln_value text = kiln_c99_text(key);
    const kiln_string *text_key;
    kiln_string registered_key = {0, KILN_STRING_LATIN1, NULL};
    kiln_value description;
    kiln_symbol *symbol;
    uint16_t *property_units;
    kiln_string property_view;
    static const char property_prefix[] = "@@symbol-for:";
    size_t prefix_len = sizeof property_prefix - 1;
    uint64_t id;
    size_t property_len;
    size_t index;
    if (text.tag != KILN_VALUE_STRING || text.ptr == NULL) return KILN_C99_OUT_OF_MEMORY;
    text_key = (const kiln_string *)text.ptr;
    if (kiln_realm_symbol_for(text_key, &id) != KILN_C99_OK) {
        KILN_C99_REFUSE("the realm would not register this symbol key");
    }
    if (kiln_realm_symbol_key_for(id, &registered_key) != KILN_C99_OK ||
        (registered_key.flags != KILN_STRING_LATIN1 && registered_key.flags != KILN_STRING_UTF16)) {
        KILN_C99_REFUSE("the realm would not answer with the symbol key it registered");
    }
    /* Copy the provider answer before any later allocation can change the active realm scratch. */
    description = kiln_c99_copy_symbol_key(&registered_key);
    if (description.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    /* The property key is spelt by the registry key itself, not by the realm's registry id: the
     * registry is one per realm and so are its ids, but `Symbol.for(k)` is the same symbol in
     * every realm, and a key spelt the same in every realm is a shared key (ADR 0071) -- its
     * shapes and the caches naming them serve every realm, as a constant name's do. */
    if (registered_key.unit_len > UINT32_MAX - prefix_len) return KILN_C99_OUT_OF_MEMORY;
    property_len = prefix_len + registered_key.unit_len;
    property_units = (uint16_t *)malloc(property_len * sizeof *property_units);
    if (property_units == NULL) return KILN_C99_OUT_OF_MEMORY;
    for (index = 0; index < prefix_len; ++index)
        property_units[index] = (unsigned char)property_prefix[index];
    {
        /* `description` is the registered key's copy; the provider's header may not outlive the
         * allocations below. */
        const kiln_string *copied = (const kiln_string *)description.ptr;
        for (index = 0; index < copied->unit_len; ++index)
            property_units[prefix_len + index] = (uint16_t)kiln_string_unit_at(copied, index);
        property_len = prefix_len + copied->unit_len;
    }
    {
        int wide = 0;
        for (index = prefix_len; index < property_len; ++index) wide |= property_units[index] > 0xFF;
        if (wide) {
            property_view.flags = KILN_STRING_UTF16 | KILN_STRING_SYMBOL_KEY;
            property_view.data = property_units;
        } else {
            /* Narrowed in place: a Latin-1 spelling is the canonical form of a narrow key. */
            unsigned char *narrow = (unsigned char *)property_units;
            for (index = 0; index < property_len; ++index) narrow[index] = (unsigned char)property_units[index];
            property_view.flags = KILN_STRING_LATIN1 | KILN_STRING_SYMBOL_KEY;
            property_view.data = narrow;
        }
        property_view.unit_len = (uint32_t)property_len;
    }
    symbol = (kiln_symbol *)kiln_c99_gc_alloc(KILN_GC_SYMBOL, sizeof *symbol);
    if (symbol == NULL) {
        free(property_units);
        return KILN_C99_OUT_OF_MEMORY;
    }
    symbol->key = kiln_c99_intern_shared_key(&property_view);
    free(property_units);
    if (symbol->key == NULL) {
        kiln_c99_gc_release(symbol);
        return KILN_C99_OUT_OF_MEMORY;
    }
    symbol->description = NULL;
    symbol->description_len = 0;
    symbol->description_value = description;
    symbol->registry_id = id;
    *out = kiln_c99_value(KILN_VALUE_SYMBOL, 0, (const char *)(uintptr_t)symbol, 0);
    return kiln_c99_remember_symbol(*out);
}

static kiln_value kiln_c99_symbol_key_for(kiln_value value) {
    kiln_string key = {0, KILN_STRING_LATIN1, NULL};
    if (value.tag != KILN_VALUE_SYMBOL || kiln_c99_symbol_of(value)->registry_id == 0) {
        return kiln_c99_undefined();
    }
    if (kiln_realm_symbol_key_for(kiln_c99_symbol_of(value)->registry_id, &key) != KILN_C99_OK ||
        (key.flags != KILN_STRING_LATIN1 && key.flags != KILN_STRING_UTF16)) {
        return kiln_c99_undefined();
    }
    return kiln_c99_copy_symbol_key(&key);
}

/* `call`, `apply`, `bind` and `toString`, which every callable answers to whatever kind of
 * callable it is. Not a range lookup, because the four are not contiguous with anything and a
 * function's *own* property of the same name has to win -- so each caller asks after its own
 * store.
 *
 * `toString` here is `Function.prototype.toString`, which is why it belongs beside the other
 * three rather than in the `Object.prototype` fallback below -- that one would answer
 * `[object Function]` for `String(Object)`, and refusing was rightly preferred to saying that.
 * This answers `function Object() { [native code] }`, which is what the interpreter says and what
 * node says. Without it a program reading `f.toString()` found no property at all and got
 * `undefined is not a function`: a wrong answer where `String(f)` on the very same value had been
 * right all along, because that path goes through `kiln_c99_text` instead. */
static uint64_t kiln_c99_callable_method(const char *key, size_t key_len) {
    if (kiln_c99_same_name(key, key_len, "call")) {
        return KILN_C99_METHOD_FUNCTION_CALL;
    }
    if (kiln_c99_same_name(key, key_len, "apply")) {
        return KILN_C99_METHOD_FUNCTION_APPLY;
    }
    if (kiln_c99_same_name(key, key_len, "bind")) {
        return KILN_C99_METHOD_FUNCTION_BIND;
    }
    if (kiln_c99_same_name(key, key_len, "toString")) {
        return KILN_C99_METHOD_FUNCTION_TO_STRING;
    }
    return 0;
}

/* A namespace is an object holding functions rather than a callable, which decides both how it
 * prints and what `typeof` answers about it. */
static int kiln_c99_is_namespace(uint64_t builtin) {
    /* `Temporal` and `Temporal.Now` hold functions rather than being callable; the eight types are
     * constructors and are not namespaces. */
    if (builtin == KILN_C99_BUILTIN_TEMPORAL || builtin == KILN_C99_BUILTIN_TEMPORAL_NOW) {
        return 1;
    }
    /* `Intl` is a namespace this backend does not carry: `typeof Intl === "object"` is what a
     * library feature-detects on, and it has to be the same word here as in the interpreter even
     * though every member behind it refuses. */
    if (builtin == KILN_C99_BUILTIN_INTL) {
        return 1;
    }
    /* `performance` is a `Performance` *object* holding `now`, not something a program calls --
     * node answers `typeof performance === "object"` and gives it no `name`. It was a callable
     * here, so `typeof performance` read "function" and `performance.name` read "performance",
     * which is an implementation detail leaking through a property a program reads to tell one
     * function from another. `require("node:perf_hooks").performance` is this same value, so the
     * two spellings are one object as they are in node. */
    if (builtin == KILN_C99_BUILTIN_PERFORMANCE) {
        return 1;
    }
    /* `crypto` the same way: an object holding functions, none of which this backend answers
     * until `node:crypto` is linked. */
    if (builtin == KILN_C99_BUILTIN_CRYPTO) {
        return 1;
    }
    return builtin == KILN_C99_BUILTIN_MATH || builtin == KILN_C99_BUILTIN_JSON ||
           builtin == KILN_C99_BUILTIN_REFLECT ||
           builtin == KILN_C99_BUILTIN_OBJECT_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_ARRAY_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_ERROR_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_AGGREGATE_ERROR_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_MAP_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_SET_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_STRING_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_ITERATOR_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_REGEXP_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_DATE_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_NUMBER_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_BIGINT_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_FUNCTION_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_PROMISE_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE ||
           /* `DataView.prototype` is a table, and therefore a namespace here; `DataView` itself
            * is a constructor and is not, so `typeof DataView === "function"` stays true. */
           builtin == KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_SYMBOL_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_BIG_INT64_ARRAY_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_BIG_UINT64_ARRAY_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER_PROTOTYPE ||
           (builtin >= KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE &&
            builtin <= KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE);
}

/* The method a name resolves to on a given receiver, or 0.  One generated code list means a read
 * and a call cannot disagree about which names exist, and category membership is independent of
 * numeric adjacency. */
static const uint16_t kiln_c99_method_codes[] = {
