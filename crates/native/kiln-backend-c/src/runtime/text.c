
static kiln_value kiln_c99_number(double value) {
    uint64_t bits;
    memcpy(&bits, &value, sizeof(bits));
    return kiln_c99_value(KILN_VALUE_NUMBER, bits, NULL, 0);
}

static int kiln_c99_wall_clock_milliseconds(kiln_host *host, double *out_milliseconds) {
#if KILN_REALM_HOST_CLOCK
    kiln_value answer;
    double milliseconds;
    if (host == NULL || host->call == NULL || out_milliseconds == NULL) {
        return KILN_C99_HOST_ERROR;
    }
    if (host->call(host->context, UINT16_C(2), UINT16_C(0), NULL, &answer) != 0 ||
        answer.tag != KILN_VALUE_NUMBER) {
        return KILN_C99_HOST_ERROR;
    }
    memcpy(&milliseconds, &answer.bits, sizeof(milliseconds));
    if (!isfinite(milliseconds)) return KILN_C99_HOST_ERROR;
    *out_milliseconds = milliseconds;
    return KILN_C99_OK;
#else
    (void)host;
    return kiln_realm_now(out_milliseconds);
#endif
}

static int kiln_c99_wall_clock_nanoseconds(kiln_host *host, int64_t *out_high,
                                           uint64_t *out_low) {
#if KILN_REALM_HOST_CLOCK
    double milliseconds;
    double nanoseconds;
    if (out_high == NULL || out_low == NULL ||
        kiln_c99_wall_clock_milliseconds(host, &milliseconds) != KILN_C99_OK) {
        return KILN_C99_HOST_ERROR;
    }
    nanoseconds = trunc(milliseconds * 1000000.0);
    if (!isfinite(nanoseconds) || nanoseconds < 0.0 || nanoseconds > 18446744073709551615.0) {
        return KILN_C99_HOST_ERROR;
    }
    *out_high = 0;
    *out_low = (uint64_t)nanoseconds;
    return KILN_C99_OK;
#else
    (void)host;
    return kiln_realm_now_nanoseconds(out_high, out_low);
#endif
}

static kiln_value kiln_c99_number_bits(uint64_t bits) {
    return kiln_c99_value(KILN_VALUE_NUMBER, bits, NULL, 0);
}

static double kiln_c99_as_number(kiln_value value) {
    double number;
    switch (value.tag) {
    case KILN_VALUE_BIGINT:
        /* Lossy on purpose and only where the language asks for it: `Number(big)` and a
         * comparison against a number. Arithmetic never comes through here -- mixing is a
         * TypeError, which is what stops the precision being lost quietly. */
        return kiln_bigint_to_double((const void *)(uintptr_t)value.bits);
    case KILN_VALUE_NULL:
        return 0.0;
    case KILN_VALUE_BOOLEAN:
        return value.bits ? 1.0 : 0.0;
    case KILN_VALUE_NUMBER:
        memcpy(&number, &value.bits, sizeof(number));
        return number;
    case KILN_VALUE_STRING:
        {
            size_t length = 0;
            const char *bytes = kiln_c99_string_ascii_bytes(value, &length);
            if (bytes != NULL) {
                int is_ascii = 1;
                for (size_t i = 0; i < length; ++i) {
                    if ((unsigned char)bytes[i] >= 0x80) {
                        is_ascii = 0;
                        break;
                    }
                }
                if (is_ascii) {
                    return kiln_string_to_number(bytes, length);
                }
            }
            uint8_t *utf8 = NULL;
            size_t utf8_len = 0;
            if (kiln_c99_string_to_utf8_lossy(value, &utf8, &utf8_len) == KILN_C99_OK) {
                double result = kiln_string_to_number((const char *)utf8, utf8_len);
                free(utf8);
                return result;
            }
            return NAN;
        }
    /* `end - start` on two dates is the difference in milliseconds, which is the whole reason
     * `valueOf` on a Date is its instant. */
    case KILN_VALUE_DATE:
        return kiln_c99_date_time(value);
    default:
        return NAN;
    }
}

static int kiln_c99_truthy(kiln_value value) {
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
    case KILN_VALUE_NULL:
        return 0;
    /* Every object is truthy, and a proxy is one however the handler answers: truthiness is not
     * a trap, so there is nothing here to consult. */
    case KILN_VALUE_PROXY:
        return 1;
    case KILN_VALUE_BOOLEAN:
        return value.bits != 0;
    case KILN_VALUE_NUMBER: {
        double number = kiln_c99_as_number(value);
        return number != 0.0 && !isnan(number);
    }
    case KILN_VALUE_STRING:
        return value.len != 0;
    case KILN_VALUE_FUNCTION:
    case KILN_VALUE_BUILTIN:
    case KILN_VALUE_METHOD:
    case KILN_VALUE_SYMBOL:
    /* An object is always truthy, empty or not. */
    case KILN_VALUE_OBJECT:
    case KILN_VALUE_ARRAY:
    case KILN_VALUE_SHARED:
    case KILN_VALUE_GENERATOR:
    case KILN_VALUE_DATE:
    case KILN_VALUE_COLLECTION:
    case KILN_VALUE_REGEXP:
    case KILN_VALUE_PROMISE:
    case KILN_VALUE_ARRAY_BUFFER:
    case KILN_VALUE_TYPED_ARRAY:
    /* A DataView is an object and objects are truthy. Listed rather than left to `default:` for
     * exactly the reason the `Temporal` note below records -- the default here is *falsy*. */
    case KILN_VALUE_DATA_VIEW:
    /* Including every `Temporal` value, which fell to `default:` and was *falsy* -- so
     * `if (date)` skipped its branch for a date the program was holding in its hand. */
    case KILN_VALUE_TEMPORAL:
        return 1;
    case KILN_VALUE_BIGINT:
        /* Truthiness follows zero, not sign: `-1n` is truthy and `0n` is not. */
        return !kiln_bigint_is_zero((const void *)(uintptr_t)value.bits);
    default:
        return 0;
    }
}

/* A string value that borrows bytes the program cannot outlive: a literal in this translation
 * unit's own read-only data.
 *
 * The collector is precise about edges and paranoid about targets (ADR 0041): every candidate
 * pointer is looked up in the block table and marked only on a hit, so a string pointing at a
 * literal is ignored by the tracer, never swept, and never released -- `kiln_c99_gc_release`
 * returns on a table miss for the same reason. That is what makes a constant answer free where
 * `kiln_c99_copy_text` would allocate a block per request. */
