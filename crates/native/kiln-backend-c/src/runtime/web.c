static int kiln_c99_entropy_word(kiln_host *host, uint64_t *out) {
#if KILN_REALM_HOST_ENTROPY
    kiln_value answer;
    if (host == NULL || host->call == NULL || out == NULL ||
        host->call(host->context, UINT16_C(3), UINT16_C(0), NULL, &answer) != 0 ||
        answer.tag != KILN_VALUE_NUMBER) {
        return KILN_C99_HOST_ERROR;
    }
    *out = answer.bits;
    return KILN_C99_OK;
#else
    (void)host;
    return kiln_realm_entropy_word(KILN_REALM_ENTROPY_SEED, out);
#endif
}

static int kiln_c99_random(kiln_host *host, double *out) {
    uint64_t word;
    int status = kiln_c99_entropy_word(host, &word);
    if (status != KILN_C99_OK || out == NULL) return status;
    /* 53 random bits, which is every bit the mantissa has. */
    *out = (double)(word >> 11) / 9007199254740992.0;
    return KILN_C99_OK;
}

/* One hex digit's value, or -1. Used by every unescaping path, none of which may accept the
 * `0x` prefixes or the locale-dependent classifications that `strtol` would. */
static int kiln_c99_hex_digit(char character) {
    if (character >= '0' && character <= '9') {
        return character - '0';
    }
    if (character >= 'a' && character <= 'f') {
        return character - 'a' + 10;
    }
    if (character >= 'A' && character <= 'F') {
        return character - 'A' + 10;
    }
    return -1;
}

static int kiln_c99_append_hex(kiln_text *text, unsigned value, int width) {
    static const char alphabet[] = "0123456789ABCDEF";
    char digits[4];
    int place;
    for (place = width - 1; place >= 0; --place) {
        digits[place] = alphabet[value & 0xFu];
        value >>= 4;
    }
    return kiln_c99_text_append(text, digits, (size_t)width);
}

/* Whether a byte span is well-formed UTF-8.
 *
 * `decodeURI` is the only place this is asked: percent-escapes name bytes, and a program can name
 * bytes that are not a string. Rust's `String::from_utf8` is what rejects them on the interpreter
 * side, and this is that check -- overlong forms, surrogates and out-of-range scalars included,
 * because accepting any of them would let the two engines hold different strings. */
static int kiln_c99_valid_utf8(const char *bytes, size_t len) {
    size_t at = 0;
    while (at < len) {
        unsigned char lead = (unsigned char)bytes[at];
        uint32_t codepoint;
        size_t extra;
        size_t index;
        if (lead < 0x80u) {
            at += 1;
            continue;
        }
        if (lead >= 0xC2u && lead <= 0xDFu) {
            extra = 1;
            codepoint = lead & 0x1Fu;
        } else if (lead >= 0xE0u && lead <= 0xEFu) {
            extra = 2;
            codepoint = lead & 0x0Fu;
        } else if (lead >= 0xF0u && lead <= 0xF4u) {
            extra = 3;
            codepoint = lead & 0x07u;
        } else {
            /* A continuation byte on its own, or an overlong two-byte lead. */
            return 0;
        }
        if (at + extra >= len) {
            /* The sequence runs off the end. */
            return 0;
        }
        for (index = 1; index <= extra; ++index) {
            unsigned char next = (unsigned char)bytes[at + index];
            if ((next & 0xC0u) != 0x80u) {
                return 0;
            }
            codepoint = (codepoint << 6) | (next & 0x3Fu);
        }
        if (codepoint > UINT32_C(0x10FFFF) ||
            (codepoint >= UINT32_C(0xD800) && codepoint <= UINT32_C(0xDFFF)) ||
            (extra == 2 && codepoint < UINT32_C(0x800)) ||
            (extra == 3 && codepoint < UINT32_C(0x10000))) {
            return 0;
        }
        at += extra + 1;
    }
    return 1;
}

static _Thread_local kiln_node_response *kiln_c99_active_http_response = NULL;

/* The prototypes the runtime bootstrap's `URL` and `URLSearchParams` classes carry, kept in
 * realm slots so the request fast path below can hand out an object that *is* a `URL` -- one
 * the class's getters answer for -- without the class constructing it. Set once per realm by
 * `__kilnUrlAdopt`, before user code. */
