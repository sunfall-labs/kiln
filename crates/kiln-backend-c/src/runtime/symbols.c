
static kiln_symbol *kiln_c99_symbol_of(kiln_value value) {
    return (kiln_symbol *)(uintptr_t)value.ptr;
}

/* The specification's own symbols. One instance each, so equality is the pointer's exactly as it
 * is for a unique symbol -- there is no second kind of comparison to keep in step. */
static const kiln_symbol kiln_c99_well_known[] = {
    {"@@iterator", 10, "Symbol.iterator", 15},
    {"@@asyncIterator", 15, "Symbol.asyncIterator", 20},
    {"@@hasInstance", 13, "Symbol.hasInstance", 18},
    {"@@toStringTag", 13, "Symbol.toStringTag", 18},
    {"@@toPrimitive", 13, "Symbol.toPrimitive", 18},
    {"@@isConcatSpreadable", 20, "Symbol.isConcatSpreadable", 25},
    {"@@match", 7, "Symbol.match", 12},
    {"@@matchAll", 10, "Symbol.matchAll", 15},
    {"@@replace", 9, "Symbol.replace", 14},
    {"@@search", 8, "Symbol.search", 13},
    {"@@species", 9, "Symbol.species", 14},
    {"@@split", 7, "Symbol.split", 12},
    {"@@unscopables", 13, "Symbol.unscopables", 18}};

/* Every symbol this run has made, by the value that *is* it.
 *
 * A symbol-keyed property stores only the reserved key, so nothing on the object can hand the
 * symbol back -- and `Reflect.ownKeys` has to answer with the same symbol, by identity, not a
 * fresh one carrying the same description. The whole `kiln_value` is kept rather than the
 * `kiln_symbol *` inside it, because `Symbol.for` and `Symbol()` fill `bits` differently and
 * equality reads what was stored.
 *
 * Held for the life of the process, because the last reference to a symbol can legitimately be
 * the property key it is stored under: `o[Symbol("k")] = 1` keeps nothing else, and the key must
 * still name a symbol afterwards. */
static kiln_value *kiln_c99_symbols;
static size_t kiln_c99_symbol_count;
static size_t kiln_c99_symbol_capacity;