static kiln_value kiln_c99_copy_text(const char *text, size_t len);
static kiln_value kiln_c99_static_text(const char *text, size_t len) {
    typedef struct { kiln_string header; const char *source; } static_text;
    static static_text kiln_c99_static_texts[128];
    size_t slot;
    if (len == 0) return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)&kiln_c99_empty_string, 0);
    if (!kiln_c99_ascii_scan(text, len)) return kiln_c99_copy_text(text, len);
    /* Shared by every realm of the process, so written by the slot protocol's rule on a smaller
     * scale: a header is claimed (`source` from NULL to the claim), filled, then published by a
     * release store of `source`; a reader matches `source` first and reads the header after.
     * An entry is never rewritten. */
    static const char kiln_claimed;
    for (slot = 0; slot < sizeof(kiln_c99_static_texts) / sizeof(kiln_c99_static_texts[0]); ++slot) {
        const char *source = __atomic_load_n(&kiln_c99_static_texts[slot].source, __ATOMIC_ACQUIRE);
        if (source == text && kiln_c99_static_texts[slot].header.unit_len == len)
            return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)&kiln_c99_static_texts[slot].header, len);
    }
    for (slot = 0; slot < sizeof(kiln_c99_static_texts) / sizeof(kiln_c99_static_texts[0]); ++slot) {
        const char *empty = NULL;
        if (__atomic_compare_exchange_n(&kiln_c99_static_texts[slot].source, &empty, &kiln_claimed, 0,
                                        __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
            kiln_c99_static_texts[slot].header.unit_len = (uint32_t)len;
            kiln_c99_static_texts[slot].header.flags = KILN_STRING_LATIN1;
            kiln_c99_static_texts[slot].header.data = text;
            __atomic_store_n(&kiln_c99_static_texts[slot].source, text, __ATOMIC_RELEASE);
            return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)&kiln_c99_static_texts[slot].header, len);
        }
    }
    /* Builtin text is ASCII and the table is deliberately bounded; an exhausted cache is still
     * correct, only less allocation-efficient. */
    return kiln_c99_copy_text(text, len);
}

static inline size_t kiln_c99_utf8_to_utf16_len(const char *text, size_t len) {
    size_t count = 0;
    for (size_t i = 0; i < len; ++i) {
        unsigned char b = (unsigned char)text[i];
        if ((b & 0xC0) != 0x80) {
            count += ((b & 0xF8) == 0xF0) ? 2 : 1;
        }
    }
    return count;
}

static uint32_t kiln_c99_utf8_next(const char *text, size_t length, size_t *offset);
static kiln_string *kiln_c99_gc_alloc_units(size_t count, uint32_t flags) {
    size_t storage;
    if (count > UINT32_MAX || !kiln_string_storage_size((uint32_t)count, flags, &storage)) return NULL;
    kiln_string *string = (kiln_string *)kiln_c99_gc_alloc_hot(
        KILN_GC_STRING, storage, 1);
    if (string == NULL) return NULL;
    string->unit_len = (uint32_t)count;
    string->flags = flags;
    string->data = (const void *)(string + 1);
    return string;
}

static kiln_string *kiln_c99_string_from_utf8(const char *bytes, size_t len) {
    if (kiln_c99_ascii_scan(bytes, len)) {
        kiln_string *string = kiln_c99_gc_alloc_units(len, KILN_STRING_LATIN1);
        if (string != NULL && len != 0) memcpy((void *)string->data, bytes, len);
        return string;
    }
    size_t count = kiln_c99_utf8_to_utf16_len(bytes, len);
    kiln_string *string = kiln_c99_gc_alloc_units(count, KILN_STRING_UTF16);
    if (string == NULL) return NULL;
    uint16_t *units = (uint16_t *)(uintptr_t)string->data;
    size_t offset = 0;
    size_t index = 0;
    while (index < count) {
        uint32_t codepoint = kiln_c99_utf8_next(bytes, len, &offset);
        if (codepoint > UINT32_C(0xFFFF)) {
            codepoint -= UINT32_C(0x10000);
            units[index++] = (uint16_t)(UINT32_C(0xD800) | (codepoint >> 10));
            units[index++] = (uint16_t)(UINT32_C(0xDC00) | (codepoint & UINT32_C(0x3FF)));
        } else {
            units[index++] = (uint16_t)codepoint;
        }
    }
    return string;
}

static kiln_val kiln_c99_val_from_text(const char *bytes, size_t len) {
    if (len == 0 || bytes == NULL) {
        return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
    }
    if (len == 1 && (unsigned char)bytes[0] < 128) {
        return kiln_c99_val_single_char((unsigned char)bytes[0]);
    }
    kiln_string *string = kiln_c99_string_from_utf8(bytes, len);
    if (string == NULL) return KILN_VAL_UNDEFINED;
    return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, string);
}

/* Internal indexed access never decodes through UTF-8.  In particular, a lone surrogate is a
 * valid JavaScript UTF-16 code unit and must remain one unit when a property read returns it.
 * Keep the result in the native tagged representation so callers do not accidentally route it
 * through the lossy boundary codec. */
static kiln_val kiln_c99_val_from_unit(uint16_t unit) {
    if (unit < 0x100u) return kiln_c99_val_single_char((unsigned char)unit);
    kiln_string *string = kiln_c99_gc_alloc_units(1, KILN_STRING_UTF16);
    if (string == NULL) return KILN_VAL_UNDEFINED;
    ((uint16_t *)(uintptr_t)string->data)[0] = unit;
    return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, string);
}

static kiln_val kiln_c99_val_from_ascii(const char *bytes, size_t len) {
    if (len == 0 || bytes == NULL) {
        return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
    }
    if (len == 1 && (unsigned char)bytes[0] < 128) {
        return kiln_c99_val_single_char((unsigned char)bytes[0]);
    }
    char *copy = kiln_c99_gc_alloc_text(len);
    if (copy == NULL) {
        return KILN_VAL_UNDEFINED;
    }
    memcpy(copy, bytes, len);
    kiln_string *s = ((kiln_string *)copy) - 1;
    s->unit_len = (uint32_t)len;
    s->flags = KILN_STRING_LATIN1;
    s->data = copy;
    return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, s);
}

static kiln_value kiln_c99_copy_ascii(const char *text, size_t len);
static kiln_value kiln_c99_copy_text(const char *text, size_t len) {
    kiln_string *decoded;
    if (len == 0 || text == NULL) {
        return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)&kiln_c99_empty_string, 0);
    }
    if (kiln_c99_ascii_scan(text, len)) return kiln_c99_copy_ascii(text, len);
    decoded = kiln_c99_string_from_utf8(text, len);
    if (decoded == NULL) {
        return kiln_c99_undefined();
    }
    kiln_value result = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)decoded, decoded->unit_len);
    result.flags = KILN_VALUE_FLAG_OWNED_STRING;
    return result;
}

/* Callers handling protocol bytes or already-decoded Latin1 use this explicitly.  Keeping it
 * separate prevents arbitrary UTF-8 input from being mislabeled as one-byte JavaScript units. */
static kiln_value kiln_c99_copy_ascii(const char *text, size_t len) {
    kiln_string *s;
    char *copy;
    if (len == 0 || text == NULL)
        return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)&kiln_c99_empty_string, 0);
    copy = kiln_c99_gc_alloc_text(len);
    if (copy == NULL) return kiln_c99_undefined();
    memcpy(copy, text, len);
    s = ((kiln_string *)copy) - 1;
    return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)s, s->unit_len);
}