#define KILN_C99_REALM_SLOT_URL_PROTOTYPE UINT32_C(6)
#define KILN_C99_REALM_SLOT_URL_SEARCH_PARAMS_PROTOTYPE UINT32_C(7)
#define KILN_C99_REALM_SLOT_URL_CLASS UINT32_C(8)

/* `__kilnUrlAdopt(URL, URL.prototype, URLSearchParams.prototype)`. */
static int kiln_c99_url_adopt_raw(kiln_value *out, const kiln_value *args, size_t argc) {
    kiln_value *slot;
    int status;
    if (argc < 3 || args[1].tag != KILN_VALUE_OBJECT || args[2].tag != KILN_VALUE_OBJECT)
        return KILN_C99_INVALID_ARGUMENT;
    status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_URL_CLASS, &slot);
    if (status != KILN_C99_OK) return status;
    *slot = args[0];
    status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_URL_PROTOTYPE, &slot);
    if (status != KILN_C99_OK) return status;
    *slot = args[1];
    status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_URL_SEARCH_PARAMS_PROTOTYPE, &slot);
    if (status != KILN_C99_OK) return status;
    *slot = args[2];
    /* Not a member read: the intrinsic's own answer, which is nothing. */
    *out = kiln_c99_value(KILN_VALUE_UNDEFINED, 0, NULL, 0);
    return KILN_C99_OK;
}

/* What a program's `URL` names: the bootstrap's class, which ran before any module body. */
static int kiln_c99_url_class(kiln_value *out) {
    kiln_value *slot;
    int status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_URL_CLASS, &slot);
    if (status != KILN_C99_OK) return status;
    if (slot->tag != KILN_VALUE_FUNCTION) {
        KILN_C99_REFUSE("URL before the runtime bootstrap defined it");
    }
    *out = *slot;
    return KILN_C99_OK;
}

/* The `url` crate's answer, spilled into a buffer sized by asking. A record is at most a few
 * copies of its input plus an origin, so the first guess is generous and the retry is rare.
 *
 * Behind the URL retain group: a program that names neither `URL` nor anything built on it
 * links none of the crate -- the two provider symbols below are its only references -- and the
 * bootstrap that would call this is left out of that program too. */
#if KILN_RETAIN_GROUP_URL
static int kiln_c99_url_record(kiln_value *out, int set, const uint8_t *a, size_t a_len,
                               uint32_t field, const uint8_t *b, size_t b_len,
                               kiln_value *thrown) {
    uint64_t capacity = (uint64_t)(a_len + b_len) * 4 + 64;
    uint64_t len = 0;
    uint8_t *buffer = malloc((size_t)capacity);
    int status;
    if (buffer == NULL) return KILN_C99_OUT_OF_MEMORY;
    status = set ? kiln_url_set(a, (uint64_t)a_len, field, b, (uint64_t)b_len, buffer, capacity, &len)
                 : kiln_url_parse(a, (uint64_t)a_len, b, (uint64_t)b_len, buffer, capacity, &len);
    if (status == KILN_C99_INVALID_ARGUMENT && len > capacity) {
        uint8_t *bigger = realloc(buffer, (size_t)len);
        if (bigger == NULL) { free(buffer); return KILN_C99_OUT_OF_MEMORY; }
        buffer = bigger;
        capacity = len;
        status = set ? kiln_url_set(a, (uint64_t)a_len, field, b, (uint64_t)b_len, buffer, capacity, &len)
                     : kiln_url_parse(a, (uint64_t)a_len, b, (uint64_t)b_len, buffer, capacity, &len);
    }
    if (status == 2 /* URL_INVALID, the provider's own code for input that is not a URL */) {
        free(buffer);
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, "Invalid URL");
    }
    if (status != KILN_C99_OK) { free(buffer); return status; }
    *out = kiln_c99_copy_text((const char *)buffer, (size_t)len);
    free(buffer);
    return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
}
#else
static int kiln_c99_url_record(kiln_value *out, int set, const uint8_t *a, size_t a_len,
                               uint32_t field, const uint8_t *b, size_t b_len,
                               kiln_value *thrown) {
    (void)out; (void)set; (void)a; (void)a_len; (void)field; (void)b; (void)b_len; (void)thrown;
    KILN_C99_REFUSE("URL, which this executable did not retain");
}
#endif

/* `__kilnUrlParse(input, base)`: the eleven components of the URL, NUL-separated, from the
 * provider (ADR 0025, ADR 0063 rule 4). The `URL` class in the runtime bootstrap is the only
 * caller; it splits the record and holds the pieces. A string that is not a URL throws the
 * class's `TypeError` from here. */
