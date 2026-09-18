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

static kiln_node_response *kiln_c99_active_http_response = NULL;

/* The URL subset shared with the interpreter today: absolute HTTP-like input, or a path against
 * an absolute base; pathname and query lookup are the public state. Keeping that advertised
 * subset identical is better than a broader native parser that disagrees in corners. */
static int kiln_c99_url_construct(kiln_value *out, const kiln_value *args, size_t argc,
                                  kiln_value *thrown) {
    kiln_value input = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
    kiln_value base = kiln_c99_text(argc > 1 ? args[1] : kiln_c99_undefined());
    const char *start;
    const char *end;
    const char *query;
    const char *fragment;
    const char *scheme;
    kiln_value params;
    uint8_t *input_owned = NULL;
    uint8_t *base_owned = NULL;
    static const char empty[] = "";
    int status;
    if (input.tag != KILN_VALUE_STRING || base.tag != KILN_VALUE_STRING) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    size_t input_len = 0;
    size_t base_len = 0;
    if (kiln_c99_string_to_utf8_lossy(input, &input_owned, &input_len) != KILN_C99_OK ||
        kiln_c99_string_to_utf8_lossy(base, &base_owned, &base_len) != KILN_C99_OK) {
        free(input_owned);
        free(base_owned);
        return KILN_C99_OUT_OF_MEMORY;
    }
    const char *input_bytes = input_len == 0 ? empty : (const char *)input_owned;
    const char *base_bytes = base_len == 0 ? empty : (const char *)base_owned;
#define KILN_WEB_RETURN(result) do { \
        free(input_owned); \
        free(base_owned); \
        return (result); \
    } while (0)
    scheme = NULL;
    {
        size_t index;
        for (index = 0; index + 2 < input_len; ++index) {
            if (input_bytes[index] == ':' && input_bytes[index + 1] == '/' &&
                input_bytes[index + 2] == '/') {
                scheme = input_bytes + index;
                break;
            }
        }
    }
    if (scheme != NULL) {
        start = scheme + 3;
        end = input_bytes + input_len;
        while (start < end && *start != '/' && *start != '?' && *start != '#') ++start;
        if (start == end || *start != '/') start = "/";
    } else {
        int valid_base = 0;
        size_t index;
        for (index = 0; index + 2 < base_len; ++index) {
            if (base_bytes[index] == ':' && base_bytes[index + 1] == '/' &&
                base_bytes[index + 2] == '/') {
                valid_base = 1;
                break;
            }
        }
        if (!valid_base) {
            KILN_WEB_RETURN(kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                 "Invalid base URL"));
        }
        start = input_len == 0 ? "/" : input_bytes;
        end = input_bytes + input_len;
    }
    if (start == input_bytes || (start >= input_bytes && start <= input_bytes + input_len)) {
        end = input_bytes + input_len;
    } else {
        end = start + 1;
    }
    query = memchr(start, '?', (size_t)(end - start));
    fragment = memchr(start, '#', (size_t)(end - start));
    if (fragment != NULL && (query == NULL || fragment < query)) query = NULL;

    if (kiln_c99_active_http_response != NULL && !kiln_c99_active_http_response->url_used) {
        kiln_node_response *resp = kiln_c99_active_http_response;
        if (resp->target_ptr != NULL && resp->target_len == input_len &&
            memcmp(resp->target_ptr, input_bytes, input_len) == 0) {
            resp->url_used = 1;
            if (resp->url_object.tag != KILN_VALUE_OBJECT) {
                status = kiln_c99_new_object_with_capacity_tenured(&resp->url_object, 6);
                if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
                status = kiln_c99_new_object_with_capacity_tenured(&resp->search_params_object, 4);
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
            size_t path_len = (size_t)((query != NULL ? query :
                                (fragment != NULL ? fragment : end)) - start);
            resp->pathname_val = kiln_c99_copy_text(start, path_len);
            if (resp->pathname_val.tag != KILN_VALUE_STRING) KILN_WEB_RETURN(KILN_C99_OUT_OF_MEMORY);
            resp->pathname_cap = path_len + 1;
            status = kiln_c99_put_hot_key(NULL, resp->url_object, KILN_C99_HOT_KEY_PATHNAME,
                                          resp->pathname_val, 0, NULL);
            if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
            size_t q_len = query != NULL ? (size_t)((fragment != NULL ? fragment : end) - query - 1) : 0;
            if (query == NULL || q_len == 0) {
                status = kiln_c99_put(NULL, resp->search_params_object, "@@url:query", 11,
                                      kiln_c99_static_text("", 0), 0, NULL);
            } else {
                resp->query_val = kiln_c99_copy_text(query + 1, q_len);
                if (resp->query_val.tag != KILN_VALUE_STRING) KILN_WEB_RETURN(KILN_C99_OUT_OF_MEMORY);
                resp->query_cap = q_len + 1;
                status = kiln_c99_put(NULL, resp->search_params_object, "@@url:query", 11,
                                      resp->query_val, 0, NULL);
            }
            if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
            *out = resp->url_object;
            KILN_WEB_RETURN(KILN_C99_OK);
        }
    }

    status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
    status = kiln_c99_put_hot_key(NULL, *out, KILN_C99_HOT_KEY_PATHNAME,
                          kiln_c99_copy_text(start, (size_t)((query != NULL ? query :
                              (fragment != NULL ? fragment : end)) - start)), 0, NULL);
    if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
    status = kiln_c99_new_object(&params);
    if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
    status = kiln_c99_put(NULL, params, "@@url:query", 11,
                          query == NULL ? kiln_c99_copy_text("", 0) :
                          kiln_c99_copy_text(query + 1, (size_t)((fragment != NULL ? fragment : end) - query - 1)),
                          0, NULL);
    if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
    status = kiln_c99_put_hot_key(NULL, params, KILN_C99_HOT_KEY_GET,
                          kiln_c99_method(KILN_C99_METHOD_URL_SEARCH_PARAMS_GET), 0, NULL);
    if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
    status = kiln_c99_put_hot_key(NULL, *out, KILN_C99_HOT_KEY_SEARCH_PARAMS, params, 0, NULL);
    if (status != KILN_C99_OK) KILN_WEB_RETURN(status);
    KILN_WEB_RETURN(kiln_c99_put(NULL, *out, "@@url", 5, kiln_c99_boolean(1), 0, NULL));
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
                    resp->param_val = kiln_c99_copy_text(val_ptr, val_len);
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
