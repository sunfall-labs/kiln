};

static void *kiln_c99_node_hidden_pointer(kiln_value object, const char *key, size_t key_len);
static int kiln_c99_val_call(kiln_host *host, kiln_val callee, const kiln_val *args, size_t argc,
                             int depth, kiln_value *thrown, const char *inst_site, size_t inst_idx,
                             const char *inst_op, kiln_val *out);
int kiln_c99_get_array_unscopables(kiln_value *out);

static const char *kiln_c99_builtin_owner_name(uint64_t builtin) {
    switch (builtin) {
    case KILN_C99_BUILTIN_SYMBOL_PROTOTYPE: return "Symbol.prototype";
    case KILN_C99_BUILTIN_ARRAY_PROTOTYPE: return "Array.prototype";
    case KILN_C99_BUILTIN_NUMBER_PROTOTYPE: return "Number.prototype";
    case KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE: return "Boolean.prototype";
    case KILN_C99_BUILTIN_BIGINT_PROTOTYPE: return "BigInt.prototype";
    case KILN_C99_BUILTIN_PROMISE_PROTOTYPE: return "Promise.prototype";
    case KILN_C99_BUILTIN_OBJECT_PROTOTYPE: return "Object.prototype";
    case KILN_C99_BUILTIN_FUNCTION_PROTOTYPE: return "Function.prototype";
    case KILN_C99_BUILTIN_MAP_PROTOTYPE: return "Map.prototype";
    case KILN_C99_BUILTIN_SET_PROTOTYPE: return "Set.prototype";
    case KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE: return "WeakMap.prototype";
    case KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE: return "WeakSet.prototype";
    case KILN_C99_BUILTIN_DATE_PROTOTYPE: return "Date.prototype";
    default: return kiln_c99_builtin_name(builtin);
    }
}

static int kiln_c99_backlog_names(const char *owner, const char *key, size_t key_len) {
    size_t index;
    if (owner == NULL) {
        return 0;
    }
    for (index = 0; index < sizeof kiln_c99_builtin_backlog / sizeof *kiln_c99_builtin_backlog;
         ++index) {
        if (strcmp(kiln_c99_builtin_backlog[index].owner, owner) == 0 &&
            kiln_c99_same_name(key, key_len, kiln_c99_builtin_backlog[index].member)) {
            return 1;
        }
    }
    return 0;
}

/* Whether this built-in's surface is table-driven at all.
 *
 * The ones that are not -- the prototype method tables, `Temporal`'s eight types, the typed-array
 * constructors -- keep their own lookups below, and the drift test names them as backlog rather
 * than skipping them. A built-in that *is* table-driven has no case in the switch below at all:
 * adding a row is the whole of adding a member to it. */
static int kiln_c99_has_surface(uint64_t builtin) {
    size_t index;
    for (index = 0; index < KILN_C99_SURFACE_MEMBER_COUNT; ++index) {
        if (kiln_c99_builtin_members[index].builtin == builtin) {
            return 1;
        }
    }
    return 0;
}

/* The row `builtin` holds for `key`, reporting through `owned` whether the built-in is
 * table-driven at all.
 *
 * The two answers come out of one pass on purpose. This sits on the read path of every property
 * access whose receiver is a built-in -- `Math.floor` inside a loop reaches it once per iteration
 * -- and asking `kiln_c99_has_surface` first walked the whole table a second time to learn
 * something this walk already knows. */
/* The surface table, indexed for the read path. The table is one linear list of every member
 * of every table-driven built-in, and a read of `Promise.resolve` or a miss on `Math.foo`
 * walked all of it, string compare by string compare -- the single hottest leaf of a promise
 * loop. The index is the same rows sorted by (built-in, name, row), built once per process;
 * a lookup is one binary search, and the first row of a run of equal names is the row the
 * walk found first, so the answer is the same. */
typedef struct kiln_builtin_member_entry {
    uint16_t builtin;
    size_t len;
    const char *name;
    size_t row;
} kiln_builtin_member_entry;

static kiln_builtin_member_entry kiln_c99_builtin_member_index[KILN_C99_SURFACE_MEMBER_COUNT];
static size_t kiln_c99_builtin_member_index_len;
static int kiln_c99_builtin_member_index_ready;