static int kiln_c99_url_parse_raw(kiln_value *out, const kiln_value *args, size_t argc,
                                  kiln_value *thrown) {
    kiln_value input = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
    kiln_value base = argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED ? kiln_c99_text(args[1])
                                                                         : kiln_c99_static_text("", 0);
    uint8_t *input_owned = NULL;
    uint8_t *base_owned = NULL;
    size_t input_len = 0;
    size_t base_len = 0;
    int status;
    if (input.tag != KILN_VALUE_STRING || base.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    if (kiln_c99_string_to_utf8_lossy(input, &input_owned, &input_len) != KILN_C99_OK ||
        kiln_c99_string_to_utf8_lossy(base, &base_owned, &base_len) != KILN_C99_OK) {
        free(input_owned);
        free(base_owned);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_url_record(out, 0, input_owned == NULL ? (const uint8_t *)"" : input_owned,
                                 input_len, 0, base_owned == NULL ? (const uint8_t *)"" : base_owned,
                                 base_len, thrown);
    free(input_owned);
    free(base_owned);
    return status;
}

/* `__kilnUrlSet(href, field, value)`: the record after one setter, from the same crate. */
static int kiln_c99_url_set_raw(kiln_value *out, const kiln_value *args, size_t argc,
                                kiln_value *thrown) {
    kiln_value href = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
    kiln_value value = kiln_c99_text(argc > 2 ? args[2] : kiln_c99_undefined());
    uint32_t field = argc > 1 ? (uint32_t)kiln_c99_as_number(args[1]) : 0;
    uint8_t *href_owned = NULL;
    uint8_t *value_owned = NULL;
    size_t href_len = 0;
    size_t value_len = 0;
    int status;
    if (href.tag != KILN_VALUE_STRING || value.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    if (kiln_c99_string_to_utf8_lossy(href, &href_owned, &href_len) != KILN_C99_OK ||
        kiln_c99_string_to_utf8_lossy(value, &value_owned, &value_len) != KILN_C99_OK) {
        free(href_owned);
        free(value_owned);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_url_record(out, 1, href_owned == NULL ? (const uint8_t *)"" : href_owned,
                                 href_len, field, value_owned == NULL ? (const uint8_t *)"" : value_owned,
                                 value_len, thrown);
    free(href_owned);
    free(value_owned);
    return status;
}

/* `__kilnUrlFast(input, base)`: the request fast path (plan step B.3). Inside a `node:http`
 * handler, the first `new URL(req.url, ...)` is answered with an object the runtime built once
 * for this response and refills per request -- `pathname` and a `searchParams` whose `get` reads
 * the raw query -- so a handler that touches the URL lightly allocates nothing. The object
 * carries the bootstrap's `URL.prototype`, and `_input`/`_base` for the class's other getters
 * to parse from on first use; its `searchParams` carries `URLSearchParams.prototype` and
 * `_query` the same way. Anything else answers `undefined` and the class parses. */
static int kiln_c99_url_fast_raw(kiln_value *out, const kiln_value *args, size_t argc,
                                 kiln_value *thrown) {
    kiln_value input = argc > 0 ? args[0] : kiln_c99_undefined();
    kiln_value base = argc > 1 ? args[1] : kiln_c99_undefined();
    kiln_node_response *resp = kiln_c99_active_http_response;
    uint8_t *input_owned = NULL;
    size_t input_len = 0;
    const char *input_bytes;
    const char *start;
    const char *end;
    const char *query;
    const char *fragment;
    int status;
    (void)thrown;
    /* Not a member read: "no fast object for this call" is the intrinsic's own answer, and the
     * class then parses. */
    *out = kiln_c99_value(KILN_VALUE_UNDEFINED, 0, NULL, 0);
    if (resp == NULL || resp->url_used || input.tag != KILN_VALUE_STRING) return KILN_C99_OK;
    if (kiln_c99_string_to_utf8_lossy(input, &input_owned, &input_len) != KILN_C99_OK)
        return KILN_C99_OUT_OF_MEMORY;
    input_bytes = input_len == 0 ? "" : (const char *)input_owned;
    if (resp->target_ptr == NULL || resp->target_len != input_len ||
        memcmp(resp->target_ptr, input_bytes, input_len) != 0) {
        free(input_owned);
        return KILN_C99_OK;
    }
#define KILN_WEB_RETURN(result) do { free(input_owned); return (result); } while (0)
    resp->url_used = 1;
    start = input_len == 0 ? "/" : input_bytes;
    end = input_bytes + input_len;
    if (input_len == 0) end = start + 1;
    query = memchr(start, '?', (size_t)(end - start));
    fragment = memchr(start, '#', (size_t)(end - start));
    if (fragment != NULL && (query == NULL || fragment < query)) query = NULL;
    if (resp->url_object.tag != KILN_VALUE_OBJECT) {
        kiln_value *proto;
        status = kiln_c99_new_object_with_capacity_tenured(&resp->url_object, 8);
        if (status == KILN_C99_OK)
            kiln_c99_candidate_record_pointer_store(kiln_c99_gc_heap(), resp, resp->url_object.ptr);
        if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
        status = kiln_c99_new_object_with_capacity_tenured(&resp->search_params_object, 6);
        if (status == KILN_C99_OK)
            kiln_c99_candidate_record_pointer_store(kiln_c99_gc_heap(), resp, resp->search_params_object.ptr);
        if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
        status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_URL_PROTOTYPE, &proto);
        if (status == KILN_C99_OK && proto->tag == KILN_VALUE_OBJECT)
            status = kiln_c99_set_prototype(resp->url_object, *proto);
        if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
        status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_URL_SEARCH_PARAMS_PROTOTYPE, &proto);
        if (status == KILN_C99_OK && proto->tag == KILN_VALUE_OBJECT)
            status = kiln_c99_set_prototype(resp->search_params_object, *proto);
        if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
        status = kiln_c99_put_hot_key(NULL, resp->search_params_object, KILN_C99_HOT_KEY_GET,
                                      kiln_c99_method(KILN_C99_METHOD_URL_SEARCH_PARAMS_GET), 0, NULL);
        if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
        status = kiln_c99_put_hot_key(NULL, resp->url_object, KILN_C99_HOT_KEY_SEARCH_PARAMS,
                                      resp->search_params_object, 0, NULL);
        if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
        status = kiln_c99_put(NULL, resp->url_object, "@@url", 5, kiln_c99_boolean(1), 0, NULL);
        if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
    }
    {
        size_t path_len = (size_t)((query != NULL ? query : (fragment != NULL ? fragment : end)) - start);
        kiln_c99_heap_store_value(resp, &resp->pathname_val, kiln_c99_copy_text(start, path_len));
        if (resp->pathname_val.tag != KILN_VALUE_STRING) KILN_WEB_RETURN(KILN_C99_OUT_OF_MEMORY);
        resp->pathname_cap = path_len + 1;
        /* Own data, not a put: the object's prototype is the class's, whose `pathname` is a
         * setter, and a put would run it. */
        status = kiln_c99_own_data_hot_key(resp->url_object, KILN_C99_HOT_KEY_PATHNAME,
                                           resp->pathname_val);
        if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
    }
    {
        size_t q_len = query != NULL ? (size_t)((fragment != NULL ? fragment : end) - query - 1) : 0;
        kiln_value query_val;
        if (query == NULL || q_len == 0) {
            query_val = kiln_c99_static_text("", 0);
        } else {
            kiln_c99_heap_store_value(resp, &resp->query_val, kiln_c99_copy_text(query + 1, q_len));
            if (resp->query_val.tag != KILN_VALUE_STRING) KILN_WEB_RETURN(KILN_C99_OUT_OF_MEMORY);
            resp->query_cap = q_len + 1;
            query_val = resp->query_val;
        }
        status = kiln_c99_own_data(resp->search_params_object, "@@url:query", 11, query_val);
        if (status == KILN_C99_OK)
            status = kiln_c99_own_data(resp->search_params_object, "_query", 6, query_val);
        if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
    }
    /* The class's other getters parse from these on first use; the values already exist. The
     * objects outlive one request -- a keep-alive socket's response is refilled -- and the class
     * notices: its cached state records the `_input` and `_query` it was built from. */
    status = kiln_c99_own_data(resp->url_object, "_input", 6, input);
    if (status == KILN_C99_OK) status = kiln_c99_own_data(resp->url_object, "_base", 5, base);
    if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
    *out = resp->url_object;
    KILN_WEB_RETURN(KILN_C99_OK);
#undef KILN_WEB_RETURN
}

static int kiln_c99_url_search_params_get(kiln_value *out, kiln_value receiver,
                                          const kiln_value *args, size_t argc,
                                          kiln_value *thrown) {
    kiln_value query;
    kiln_value name = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
    uint8_t *query_owned = NULL;
    uint8_t *name_owned = NULL;
    static const char empty[] = "";
    size_t at = 0;
    int status;
    if (receiver.tag != KILN_VALUE_OBJECT || name.tag != KILN_VALUE_STRING) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "URLSearchParams.get called on an incompatible receiver");
    }
    status = kiln_c99_get(NULL, receiver, "@@url:query", 11, &query, 0, NULL);
    if (status != KILN_C99_OK || query.tag != KILN_VALUE_STRING)
        KILN_C99_REFUSE("URLSearchParams with no captured query string");
    size_t query_len = 0;
    size_t name_len = 0;
    if (kiln_c99_string_to_utf8_lossy(query, &query_owned, &query_len) != KILN_C99_OK ||
        kiln_c99_string_to_utf8_lossy(name, &name_owned, &name_len) != KILN_C99_OK) {
        free(query_owned);
        free(name_owned);
        return KILN_C99_OUT_OF_MEMORY;
    }
    const char *query_bytes = query_len == 0 ? empty : (const char *)query_owned;
    const char *name_bytes = name_len == 0 ? empty : (const char *)name_owned;
#define KILN_QUERY_RETURN(result) do { \
        free(query_owned); \
        free(name_owned); \
        return (result); \
    } while (0)
    while (at <= query_len) {
        size_t pair_end = at;
        size_t equal;
        while (pair_end < query_len && query_bytes[pair_end] != '&') ++pair_end;
        equal = at;
        while (equal < pair_end && query_bytes[equal] != '=') ++equal;
        if (equal - at == name_len && memcmp(query_bytes + at, name_bytes, name_len) == 0) {
            const char *val_ptr = equal < pair_end ? query_bytes + equal + 1 : "";
            size_t val_len = equal < pair_end ? pair_end - equal - 1 : 0;
            if (val_len == 0) {
                *out = kiln_c99_static_text("", 0);
                KILN_QUERY_RETURN(KILN_C99_OK);
            }
            if (memchr(val_ptr, '%', val_len) == NULL) {
                kiln_node_response *resp = kiln_c99_active_http_response;
                if (resp != NULL) {
                    kiln_c99_heap_store_value(resp, &resp->param_val, kiln_c99_copy_text(val_ptr, val_len));
                    if (resp->param_val.tag != KILN_VALUE_STRING)
                        KILN_QUERY_RETURN(KILN_C99_OUT_OF_MEMORY);
                    resp->param_cap = val_len + 1;
                    *out = resp->param_val;
                    KILN_QUERY_RETURN(KILN_C99_OK);
                }
                *out = kiln_c99_copy_text(val_ptr, val_len);
                KILN_QUERY_RETURN(KILN_C99_OK);
            }
            kiln_value encoded = kiln_c99_copy_text(val_ptr, val_len);
            const kiln_value decode_args[1] = {encoded};
            KILN_QUERY_RETURN(kiln_c99_invoke_method(NULL, out, KILN_C99_METHOD_DECODE_URI_COMPONENT,
                                                     kiln_c99_undefined(), decode_args, 1, 0, thrown));
        }
        if (pair_end == query_len) break;
        at = pair_end + 1;
    }
    *out = kiln_c99_null();
#undef KILN_QUERY_RETURN
    return KILN_C99_OK;
}



/* `Number.prototype.toFixed`, rounded on the value's *exact* decimal expansion.
 *
 * Not the printf `%.*f` conversion: it reads the process locale's decimal point, and an embedded
 * module runs in whatever locale its host set. The same objection is why a number's shortest
 * spelling comes from the number provider (ADR 0018); this is that rule applied to the other
 * direction.
 *
 * Not `floor(value * 10^f + 0.5)` either, which is the obvious arithmetic and is wrong:
 * `(1.45).toFixed(1)` is `"1.4"` because 1.45 is really 1.44999..., and multiplying by ten rounds
 * that up to exactly 14.5 before any rounding decision is made. The digits that decide have to be
 * the value's own.
 *
 * So the expansion is computed exactly. A binary64 is `m * 2^e` with `m` under 2^53, and for
 * negative `e` that is `m * 5^(-e)` with the point `-e` digits from the right -- an exact integer
 * a decimal digit array can hold, and repeated multiplication by five is the whole of the
 * arithmetic. 5^1074 is 751 digits, `m` adds sixteen, and values at or above 1e21 are handed back
 * to `ToString` by the specification, so the array is bounded and so is the loop. */