static kiln_value kiln_c99_control_diagnostic(void) {
    if (kiln_realm_is_active()) {
        uint64_t len = 0;
        uint8_t *bytes;
        if (kiln_realm_diagnostic_render(NULL, 0, &len) == KILN_C99_INVALID_ARGUMENT && len != 0) {
            const kiln_string *rendered;
            bytes = (uint8_t *)kiln_c99_gc_alloc_text((size_t)len);
            if (bytes != NULL && kiln_realm_diagnostic_render(bytes, len, &len) == KILN_C99_OK) {
                /* `gc_alloc_text` answers the payload; a string value carries the header the
                 * payload shares its allocation with. Passing the payload reads the header
                 * words as units further down the line. */
                rendered = ((const kiln_string *)bytes) - 1;
                return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)rendered,
                                      (size_t)len);
            }
            kiln_c99_gc_release(bytes);
        }
    }
    kiln_text text = {NULL, 0, 0};
    char number[32];
    size_t written;
    uint64_t value;
#define KILN_DIAGNOSTIC_DECIMAL(subject) do { \
    value = (uint64_t)(subject); written = 0; \
    do { number[written++] = (char)('0' + value % 10); value /= 10; } while (value != 0); \
    for (size_t left = 0, right = written - 1; left < right; ++left, --right) { \
        char swap = number[left]; number[left] = number[right]; number[right] = swap; \
    } \
} while (0)
    if (kiln_c99_diagnostic_site != NULL) {
        (void)kiln_c99_text_append_c(&text, "module=");
        (void)kiln_c99_text_append_c(&text, kiln_c99_diagnostic_site);
        KILN_DIAGNOSTIC_DECIMAL(kiln_c99_diagnostic_instruction);
        (void)kiln_c99_text_append_c(&text, " instruction=");
        (void)kiln_c99_text_append(&text, number, written);
    }
    if (kiln_c99_diagnostic_operation != NULL) {
        (void)kiln_c99_text_append_c(&text, " operation=");
        (void)kiln_c99_text_append_c(&text, kiln_c99_diagnostic_operation);
    }
    if (kiln_c99_diagnostic_detail != NULL) {
        (void)kiln_c99_text_append_c(&text, " detail=");
        (void)kiln_c99_text_append_c(&text, kiln_c99_diagnostic_detail);
    }
    if (kiln_c99_diagnostic_function != NULL || kiln_c99_diagnostic_function_key != NULL) {
        (void)kiln_c99_text_append_c(&text, " function=");
        if (kiln_c99_diagnostic_function_key != NULL) {
            kiln_value function = kiln_c99_value(
                KILN_VALUE_STRING, 0, (const char *)kiln_c99_diagnostic_function_key,
                kiln_c99_diagnostic_function_key->unit_len);
            (void)kiln_c99_text_append_string(&text, function);
        } else {
            (void)kiln_c99_text_append(&text, kiln_c99_diagnostic_function,
                                       kiln_c99_diagnostic_function_len);
        }
    }
    if (kiln_c99_diagnostic_has_method) {
        KILN_DIAGNOSTIC_DECIMAL(kiln_c99_diagnostic_method);
        (void)kiln_c99_text_append_c(&text, " method=");
        (void)kiln_c99_text_append(&text, number, written);
        KILN_DIAGNOSTIC_DECIMAL(kiln_c99_diagnostic_receiver_tag);
        (void)kiln_c99_text_append_c(&text, " receiver_tag=");
        (void)kiln_c99_text_append(&text, number, written);
    } else if (kiln_c99_diagnostic_receiver_tag != 0) {
        KILN_DIAGNOSTIC_DECIMAL(kiln_c99_diagnostic_receiver_tag);
        (void)kiln_c99_text_append_c(&text, " receiver_tag=");
        (void)kiln_c99_text_append(&text, number, written);
    }
#undef KILN_DIAGNOSTIC_DECIMAL
    if (text.bytes == NULL) return kiln_c99_copy_text("unknown native operation", 24);
    return kiln_c99_text_value(&text);
}

/* Forget what a throw was about, once a handler has taken it.
 *
 * The diagnostic fields are written only while still `NULL`, so the first note of a run wins for
 * the rest of it. That is right for a refusal, which ends the program at the place it happened --
 * and wrong for a throw the program caught, which is an ordinary event that explains nothing about
 * whatever refuses later. A `try { view.getUint8(9) } catch {}` forty lines earlier was still
 * naming itself as the cause of an unrelated refusal, reporting its own instruction index with it,
 * and it sent two separate investigations to the wrong line. */
static void kiln_c99_forget_handled_throw(void) {
    kiln_c99_diagnostic_site = NULL;
    kiln_c99_diagnostic_operation = NULL;
    kiln_c99_diagnostic_instruction = 0;
    kiln_c99_diagnostic_depth = 0;
    kiln_c99_diagnostic_method = 0;
    kiln_c99_diagnostic_receiver_tag = 0;
    kiln_c99_diagnostic_has_method = 0;
    kiln_c99_diagnostic_detail = NULL;
    kiln_c99_diagnostic_function = NULL;
    kiln_c99_diagnostic_function_len = 0;
    kiln_c99_diagnostic_function_key = NULL;
    /* The realm keeps its own copy, and it is the one that is *rendered*: a refusal reaching the
     * embedding is composed by `kiln_realm_diagnostic_render` whenever a realm is active, and the
     * statics above are only the fallback for a unit running without one. Clearing half of the
     * pair left a handled throw naming itself for the rest of the run through the other half --
     * which is how a `hasOwnProperty` refusal came to report `CallMethod invocation threw` about
     * a `TypeError` two `say()` calls earlier. */
    if (kiln_realm_is_active()) {
        (void)kiln_realm_diagnostic_reset();
    }
}

KILN_COLD void kiln_c99_publish_diagnostic(void) {
    uint8_t *function_owned = NULL;
    size_t function_owned_len = 0;
    const uint8_t *function_bytes;
    size_t function_len;
    if (!kiln_realm_is_active() || kiln_c99_diagnostic_site == NULL ||
        kiln_c99_diagnostic_operation == NULL) return;
    const char *detail = kiln_c99_diagnostic_detail;
    const char *function = kiln_c99_diagnostic_function;
    if (kiln_c99_diagnostic_function_key != NULL) {
        kiln_value function_value = kiln_c99_value(
            KILN_VALUE_STRING, 0, (const char *)kiln_c99_diagnostic_function_key,
            kiln_c99_diagnostic_function_key->unit_len);
        if (kiln_c99_string_to_utf8_lossy(function_value, &function_owned,
                                          &function_owned_len) == KILN_C99_OK) {
            function_bytes = function_owned;
            function_len = function_owned_len;
        } else {
            function_bytes = (const uint8_t *)"";
            function_len = 0;
        }
    } else {
        function_bytes = (const uint8_t *)(function == NULL ? "" : function);
        function_len = function == NULL ? 0 : kiln_c99_diagnostic_function_len;
    }
    (void)kiln_realm_diagnostic_note(
        (const uint8_t *)kiln_c99_diagnostic_site, (uint64_t)strlen(kiln_c99_diagnostic_site),
        (uint64_t)kiln_c99_diagnostic_instruction,
        (const uint8_t *)kiln_c99_diagnostic_operation,
        (uint64_t)strlen(kiln_c99_diagnostic_operation),
        (uint64_t)kiln_c99_diagnostic_depth,
        (const uint8_t *)(detail == NULL ? "" : detail),
        (uint64_t)(detail == NULL ? 0 : strlen(detail)),
        function_bytes, (uint64_t)function_len,
        kiln_c99_diagnostic_method, kiln_c99_diagnostic_receiver_tag,
        kiln_c99_diagnostic_has_method);
    free(function_owned);
}