static int kiln_c99_builtin_member_entry_before(const kiln_builtin_member_entry *left,
                                                const kiln_builtin_member_entry *right) {
    size_t shortest;
    int ordering;
    if (left->builtin != right->builtin) return left->builtin < right->builtin;
    shortest = left->len < right->len ? left->len : right->len;
    ordering = memcmp(left->name, right->name, shortest);
    if (ordering != 0) return ordering < 0;
    if (left->len != right->len) return left->len < right->len;
    return left->row < right->row;
}

static void kiln_c99_builtin_member_index_build(void) {
    size_t index;
    for (index = 0; index < KILN_C99_SURFACE_MEMBER_COUNT; ++index) {
        const kiln_builtin_member *member = &kiln_c99_builtin_members[index];
        kiln_c99_builtin_member_index[index].builtin = member->builtin;
        kiln_c99_builtin_member_index[index].name = member->name;
        kiln_c99_builtin_member_index[index].len = member->name != NULL ? strlen(member->name) : 0;
        kiln_c99_builtin_member_index[index].row = index;
    }
    for (index = 1; index < KILN_C99_SURFACE_MEMBER_COUNT; ++index) {
        kiln_builtin_member_entry pending = kiln_c99_builtin_member_index[index];
        size_t behind = index;
        while (behind > 0 &&
               kiln_c99_builtin_member_entry_before(&pending, &kiln_c99_builtin_member_index[behind - 1])) {
            kiln_c99_builtin_member_index[behind] = kiln_c99_builtin_member_index[behind - 1];
            --behind;
        }
        kiln_c99_builtin_member_index[behind] = pending;
    }
    kiln_c99_builtin_member_index_len = KILN_C99_SURFACE_MEMBER_COUNT;
    kiln_c99_builtin_member_index_ready = 1;
}

/* The first index entry at or after (builtin, key): the lower bound. */
static size_t kiln_c99_builtin_member_lower_bound(uint64_t builtin, const char *key, size_t key_len) {
    size_t low = 0;
    size_t high = kiln_c99_builtin_member_index_len;
    while (low < high) {
        size_t middle = low + (high - low) / 2;
        const kiln_builtin_member_entry *entry = &kiln_c99_builtin_member_index[middle];
        int before;
        if (entry->builtin != builtin) {
            before = entry->builtin < builtin;
        } else {
            size_t shortest = entry->len < key_len ? entry->len : key_len;
            int ordering = memcmp(entry->name, key, shortest);
            if (ordering == 0 && entry->len != key_len) ordering = entry->len < key_len ? -1 : 1;
            before = ordering < 0;
        }
        if (before) low = middle + 1; else high = middle;
    }
    return low;
}

static const kiln_builtin_member *kiln_c99_builtin_member(uint64_t builtin, const char *key,
                                                          size_t key_len, int *owned) {
    size_t at;
    *owned = 0;
    if (builtin > 0xFFFFu) return NULL;
    if (!kiln_c99_builtin_member_index_ready) kiln_c99_builtin_member_index_build();
    /* Owned: any row with this built-in, which is the lower bound of (builtin, "") landing on one. */
    at = kiln_c99_builtin_member_lower_bound(builtin, "", 0);
    if (at >= kiln_c99_builtin_member_index_len || kiln_c99_builtin_member_index[at].builtin != builtin) {
        return NULL;
    }
    *owned = 1;
    if (key == NULL) return NULL;
    at = kiln_c99_builtin_member_lower_bound(builtin, key, key_len);
    if (at < kiln_c99_builtin_member_index_len) {
        const kiln_builtin_member_entry *entry = &kiln_c99_builtin_member_index[at];
        if (entry->builtin == builtin && entry->len == key_len &&
            memcmp(entry->name, key, key_len) == 0) {
            return &kiln_c99_builtin_members[entry->row];
        }
    }
    return NULL;
}

/* The method a table-driven built-in's member names, or 0.
 *
 * `owned` separates "this built-in is not table-driven, ask the switch" from "it is, and it has no
 * such method" -- which are the same 0 to a caller and must not be. */