static int kiln_c99_remember_symbol(kiln_value symbol) {
    if (kiln_c99_symbol_count == kiln_c99_symbol_capacity) {
        size_t wanted = kiln_c99_symbol_capacity == 0 ? 8 : kiln_c99_symbol_capacity * 2;
        kiln_value *grown =
            (kiln_value *)realloc(kiln_c99_symbols, wanted * sizeof *kiln_c99_symbols);
        if (grown == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_symbols = grown;
        kiln_c99_symbol_capacity = wanted;
    }
    kiln_c99_symbols[kiln_c99_symbol_count++] = symbol;
    return KILN_C99_OK;
}

/* The symbol a reserved store key names, reporting whether the key names one at all.
 *
 * Both halves are the same question. A symbol key and an internal slot are both `@@` strings
 * here -- `@@iterator` beside `@@error`, `@@symbol:3` beside `@@iterpart` -- and reflection has
 * to report the first kind and must never report the second. This is the one place that line is
 * drawn on this side. */
static int kiln_c99_symbol_of_key(const char *key, size_t key_len, kiln_value *out) {
    size_t index;
    size_t known = sizeof kiln_c99_well_known / sizeof *kiln_c99_well_known;
    for (index = 0; index < known; ++index) {
        if (kiln_c99_well_known[index].key_len == key_len &&
            memcmp(kiln_c99_well_known[index].key, key, key_len) == 0) {
            *out = kiln_c99_value(KILN_VALUE_SYMBOL,
                                  (uint64_t)(uintptr_t)&kiln_c99_well_known[index],
                                  (const char *)(uintptr_t)&kiln_c99_well_known[index], 0);
            return 1;
        }
    }
    for (index = 0; index < kiln_c99_symbol_count; ++index) {
        const kiln_symbol *symbol = kiln_c99_symbol_of(kiln_c99_symbols[index]);
        if (symbol->key_len == key_len && memcmp(symbol->key, key, key_len) == 0) {
            *out = kiln_c99_symbols[index];
            return 1;
        }
    }
    return 0;
}

/* What the program would see as this property's `enumerable`. For a string key that is the stored
 * bit. For a symbol key the stored bit is held at 0 so the walks over own string keys skip the
 * `@@` spelling, and the program's own answer lives in `symbol_hidden` -- see `kiln_property`. */
static int kiln_c99_property_is_enumerable(const kiln_property *property) {
    kiln_value symbol;
    if (kiln_c99_symbol_of_key(property->key, property->key_len, &symbol)) {
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
        return kiln_c99_value(KILN_VALUE_STRING, 0, symbol->key, symbol->key_len);
    }
    return kiln_c99_text(key);
}

/* `Symbol(description)`. Two calls make two different symbols even given the same description,
 * so the identity is the allocation's and the reserved key carries a serial. */
static int kiln_c99_new_symbol(kiln_value *out, kiln_value description) {
    static uint64_t next_serial;
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
    next_serial += 1;
    written = kiln_c99_double_to_js((double)next_serial, digits);
    if (kiln_c99_text_append_c(&key, "@@symbol:") != KILN_C99_OK ||
        kiln_c99_text_append(&key, digits, written) != KILN_C99_OK) {
        free(key.bytes);
        kiln_c99_gc_release(symbol);
        return KILN_C99_OUT_OF_MEMORY;
    }
    /* The reserved key stays malloc-backed and is never released: a symbol lives as long as the
     * realm does (`kiln_c99_symbols` roots every one of them), and the key bytes are borrowed by
     * every property stored under this symbol. The tracer still marks both `key` and
     * `description` as candidate blocks, so a description that came from a collected string --
     * `Symbol(String(x))` -- is kept alive by the symbol that carries it. */
    symbol->key = key.bytes;
    symbol->key_len = key.len;
    /* Registered as immortal: the interned copy is pinned for the realm's life, and a property
     * stored under the symbol carries that canonical pointer. The builder's malloc has done its
     * job once the copy exists. */
    {
        const char *canonical = kiln_c99_intern_static(symbol->key, symbol->key_len);
        if (canonical == NULL) {
            free(key.bytes);
            kiln_c99_gc_release(symbol);
            return KILN_C99_OUT_OF_MEMORY;
        }
        symbol->key = canonical;
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

static int kiln_c99_symbol_for(kiln_value *out, kiln_value key) {
    kiln_value text = kiln_c99_text(key);
    uint8_t *encoded = NULL;
    size_t encoded_len = 0;
    kiln_symbol *symbol;
    const uint8_t *registered_key;
    uint64_t registered_key_len;
    char *property_key;
    static const char property_prefix[] = "@@symbol-for:";
    uint64_t id;
    if (text.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    if (kiln_c99_string_to_utf8_lossy(text, &encoded, &encoded_len) != KILN_C99_OK) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    if (kiln_realm_symbol_for(encoded, (uint64_t)encoded_len, &id) != KILN_C99_OK) {
        free(encoded);
        KILN_C99_REFUSE("the realm would not register this symbol key");
    }
    free(encoded);
    if (kiln_realm_symbol_key_for(id, &registered_key, &registered_key_len) != KILN_C99_OK ||
        registered_key_len > SIZE_MAX)
        KILN_C99_REFUSE("the realm would not answer with the symbol key it registered");
    symbol = (kiln_symbol *)kiln_c99_gc_alloc(KILN_GC_SYMBOL, sizeof *symbol);
    if (symbol == NULL) return KILN_C99_OUT_OF_MEMORY;
    symbol->description = NULL;
    symbol->description_len = 0;
    symbol->description_value = kiln_c99_undefined();
    /* Registry key bytes, like `Symbol()`'s, stay malloc-backed and outlive the realm's heap:
     * the symbol they belong to is rooted for the realm's whole life. */
    property_key = (char *)malloc((sizeof property_prefix - 1) + sizeof id);
    if (property_key == NULL) {
        kiln_c99_gc_release(symbol);
        return KILN_C99_OUT_OF_MEMORY;
    }
    memcpy(property_key, property_prefix, sizeof property_prefix - 1);
    memcpy(property_key + sizeof property_prefix - 1, &id, sizeof id);
    symbol->key = property_key;
    symbol->key_len = (sizeof property_prefix - 1) + sizeof id;
    /* Two `Symbol.for("x")` calls make two of these structs with one key: interning is what
     * makes a property stored through either findable through the other by pointer. The key
     * carries the id's raw bytes, NULs included, which is one reason a key's length is read
     * from its header and never measured. */
    {
        const char *canonical = kiln_c99_intern_static(symbol->key, symbol->key_len);
        free(property_key);
        if (canonical == NULL) {
            kiln_c99_gc_release(symbol);
            return KILN_C99_OUT_OF_MEMORY;
        }
        symbol->key = canonical;
    }
    symbol->description = NULL;
    symbol->description_len = 0;
    symbol->description_value = kiln_c99_copy_text((const char *)registered_key,
                                                   (size_t)registered_key_len);
    if (symbol->description_value.tag != KILN_VALUE_STRING) {
        kiln_c99_gc_release(symbol);
        return KILN_C99_OUT_OF_MEMORY;
    }
    symbol->registry_id = id;
    *out = kiln_c99_value(KILN_VALUE_SYMBOL, 0, (const char *)(uintptr_t)symbol, 0);
    return kiln_c99_remember_symbol(*out);
}

static kiln_value kiln_c99_symbol_key_for(kiln_value value) {
    const uint8_t *key;
    uint64_t key_len;
    if (value.tag != KILN_VALUE_SYMBOL) return kiln_c99_undefined();
    if (kiln_realm_symbol_key_for(kiln_c99_symbol_of(value)->registry_id, &key, &key_len) !=
        KILN_C99_OK)
        return kiln_c99_undefined();
    return kiln_c99_copy_text((const char *)key, (size_t)key_len);
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
           builtin == KILN_C99_BUILTIN_FUNCTION_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_PROMISE_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE ||
           /* `DataView.prototype` is a table, and therefore a namespace here; `DataView` itself
            * is a constructor and is not, so `typeof DataView === "function"` stays true. */
           builtin == KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_SYMBOL_PROTOTYPE ||
           (builtin >= KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE &&
            builtin <= KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE);
}

/* The method a name resolves to on a given receiver, or 0.  One generated code list means a read
 * and a call cannot disagree about which names exist, and category membership is independent of
 * numeric adjacency. */
static const uint16_t kiln_c99_method_codes[] = {