int kiln_c99_diagnostic_available(void) {
    return kiln_c99_diagnostic_site != NULL && kiln_c99_diagnostic_operation != NULL;
}

static KILN_COLD void kiln_c99_note_refusal(const char *site, size_t instruction,
                                  const char *operation, size_t depth) {
    /* Runtime helpers do not own a bytecode instruction and deliberately pass no site. The
     * generated caller will attribute a propagated throw/refusal to its real instruction. In an
     * active realm, forwarding this partial record would also ask the diagnostic bridge to
     * measure a null C string. */
    if (site == NULL || operation == NULL) return;
    int replaces_body_fallback =
        kiln_c99_diagnostic_operation != NULL &&
        strcmp(kiln_c99_diagnostic_operation, "FunctionBody") == 0;
    /* A helper may add detail before the generated instruction that owns the failure has had a
     * chance to name itself. That partial diagnostic is not provenance. The first instruction at
     * the deepest call depth owns the site; equal-depth control unwinding must not blame a caller. */
    if (kiln_c99_diagnostic_site != NULL &&
        kiln_c99_diagnostic_operation != NULL &&
        strcmp(kiln_c99_diagnostic_operation, "FunctionBody") != 0 &&
        depth <= kiln_c99_diagnostic_depth) return;
    kiln_c99_diagnostic_site = site;
    kiln_c99_diagnostic_instruction = instruction;
    kiln_c99_diagnostic_operation = operation;
    kiln_c99_diagnostic_depth = depth;
    /* FunctionBody is only a fallback for a body that returned before any emitted instruction.
     * Once an instruction claims the failure, its operation owns the payload too. */
    if (replaces_body_fallback) {
        if (kiln_c99_diagnostic_detail != NULL &&
            strcmp(kiln_c99_diagnostic_detail,
                   "generated function body refused before an instruction owned the failure") == 0) {
            kiln_c99_diagnostic_function = NULL;
            kiln_c99_diagnostic_function_len = 0;
            kiln_c99_diagnostic_function_key = NULL;
            kiln_c99_diagnostic_detail = NULL;
            kiln_c99_diagnostic_receiver_tag = 0;
            kiln_c99_diagnostic_has_method = 0;
        }
    }
    kiln_c99_publish_diagnostic();
}