static uint64_t kiln_c99_surface_method(uint64_t builtin, const char *key, size_t key_len,
                                        int *owned) {
    const kiln_builtin_member *member = kiln_c99_builtin_member(builtin, key, key_len, owned);
    return member != NULL && member->kind == KILN_C99_MEMBER_METHOD ? member->code : 0;
}

/* The name a table-driven method answers to, which is the member's own key in the row above.
 *
 * WHY THE ROW RATHER THAN A `case`. `X.member.name` must be `"member"`: node makes that true for
 * every static on every built-in, and it is the string a decorator preserves, a stack trace
 * prints and a container keys on. The name used to be written down twice -- once as the key here
 * and once as a `case` in `kiln_c99_method_name` -- and fifteen rows had the key and no case, so
 * `Symbol.for`, `ArrayBuffer.isView`, `Buffer.concat` and `AbortSignal.timeout` all reported the
 * empty string. Nothing caught it, because a wrong name refuses nothing: the read succeeds and
 * hands back a string, and only something comparing it to the key would ever know.
 *
 * Reading it out of the row makes the key the name, so a member that is reachable has a name by
 * construction and the two can no longer disagree. The scan is linear, which is fine here and
 * would not be on the read path: `kiln_c99_builtin_member` above is one pass for exactly that
 * reason, and a name is read by tooling rather than in a loop.
 *
 * A code serving several members reports the first row that names it -- the canonical one. That
 * is `Buffer.alloc` for the three `alloc` spellings, which node gives three functions and three
 * names; `kiln_builtin_surface::NAME_IS_SHARED` is where that debt is written down, and it is
 * what the guards in both engines read so an alias cannot be mistaken for a drift. */
static const char *kiln_c99_surface_method_name(uint64_t code) {
    size_t index;
    /* Zero is "no method", which every miss in the lookup helpers returns; it names nothing. */
    if (code == 0) {
        return NULL;
    }
    for (index = 0; index < KILN_C99_SURFACE_MEMBER_COUNT; ++index) {
        const kiln_builtin_member *member = &kiln_c99_builtin_members[index];
        if (member->kind == KILN_C99_MEMBER_METHOD && (uint64_t)member->code == code) {
            return member->name;
        }
    }
    return NULL;
}

/* A member whose answer is a value rather than a call: `Math.PI`, `Number.MAX_VALUE`,
 * `Symbol.iterator`, `Array[Symbol.species]`.  Returns whether there was one.
 *
 * Out of the same rows as the methods, because these used to be a separate list and that is
 * exactly how `Array[Symbol.species]` came to read `undefined` here where the interpreter answers
 * a function. No refusal, no diagnostic -- the program simply took the other branch, which is the
 * one wrong answer this backend is not allowed to give. */
static int kiln_c99_surface_value(uint64_t builtin, const char *key, size_t key_len,
                                  kiln_value *out) {
    int owned;
    const kiln_builtin_member *member = kiln_c99_builtin_member(builtin, key, key_len, &owned);
    if (member == NULL) {
        return 0;
    }
    switch (member->kind) {
    case KILN_C99_MEMBER_NUMBER:
        *out = kiln_c99_number(member->number);
        return 1;
    case KILN_C99_MEMBER_TEXT:
        *out = kiln_c99_copy_text(member->text, strlen(member->text));
        return 1;
    case KILN_C99_MEMBER_BUILTIN:
        *out = kiln_c99_builtin(member->code);
        return 1;
    case KILN_C99_MEMBER_SYMBOL:
        return kiln_c99_symbol_of_key(member->text, strlen(member->text), out);
    case KILN_C99_MEMBER_UNSCOPABLES:
        return kiln_c99_get_array_unscopables(out) == KILN_C99_OK;
    default:
        return 0;
    }
}

/* A namespace's own methods, by which namespace it is: `Math.min` and `Number.isNaN` are
 * different functions and the ranges keep them apart. */