/* The growth half of `kiln_c99_text_append`; the fit-in-place half is inline in prelude_types.h. */
static int kiln_c99_text_append_grow(kiln_text *text, const char *bytes, size_t len) {
    int status;
    if (text->borrowed) {
        size_t grown = text->capacity == 0 ? 4 : text->capacity;
        char *owned;
        while (grown < text->len + len) grown *= 2;
        owned = (char *)malloc(grown);
        if (owned == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        memcpy(owned, text->bytes, text->len);
        text->bytes = owned;
        text->capacity = grown;
        text->borrowed = 0;
    } else {
        status = kiln_c99_reserve((void **)&text->bytes, &text->capacity, text->len + len, 1);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    memcpy(text->bytes + text->len, bytes, len);
    text->len += len;
    return KILN_C99_OK;
}

static int kiln_c99_text_append_c(kiln_text *text, const char *literal) {
    return kiln_c99_text_append(text, literal, strlen(literal));
}

/* A builder's bytes become a string value here and only here (ADR 0041).
 *
 * The builder itself is scratch: it doubles through `realloc`, and a collected block cannot move,
 * so the two cannot be the same allocation. The copy at the escape is the price, and it is paid
 * once per string rather than once per append. The builder is emptied as well, so an error path
 * further down that still calls `free(text.bytes)` frees nothing rather than freeing a block. */
static kiln_value kiln_c99_text_value(kiln_text *text) {
    /* An empty build answers the shared empty header like every other
     * constructor (`static_text`, `copy_text`, `val_from_text`): a NULL
     * pointer is not a string here, and `concat` and `strict_equal`
     * answer `undefined` and `false` for one. */
    kiln_value result = text->len == 0
                            ? kiln_c99_value(KILN_VALUE_STRING, 0,
                                             (const char *)&kiln_c99_empty_string, 0)
                            : kiln_c99_copy_text(text->bytes, text->len);
    if (!text->borrowed) free(text->bytes);
    text->borrowed = 0;
    text->bytes = NULL;
    text->len = 0;
    text->capacity = 0;
    return result;
}

/* The same escape as `kiln_c99_text_value`, answering the heap's own representation.
 *
 * A boundary `kiln_value` string is a bare pointer and length with no heap identity, so a caller
 * that wanted a `kiln_val` had to hand this function's answer to `kiln_val_from_boundary` -- which
 * has no way to know the bytes were already in a block of this heap and allocates a second one,
 * copying every byte and scanning it for ASCII a second time. `JSON.stringify` did exactly that
 * for every document it built. */
static kiln_val kiln_c99_text_val(kiln_text *text) {
    kiln_val result = kiln_c99_val_from_text(text->bytes, text->len);
    if (!text->borrowed) free(text->bytes);
    text->borrowed = 0;
    text->bytes = NULL;
    text->len = 0;
    text->capacity = 0;
    return result;
}

/* Node decodes a `readFile(..., "utf8")` result with replacement semantics rather than exposing
 * malformed bytes as a JavaScript string. Match Rust's `String::from_utf8_lossy`: consume the
 * valid prefix of an otherwise malformed sequence, leave the offending byte to be reconsidered,
 * and collapse a truncated continuation suffix into one U+FFFD. */
static kiln_value kiln_c99_copy_utf8_lossy(const uint8_t *bytes, size_t length) {
    static const char replacement[] = "\xEF\xBF\xBD";
    kiln_text decoded = {NULL, 0, 0};
    size_t at = 0;
    if (length == 0) return kiln_c99_copy_text(NULL, 0);
    while (at < length) {
        unsigned char first = bytes[at];
        size_t width = 0;
        size_t continuation;
        if (first < 0x80) {
            width = 1;
        } else if (first >= 0xC2 && first <= 0xDF) {
            width = 2;
        } else if (first >= 0xE0 && first <= 0xEF) {
            width = 3;
        } else if (first >= 0xF0 && first <= 0xF4) {
            width = 4;
        }
        if (width == 0) {
            if (kiln_c99_text_append(&decoded, replacement, 3) != KILN_C99_OK) goto oom;
            at += 1;
            continue;
        }
        if (width > 1 && at + 1 < length) {
            unsigned char second = bytes[at + 1];
            if ((second & 0xC0) != 0x80 || (first == 0xE0 && second < 0xA0) ||
                (first == 0xED && second >= 0xA0) || (first == 0xF0 && second < 0x90) ||
                (first == 0xF4 && second > 0x8F)) {
                if (kiln_c99_text_append(&decoded, replacement, 3) != KILN_C99_OK) goto oom;
                at += 1;
                continue;
            }
        }
        for (continuation = 1; continuation < width; ++continuation) {
            if (at + continuation >= length) {
                if (kiln_c99_text_append(&decoded, replacement, 3) != KILN_C99_OK) goto oom;
                at = length;
                break;
            }
            if ((bytes[at + continuation] & 0xC0) != 0x80) {
                if (kiln_c99_text_append(&decoded, replacement, 3) != KILN_C99_OK) goto oom;
                at += continuation;
                break;
            }
        }
        if (continuation != width) continue;
        if (kiln_c99_text_append(&decoded, (const char *)bytes + at, width) != KILN_C99_OK)
            goto oom;
        at += width;
    }
    return kiln_c99_text_value(&decoded);
oom:
    free(decoded.bytes);
    return kiln_c99_undefined();
}

static int kiln_c99_string_to_utf8_lossy(kiln_value value, uint8_t **out, size_t *out_len) {
    const kiln_string *string;
    size_t capacity;
    size_t written = 0;
    size_t index;
    uint8_t *bytes;
    if (out == NULL || out_len == NULL || value.tag != KILN_VALUE_STRING || value.ptr == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    string = (const kiln_string *)value.ptr;
#if SIZE_MAX <= UINT32_MAX
    if ((uint64_t)string->unit_len > ((uint64_t)SIZE_MAX - 4) / 3) {
        return KILN_C99_OUT_OF_MEMORY;
    }
#endif
    capacity = (size_t)string->unit_len * 3 + 4;
    bytes = capacity == 0 ? NULL : (uint8_t *)malloc(capacity);
    if (capacity != 0 && bytes == NULL) return KILN_C99_OUT_OF_MEMORY;
    for (index = 0; index < string->unit_len; ++index) {
        uint32_t unit = kiln_string_unit_at(string, index);
        uint32_t codepoint = unit;
        if (kiln_string_is_utf16(string) && unit >= 0xD800 && unit <= 0xDBFF &&
            index + 1 < string->unit_len) {
            uint32_t next = kiln_string_unit_at(string, index + 1);
            if (next >= 0xDC00 && next <= 0xDFFF) {
                codepoint = 0x10000u + ((unit - 0xD800u) << 10) + (next - 0xDC00u);
                index += 1;
            }
        }
        if (codepoint >= 0xD800 && codepoint <= 0xDFFF) codepoint = 0xFFFD;
        if (codepoint < 0x80) {
            bytes[written++] = (uint8_t)codepoint;
        } else if (codepoint < 0x800) {
            bytes[written++] = (uint8_t)(0xC0 | (codepoint >> 6));
            bytes[written++] = (uint8_t)(0x80 | (codepoint & 0x3F));
        } else if (codepoint < 0x10000) {
            bytes[written++] = (uint8_t)(0xE0 | (codepoint >> 12));
            bytes[written++] = (uint8_t)(0x80 | ((codepoint >> 6) & 0x3F));
            bytes[written++] = (uint8_t)(0x80 | (codepoint & 0x3F));
        } else {
            bytes[written++] = (uint8_t)(0xF0 | (codepoint >> 18));
            bytes[written++] = (uint8_t)(0x80 | ((codepoint >> 12) & 0x3F));
            bytes[written++] = (uint8_t)(0x80 | ((codepoint >> 6) & 0x3F));
            bytes[written++] = (uint8_t)(0x80 | (codepoint & 0x3F));
        }
    }
    *out = bytes;
    *out_len = written;
    return KILN_C99_OK;
}

/* Text builders store UTF-8 bytes until their final escape into the canonical unit string. Keep
 * this conversion explicit at the few host-facing builders that combine a value with literal
 * bytes; treating the canonical header as a byte span would serialize its metadata. */
static int kiln_c99_text_append_string(kiln_text *text, kiln_value value) {
    uint8_t *owned = NULL;
    size_t length = 0;
    int status = kiln_c99_string_to_utf8_lossy(value, &owned, &length);
    if (status == KILN_C99_OK) status = kiln_c99_text_append(text, (const char *)owned, length);
    free(owned);
    return status;
}

static kiln_value kiln_c99_text(kiln_value value) {
    static const char undefined_text[] = "undefined";
    static const char null_text[] = "null";
    static const char true_text[] = "true";
    static const char false_text[] = "false";
    /* The unchecked conversion, reached where there is no error to raise: a proxy shows as its
     * target. The conversions a program can observe go through `kiln_c99_to_primitive`, which
     * refuses one instead. */
    value = kiln_c99_proxy_inner(value);
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
        return kiln_c99_static_text(undefined_text, sizeof(undefined_text) - 1);
    case KILN_VALUE_NULL:
        return kiln_c99_static_text(null_text, sizeof(null_text) - 1);
    case KILN_VALUE_BOOLEAN:
        if (value.bits) {
            return kiln_c99_static_text(true_text, sizeof(true_text) - 1);
        }
        return kiln_c99_static_text(false_text, sizeof(false_text) - 1);
    case KILN_VALUE_STRING:
        return value;
    case KILN_VALUE_NUMBER: {
        char buffer[32];
        double number = kiln_c99_as_number(value);
        /* Spelt first, copied second: one statement passing `buffer` to both reads as though the
         * two could interleave, though C sequences a call's arguments before its body. */
        size_t length = kiln_c99_double_to_js(number, buffer);
        return kiln_c99_copy_text(buffer, length);
    }
    case KILN_VALUE_SHARED: {
        /* Observable conversion asks the facade's prototype through ToPrimitive, so an array
         * joins its elements and a null-prototype record may throw. This unchecked text path
         * is used by diagnostics and the embedding boundary, where a record's fallback is the
         * same ordinary-object label as its source. */
        static const char object_text[] = "[object Object]";
        return kiln_c99_static_text(object_text, sizeof(object_text) - 1);
    }
    case KILN_VALUE_OBJECT: {
        static const char object_text[] = "[object Object]";
        kiln_value kind;
        kiln_value shown;
        /* An error converts to `Name: message`, not `[object Object]`: it is the one object with
         * a text form of its own, and every log line that prints one depends on it. */
        if (kiln_c99_get(NULL, value, "@@error", 7, &kind, 0, NULL) == KILN_C99_OK &&
            kind.tag == KILN_VALUE_NUMBER &&
            kiln_c99_get(NULL, value, "stack", 5, &shown, 0, NULL) == KILN_C99_OK &&
            shown.tag == KILN_VALUE_STRING) {
            return shown;
        }
        return kiln_c99_static_text(object_text, sizeof(object_text) - 1);
    }
    case KILN_VALUE_DATE: {
        /* `String(date)` is the human form, not the ISO one: the hint is a string and
         * `toString` decides. Arithmetic reaches the instant through `as_number` instead. */
        kiln_value shown;
        if (kiln_c99_date_to_string(&shown, kiln_c99_date_time(value)) != KILN_C99_OK) {
            return kiln_c99_undefined();
        }
        return shown;
    }
    case KILN_VALUE_ARRAY: {
        /* `String([1, , 3])` is "1,,3": the separator is a comma and a nullish element
         * contributes nothing, which is `Array.prototype.join` with its default. */
        kiln_array *array = kiln_c99_array_of(value);
        kiln_text joined = {NULL, 0, 0};
        size_t index;
        for (index = 0; index < array->count; ++index) {
            kiln_val element = array->elements[index];
            if (index != 0 && kiln_c99_text_append(&joined, ",", 1) != KILN_C99_OK) {
                free(joined.bytes);
                return kiln_c99_undefined();
            }
            if (element == KILN_VAL_UNDEFINED || element == KILN_VAL_NULL ||
                element == KILN_VAL_HOLE) {
                continue;
            }
            {
                kiln_value shown = kiln_c99_text(kiln_val_to_boundary(element));
                uint8_t *encoded = NULL;
                size_t encoded_len = 0;
                if (shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_string_to_utf8_lossy(shown, &encoded, &encoded_len) != KILN_C99_OK ||
                    kiln_c99_text_append(&joined, (const char *)encoded, encoded_len) != KILN_C99_OK) {
                    free(encoded);
                    free(joined.bytes);
                    return kiln_c99_undefined();
                }
                free(encoded);
            }
        }
        return kiln_c99_text_value(&joined);
    }
    case KILN_VALUE_BIGINT: {
        /* No `n` suffix: that is the literal's spelling, not the value's -- `String(1n)` is
         * `"1"`. A console adds it back, which is a different question and answered there. */
        return kiln_c99_bigint_text(value, 10);
    }
    case KILN_VALUE_TYPED_ARRAY: {
        kiln_typed_array *view = kiln_c99_typed_array_of(value);
        kiln_text joined = {NULL, 0, 0};
        size_t index;
        for (index = 0; index < view->length; ++index) {
            kiln_value shown;
            if (index != 0 && kiln_c99_text_append(&joined, ",", 1) != KILN_C99_OK) {
                free(joined.bytes);
                return kiln_c99_undefined();
            }
            if (kiln_c99_typed_element(view, index, &shown) != KILN_C99_OK) {
                free(joined.bytes);
                return kiln_c99_undefined();
            }
            shown = kiln_c99_text(shown);
            if (shown.tag != KILN_VALUE_STRING ||
                kiln_c99_text_append(&joined, (const char *)kiln_string_latin1((const kiln_string *)shown.ptr),
                                     ((const kiln_string *)shown.ptr)->unit_len) != KILN_C99_OK) {
                free(joined.bytes);
                return kiln_c99_undefined();
            }
        }
        return kiln_c99_text_value(&joined);
    }
    case KILN_VALUE_ARRAY_BUFFER:
        if (kiln_c99_typed_buffer_of(value)->shared != NULL)
            return kiln_c99_copy_text("[object SharedArrayBuffer]", 26);
        return kiln_c99_copy_text("[object ArrayBuffer]", 20);
    /* A DataView holds no elements to join, so unlike a typed array it converts to the tagged
     * form its buffer does -- which is what node and the interpreter both answer. */
    case KILN_VALUE_DATA_VIEW:
        return kiln_c99_copy_text("[object DataView]", 17);
    case KILN_VALUE_REGEXP: {
        /* `String(/a/g)` is the literal as written, which is what `toString` returns. */
        kiln_regexp *regexp = kiln_c99_regexp_of(value);
        kiln_text built = {NULL, 0, 0};
        if (kiln_c99_text_append_c(&built, "/") != KILN_C99_OK ||
            kiln_c99_text_append_string(&built,
                                        kiln_c99_value(KILN_VALUE_STRING, 0,
                                                       (const char *)regexp->source,
                                                       regexp->source == NULL ? 0
                                                                              : regexp->source->unit_len)) !=
                KILN_C99_OK ||
            kiln_c99_text_append_c(&built, "/") != KILN_C99_OK ||
            kiln_c99_text_append_string(&built,
                                        kiln_c99_value(KILN_VALUE_STRING, 0,
                                                       (const char *)regexp->flags,
                                                       regexp->flags == NULL ? 0
                                                                            : regexp->flags->unit_len)) !=
                KILN_C99_OK) {
            free(built.bytes);
            return kiln_c99_undefined();
        }
        return kiln_c99_text_value(&built);
    }
    case KILN_VALUE_SYMBOL: {
        /* `String(sym)` spells the symbol out. Concatenation is a different question -- the
         * language throws for `sym + ""` -- and neither engine draws that line yet. */
        kiln_symbol *symbol = kiln_c99_symbol_of(value);
        kiln_text built = {NULL, 0, 0};
        if (kiln_c99_text_append_c(&built, "Symbol(") != KILN_C99_OK ||
            (symbol->description_value.tag == KILN_VALUE_STRING
                 ? kiln_c99_text_append_string(&built, symbol->description_value)
                 : kiln_c99_text_append(&built, symbol->description, symbol->description_len)) !=
                KILN_C99_OK ||
            kiln_c99_text_append_c(&built, ")") != KILN_C99_OK) {
            free(built.bytes);
            return kiln_c99_undefined();
        }
        return kiln_c99_text_value(&built);
    }
    case KILN_VALUE_METHOD:
    case KILN_VALUE_BUILTIN: {
        /* The interpreter names a runtime-supplied callable by the native it resolves to, and
         * this has to be the same text for the same program. A namespace converts as the object
         * it is -- `String(Math)` is `[object Math]`, not a function's name. */
        kiln_text built = {NULL, 0, 0};
        const char *name = value.tag == KILN_VALUE_METHOD ? kiln_c99_method_name(value.bits)
                                                          : kiln_c99_builtin_name(value.bits);
        if (name == NULL) {
            return kiln_c99_undefined();
        }
        /* A namespace is an object and stringifies as its tag.  Everything else here is a
         * built-in function, and the specification requires the NativeFunction form for one --
         * which is also what `/\\[native code\\]/` feature detection reads. */
        if (value.tag == KILN_VALUE_BUILTIN && kiln_c99_is_namespace(value.bits)) {
            if (kiln_c99_text_append_c(&built, "[object ") != KILN_C99_OK ||
                kiln_c99_text_append_c(&built, name) != KILN_C99_OK ||
                kiln_c99_text_append_c(&built, "]") != KILN_C99_OK) {
                free(built.bytes);
                return kiln_c99_undefined();
            }
            return kiln_c99_text_value(&built);
        }
        if (kiln_c99_text_append_c(&built, "function ") != KILN_C99_OK ||
            kiln_c99_text_append_c(&built, name) != KILN_C99_OK ||
            kiln_c99_text_append_c(&built, "() { [native code] }") != KILN_C99_OK) {
            free(built.bytes);
            return kiln_c99_undefined();
        }
        return kiln_c99_text_value(&built);
    }
    case KILN_VALUE_FUNCTION: {
        /* `Function.prototype.toString`: the text the program wrote, carried on the closure as a
         * pointer into the unit's static source.  The interpreter slices the same span out of
         * the same text, so the two engines answer with identical bytes. */
        kiln_closure *closure = kiln_c99_closure_of(value);
        if (closure->source_value.tag == KILN_VALUE_STRING) {
            return closure->source_value;
        }
        if (closure->source == NULL) {
            /* A closure the runtime built for itself has no text.  It is a built-in in every
             * sense that matters here, so it answers as one. */
            return kiln_c99_copy_text("function () { [native code] }", 29);
        }
        return kiln_c99_copy_text(closure->source, closure->source_len);
    }
    /* A `Temporal` value's text form is its ISO string, which is what its own `toString`
     * answers and what `String(d)` and `${d}` have to produce. It had no case at all, so both
     * read `undefined` -- and, through `kiln_c99_concat`'s length check, `"x" + d` appended
     * nothing and printed "x". The handle renders itself; there is no second spelling of an ISO
     * date anywhere here for the two to drift apart on. */
#if KILN_RETAIN_GROUP_TEMPORAL
    case KILN_VALUE_TEMPORAL: {
        char buffer[128];
        int32_t written = kiln_temporal_to_string(kiln_c99_temporal_of(value),
                                                  (unsigned char *)buffer, sizeof buffer);
        if (written < 0 || (size_t)written > sizeof buffer) {
            return kiln_c99_undefined();
        }
        return kiln_c99_copy_text(buffer, (size_t)written);
    }
#endif
    /* The three object kinds whose text form is the tagged `[object X]` and nothing else. They
     * inherit it from `Object.prototype.toString` in the language; this backend models that as
     * `kiln_c99_text` rather than as a method a property walk finds, exactly as it does for a
     * namespace. Without a case here `String(new Map())` threw "Cannot convert object to
     * primitive value" where the language says "[object Map]". */
    case KILN_VALUE_COLLECTION: {
        kiln_collection *collection = kiln_c99_collection_of(value);
        const char *shown = collection->is_weak
                                ? (collection->is_map ? "[object WeakMap]" : "[object WeakSet]")
                                : (collection->is_map ? "[object Map]" : "[object Set]");
        return kiln_c99_copy_text(shown, strlen(shown));
    }
    case KILN_VALUE_PROMISE:
        return kiln_c99_copy_text("[object Promise]", 16);
    case KILN_VALUE_GENERATOR:
        return kiln_c99_copy_text("[object Generator]", 18);
    /* The remaining callables, each of which `kiln_c99_is_callable` already answers `1` for.
     * None has source text a program wrote, so each takes the NativeFunction form the
     * specification allows for a built-in -- the same answer the interpreter gives.
     *
     * Their absence here is what `callable_text_covers_every_callable_tag` now pins. They fell to
     * `default:` and returned `undefined`, so `bind(f).toString()` refused where `String(f)` on
     * the very same value was right: one conversion, two routes, and only one of them complete. */
    case KILN_VALUE_SETTLER:
    case KILN_VALUE_BOUND:
        return kiln_c99_copy_text("function () { [native code] }", 29);
    case KILN_VALUE_COMMONJS_REQUIRE:
        return kiln_c99_copy_text("function require() { [native code] }", 35);
    default:
        return kiln_c99_undefined();
    }
}

static kiln_value kiln_c99_concat(kiln_value left, kiln_value right) {
    kiln_value l = kiln_c99_text(left);
    kiln_value r = kiln_c99_text(right);
    const kiln_string *ls = l.ptr == NULL ? NULL : (const kiln_string *)l.ptr;
    const kiln_string *rs = r.ptr == NULL ? NULL : (const kiln_string *)r.ptr;
    size_t total;
    uint32_t flags;
    kiln_string *joined;
    if (ls == NULL || rs == NULL || ls->unit_len > SIZE_MAX - rs->unit_len) {
        return kiln_c99_undefined();
    }
    total = (size_t)ls->unit_len + (size_t)rs->unit_len;
    if (total > UINT32_MAX) return kiln_c99_undefined();
    flags = kiln_string_is_utf16(ls) || kiln_string_is_utf16(rs) ? KILN_STRING_UTF16 : KILN_STRING_LATIN1;
    joined = kiln_c99_gc_alloc_units(total, flags);
    if (joined == NULL) {
        return kiln_c99_undefined();
    }
    if (flags == KILN_STRING_UTF16) {
        uint16_t *units = (uint16_t *)(uintptr_t)joined->data;
        if (kiln_string_is_utf16(ls) && kiln_string_is_utf16(rs)) {
            memcpy(units, ls->data, ls->unit_len * sizeof(uint16_t));
            memcpy(units + ls->unit_len, rs->data, rs->unit_len * sizeof(uint16_t));
        } else {
            for (size_t index = 0; index < ls->unit_len; ++index) units[index] = kiln_string_unit_at(ls, index);
            for (size_t index = 0; index < rs->unit_len; ++index) units[ls->unit_len + index] = kiln_string_unit_at(rs, index);
        }
    } else {
        uint8_t *bytes = (uint8_t *)(uintptr_t)joined->data;
        memcpy(bytes, ls->data, ls->unit_len);
        memcpy(bytes + ls->unit_len, rs->data, rs->unit_len);
    }
    return kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)joined, total);
}

static int kiln_c99_strict_equal(kiln_value left, kiln_value right) {
    if (left.tag != right.tag) {
        return 0;
    }
    if (left.tag == KILN_VALUE_NUMBER) {
        double l = kiln_c99_as_number(left);
        double r = kiln_c99_as_number(right);
        return l == r;
    }
    if (left.tag == KILN_VALUE_BIGINT) {
        /* A BigInt is a primitive: two are equal when the numbers are, however they were built.
         * `1n === 1n` is true and the two handles are different allocations. */
        return kiln_bigint_compare((const void *)(uintptr_t)left.bits,
                                   (const void *)(uintptr_t)right.bits) == 0;
    }
    if (left.tag == KILN_VALUE_STRING) {
        const kiln_string *l = left.ptr == NULL ? NULL : (const kiln_string *)left.ptr;
        const kiln_string *r = right.ptr == NULL ? NULL : (const kiln_string *)right.ptr;
        return l != NULL && r != NULL && kiln_string_equal(l, r);
    }
    if (left.tag == KILN_VALUE_SYMBOL) {
        /* A registry symbol is identified by its id, not by the object that carries it: two
         * `Symbol.for(k)` calls are the same symbol. The id used to be the value's payload word,
         * which made this comparison free; now it lives on the symbol, so ask for it. A unique
         * symbol has id zero and falls through to object identity, which is what distinguishes it
         * from a registered one and from every other unique symbol. */
        const kiln_symbol *left_symbol = kiln_c99_symbol_of(left);
        const kiln_symbol *right_symbol = kiln_c99_symbol_of(right);
        if (left_symbol != NULL && right_symbol != NULL && left_symbol->registry_id != 0) {
            return left_symbol->registry_id == right_symbol->registry_id;
        }
    }
    return left.bits == right.bits;
}