static uint64_t kiln_c99_namespace_method(uint64_t builtin, const char *key, size_t key_len) {
    /* Every error constructor carries it, and `Error.captureStackTrace` is the spelling libraries
     * use, so it is answered ahead of the tables rather than repeated in a row per kind. `Error`
     * still has a row of its own, so enumeration and the drift test can both see the member. */
    if (kiln_c99_error_name(builtin) != NULL &&
        kiln_c99_same_name(key, key_len, "captureStackTrace")) {
        return KILN_C99_METHOD_ERROR_CAPTURE_STACK_TRACE;
    }
    /* The single-sourced surface first, and exclusively: a table-driven built-in has no case in
     * the switch below at all. Those cases were hand-written `kiln_c99_same_name` chains -- a
     * list a human had to remember to extend, one per built-in -- and forgetting to extend one
     * was the whole disease. What is left below are the prototype method tables, which are
     * dispatched by code range rather than by name. */
    {
        int owned;
        uint64_t code = kiln_c99_surface_method(builtin, key, key_len, &owned);
        if (owned) {
            return code;
        }
    }
    /* The prototype object answers `@@iterator` with the same function an *instance* of it hands
     * out. It did not, and a `@@` key that no receiver answers is turned into `undefined` one
     * frame up rather than refused -- so `String.prototype[Symbol.iterator]` read `undefined` here
     * against the interpreter's function, silently, which is how a polyfill decides the realm has
     * no iteration protocol and installs its own. */
    if (builtin == KILN_C99_BUILTIN_STRING_PROTOTYPE &&
        kiln_c99_same_name(key, key_len, "@@iterator")) {
        return KILN_C99_METHOD_ARRAY_VALUES;
    }
    switch (builtin) {
    /* `performance` carries exactly one member, and deliberately has no rows in the surface
     * table: a table-driven built-in answers `undefined` for a name it has no row for, and
     * `undefined` is the wrong answer for `performance.mark`. The language really does put
     * `mark`, `measure` and the rest here, this backend has not built them, and a program that
     * asks `typeof performance.mark === "function"` must be refused by name rather than sent
     * quietly down the branch written for a realm that never had them. */
    case KILN_C99_BUILTIN_PERFORMANCE:
        return kiln_c99_same_name(key, key_len, "now") ? KILN_C99_METHOD_PERFORMANCE_NOW : 0;
    case KILN_C99_BUILTIN_ERROR_PROTOTYPE:
        /* `Error.prototype.toString` is the one member that is genuinely the error's rather than
         * `Object.prototype`'s, and it is what `String(err)` reaches. */
        if (kiln_c99_same_name(key, key_len, "toString")) {
            return KILN_C99_METHOD_ERROR_TO_STRING;
        }
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                      KILN_C99_METHOD_OBJECT_VALUE_OF);

    case KILN_C99_BUILTIN_STRING_PROTOTYPE:
        return kiln_c99_string_method(key, key_len);
    case KILN_C99_BUILTIN_ITERATOR_PROTOTYPE:
        /* The same function an instance finds through its chain, out of the same table -- see the
         * `Iterator.prototype` arm of the object read in `kiln_c99_get_own_kind`. */
        return kiln_c99_iterator_prototype_method(key, key_len);
    case KILN_C99_BUILTIN_REGEXP_PROTOTYPE:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_REGEXP_TEST,
                                      KILN_C99_METHOD_REGEXP_TO_STRING);
    case KILN_C99_BUILTIN_DATE_PROTOTYPE:
        if (kiln_c99_same_name(key, key_len, "getTime")) {
            return KILN_C99_METHOD_GET_TIME;
        }
        if (kiln_c99_same_name(key, key_len, "setTime")) {
            return KILN_C99_METHOD_SET_TIME;
        }
        return 0;

    case KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE:
        /* The same range an instance read uses, so `DataView.prototype.getUint8` and
         * `view.getUint8` are the same function -- which is what `===` between them asks, and
         * what a `.call` borrow off the prototype depends on. */
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_DATA_VIEW_GET_INT8,
                                      KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64);
    case KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE:
        if (kiln_c99_same_name(key, key_len, "slice")) {
            return KILN_C99_METHOD_SLICE;
        }
        return 0;
    default:
        return 0;
    }
}