static int kiln_c99_loose_equal(kiln_value left, kiln_value right) {
    if (left.tag == right.tag) {
        return kiln_c99_strict_equal(left, right);
    }
    if ((left.tag == KILN_VALUE_UNDEFINED || left.tag == KILN_VALUE_NULL) &&
        (right.tag == KILN_VALUE_UNDEFINED || right.tag == KILN_VALUE_NULL)) {
        return 1;
    }
    if (left.tag == KILN_VALUE_UNDEFINED || left.tag == KILN_VALUE_NULL ||
        right.tag == KILN_VALUE_UNDEFINED || right.tag == KILN_VALUE_NULL) {
        return 0;
    }
    if (left.tag == KILN_VALUE_BOOLEAN) {
        left = kiln_c99_number(left.bits != 0 ? 1.0 : 0.0);
    }
    if (right.tag == KILN_VALUE_BOOLEAN) {
        right = kiln_c99_number(right.bits != 0 ? 1.0 : 0.0);
    }
    if ((left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_STRING) ||
        (left.tag == KILN_VALUE_STRING && right.tag == KILN_VALUE_NUMBER)) {
        return kiln_c99_as_number(left) == kiln_c99_as_number(right);
    }
    /* `==` crosses the numeric types where `===` does not: `1n == 1` is true and `1n === 1` is
     * false, which is the whole reason both operators exist. */
    if (left.tag == KILN_VALUE_BIGINT || right.tag == KILN_VALUE_BIGINT) {
        if (left.tag == KILN_VALUE_NUMBER || right.tag == KILN_VALUE_NUMBER ||
            left.tag == KILN_VALUE_STRING || right.tag == KILN_VALUE_STRING) {
            return kiln_c99_as_number(left) == kiln_c99_as_number(right);
        }
    }
    return kiln_c99_strict_equal(left, right);
}

static uint32_t kiln_c99_to_uint32(double value) {
    double wrapped;
    if (!isfinite(value) || value == 0.0) {
        return UINT32_C(0);
    }
    wrapped = fmod(trunc(value), 4294967296.0);
    if (wrapped < 0.0) {
        wrapped += 4294967296.0;
    }
    return (uint32_t)wrapped;
}

static double kiln_c99_signed32(uint32_t value) {
    return value <= UINT32_C(0x7fffffff)
        ? (double)value
        : (double)value - 4294967296.0;
}

static uint32_t kiln_c99_shift_right(uint32_t value, uint32_t count) {
    count &= UINT32_C(31);
    if (count == 0 || (value & UINT32_C(0x80000000)) == 0) {
        return value >> count;
    }
    return (value >> count) | (~UINT32_C(0) << (UINT32_C(32) - count));
}

/* Decode the UTF-8 used at the C boundary into Unicode scalar values.  Generated constants are
 * valid UTF-8, and host strings are required to use the same representation.  Treating malformed
 * bytes as U+FFFD keeps comparison bounded and deterministic if a host violates that contract. */
static uint32_t kiln_c99_utf8_next(const char *text, size_t length, size_t *offset) {
    size_t index = *offset;
    uint32_t codepoint;
    unsigned char first;
    unsigned char second;
    unsigned char third;
    unsigned char fourth;
    if (index >= length || text == NULL) {
        return UINT32_MAX;
    }
    first = (unsigned char)text[index++];
    if (first < 0x80) {
        *offset = index;
        return first;
    }
    if (first >= 0xC2 && first <= 0xDF && index < length) {
        second = (unsigned char)text[index];
        if ((second & 0xC0) == 0x80) {
            *offset = index + 1;
            return ((uint32_t)(first & 0x1F) << 6) | (uint32_t)(second & 0x3F);
        }
    }
    if (first >= 0xE0 && first <= 0xEF && index + 1 < length) {
        second = (unsigned char)text[index];
        third = (unsigned char)text[index + 1];
        if ((second & 0xC0) == 0x80 && (third & 0xC0) == 0x80 &&
            (first != 0xE0 || second >= 0xA0) && (first != 0xED || second < 0xA0)) {
            *offset = index + 2;
            return ((uint32_t)(first & 0x0F) << 12) |
                   ((uint32_t)(second & 0x3F) << 6) | (uint32_t)(third & 0x3F);
        }
    }
    if (first >= 0xF0 && first <= 0xF4 && index + 2 < length) {
        second = (unsigned char)text[index];
        third = (unsigned char)text[index + 1];
        fourth = (unsigned char)text[index + 2];
        if ((second & 0xC0) == 0x80 && (third & 0xC0) == 0x80 &&
            (fourth & 0xC0) == 0x80 && (first != 0xF0 || second >= 0x90) &&
            (first != 0xF4 || second <= 0x8F)) {
            *offset = index + 3;
            codepoint = ((uint32_t)(first & 0x07) << 18) |
                        ((uint32_t)(second & 0x3F) << 12) |
                        ((uint32_t)(third & 0x3F) << 6) | (uint32_t)(fourth & 0x3F);
            return codepoint;
        }
    }
    /* Consume one byte, allowing the next byte to start a fresh sequence. */
    *offset = index;
    return UINT32_C(0xFFFD);
}

typedef struct kiln_c99_utf16_iter {
    const void *text;
    size_t length;
    size_t offset;
    uint16_t pending;
    int has_pending;
} kiln_c99_utf16_iter;

static int kiln_c99_utf16_next(kiln_c99_utf16_iter *iterator, uint16_t *unit) {
    const kiln_string *string;
    if (iterator->has_pending) {
        iterator->has_pending = 0;
        *unit = iterator->pending;
        return 1;
    }
    if (iterator->offset >= iterator->length || iterator->text == NULL) {
        return 0;
    }
    string = (const kiln_string *)iterator->text;
    *unit = kiln_string_unit_at(string, iterator->offset++);
    return 1;
}

/* Read one JavaScript UTF-16 code unit without materialising the complete unit array.  The
 * ASCII metadata is authoritative for the direct-byte case; every other string follows the
 * same iterator used by length, slicing, and the bulk string operations. */
static int kiln_c99_utf16_unit_at(kiln_value value, size_t index, uint16_t *unit) {
    const kiln_string *string;
    if (unit == NULL) return 0;
    if (value.ptr == NULL) return 0;
    string = (const kiln_string *)value.ptr;
    if (index >= string->unit_len) return 0;
    *unit = kiln_string_unit_at(string, index);
    return 1;
}

/* `length` in UTF-16 code units, which is what JavaScript counts and is neither bytes nor
 * characters. For ASCII text a code unit is a byte (ADR 0022), so `value.len` -- already known,
 * already paid for -- is the answer without walking a single byte. */
static size_t kiln_c99_string_length(kiln_value value) {
    return value.ptr == NULL ? 0 : ((const kiln_string *)value.ptr)->unit_len;
}
