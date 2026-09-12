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
    int status;
    if (input.tag != KILN_VALUE_STRING || base.tag != KILN_VALUE_STRING) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    scheme = NULL;
    {
        size_t index;
        for (index = 0; index + 2 < input.len; ++index) {
            if (input.ptr[index] == ':' && input.ptr[index + 1] == '/' &&
                input.ptr[index + 2] == '/') {
                scheme = input.ptr + index;
                break;
            }
        }
    }
    if (scheme != NULL) {
        start = scheme + 3;
        end = input.ptr + input.len;
        while (start < end && *start != '/' && *start != '?' && *start != '#') ++start;
        if (start == end || *start != '/') start = "/";
    } else {
        int valid_base = 0;
        size_t index;
        for (index = 0; index + 2 < base.len; ++index) {
            if (base.ptr[index] == ':' && base.ptr[index + 1] == '/' &&
                base.ptr[index + 2] == '/') {
                valid_base = 1;
                break;
            }
        }
        if (!valid_base) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, "Invalid base URL");
        }
        start = input.len == 0 ? "/" : input.ptr;
        end = input.ptr + input.len;
    }
    if (start == input.ptr || (start >= input.ptr && start <= input.ptr + input.len)) {
        end = input.ptr + input.len;
    } else {
        end = start + 1;
    }
    query = memchr(start, '?', (size_t)(end - start));
    fragment = memchr(start, '#', (size_t)(end - start));
    if (fragment != NULL && (query == NULL || fragment < query)) query = NULL;

    if (kiln_c99_active_http_response != NULL && !kiln_c99_active_http_response->url_used) {
        kiln_node_response *resp = kiln_c99_active_http_response;
        if (resp->target_ptr != NULL && resp->target_len == input.len &&
            memcmp(resp->target_ptr, input.ptr, input.len) == 0) {
            resp->url_used = 1;
            if (resp->url_object.tag != KILN_VALUE_OBJECT) {
                status = kiln_c99_new_object_with_capacity_tenured(&resp->url_object, 6);
                if (status != KILN_C99_OK) return status;
                status = kiln_c99_new_object_with_capacity_tenured(&resp->search_params_object, 4);
                if (status != KILN_C99_OK) return status;
                status = kiln_c99_put_hot_key(NULL, resp->search_params_object, KILN_C99_HOT_KEY_GET,
                                              kiln_c99_method(KILN_C99_METHOD_URL_SEARCH_PARAMS_GET), 0, NULL);
                if (status != KILN_C99_OK) return status;
                status = kiln_c99_put_hot_key(NULL, resp->url_object, KILN_C99_HOT_KEY_SEARCH_PARAMS,
                                              resp->search_params_object, 0, NULL);
                if (status != KILN_C99_OK) return status;
                status = kiln_c99_put(NULL, resp->url_object, "@@url", 5, kiln_c99_boolean(1), 0, NULL);
                if (status != KILN_C99_OK) return status;
            }
            size_t path_len = (size_t)((query != NULL ? query :
                                (fragment != NULL ? fragment : end)) - start);
            if (resp->pathname_val.tag != KILN_VALUE_STRING || resp->pathname_val.ptr == NULL ||
                path_len + 1 > resp->pathname_cap) {
                size_t cap = path_len < 128 ? 128 : path_len + 128;
                char *buf = kiln_c99_gc_alloc_text_tenured(cap);
                if (buf == NULL) return KILN_C99_OUT_OF_MEMORY;
                resp->pathname_val = kiln_c99_value(KILN_VALUE_STRING, 0, buf, (uint32_t)path_len);
                resp->pathname_cap = cap;
            }
            char *pbuf = (char *)resp->pathname_val.ptr;
            memcpy(pbuf, start, path_len);
            pbuf[path_len] = '\0';
            kiln_gc_heap *heap = kiln_c99_gc_heap();
            kiln_gc_block *block = heap != NULL ? kiln_c99_gc_lookup(heap, pbuf) : NULL;
            if (block != NULL) {
                block->size = path_len;
            }
            resp->pathname_val.len = (uint32_t)path_len;
            status = kiln_c99_put_hot_key(NULL, resp->url_object, KILN_C99_HOT_KEY_PATHNAME,
                                          resp->pathname_val, 0, NULL);
            if (status != KILN_C99_OK) return status;
            size_t q_len = query != NULL ? (size_t)((fragment != NULL ? fragment : end) - query - 1) : 0;
            if (query == NULL || q_len == 0) {
                status = kiln_c99_put(NULL, resp->search_params_object, "@@url:query", 11,
                                      kiln_c99_static_text("", 0), 0, NULL);
            } else {
                if (resp->query_val.tag != KILN_VALUE_STRING || resp->query_val.ptr == NULL ||
                    q_len + 1 > resp->query_cap) {
                    size_t cap = q_len < 64 ? 64 : q_len + 64;
                    char *buf = kiln_c99_gc_alloc_text_tenured(cap);
                    if (buf == NULL) return KILN_C99_OUT_OF_MEMORY;
                    resp->query_val = kiln_c99_value(KILN_VALUE_STRING, 0, buf, (uint32_t)q_len);
                    resp->query_cap = cap;
                }
                char *qbuf = (char *)resp->query_val.ptr;
                memcpy(qbuf, query + 1, q_len);
                qbuf[q_len] = '\0';
                kiln_gc_block *qblock = heap != NULL ? kiln_c99_gc_lookup(heap, qbuf) : NULL;
                if (qblock != NULL) {
                    qblock->size = q_len;
                }
                resp->query_val.len = (uint32_t)q_len;
                status = kiln_c99_put(NULL, resp->search_params_object, "@@url:query", 11,
                                      resp->query_val, 0, NULL);
            }
            if (status != KILN_C99_OK) return status;
            *out = resp->url_object;
            return KILN_C99_OK;
        }
    }

    status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put_hot_key(NULL, *out, KILN_C99_HOT_KEY_PATHNAME,
                          kiln_c99_copy_text(start, (size_t)((query != NULL ? query :
                              (fragment != NULL ? fragment : end)) - start)), 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_new_object(&params);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(NULL, params, "@@url:query", 11,
                          query == NULL ? kiln_c99_copy_text("", 0) :
                          kiln_c99_copy_text(query + 1, (size_t)((fragment != NULL ? fragment : end) - query - 1)),
                          0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put_hot_key(NULL, params, KILN_C99_HOT_KEY_GET,
                          kiln_c99_method(KILN_C99_METHOD_URL_SEARCH_PARAMS_GET), 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put_hot_key(NULL, *out, KILN_C99_HOT_KEY_SEARCH_PARAMS, params, 0, NULL);
    if (status != KILN_C99_OK) return status;
    return kiln_c99_put(NULL, *out, "@@url", 5, kiln_c99_boolean(1), 0, NULL);
}

static int kiln_c99_url_search_params_get(kiln_value *out, kiln_value receiver,
                                          const kiln_value *args, size_t argc,
                                          kiln_value *thrown) {
    kiln_value query;
    kiln_value name = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
    size_t at = 0;
    int status;
    if (receiver.tag != KILN_VALUE_OBJECT || name.tag != KILN_VALUE_STRING) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "URLSearchParams.get called on an incompatible receiver");
    }
    status = kiln_c99_get(NULL, receiver, "@@url:query", 11, &query, 0, NULL);
    if (status != KILN_C99_OK || query.tag != KILN_VALUE_STRING)
        KILN_C99_REFUSE("URLSearchParams with no captured query string");
    while (at <= query.len) {
        size_t pair_end = at;
        size_t equal;
        while (pair_end < query.len && query.ptr[pair_end] != '&') ++pair_end;
        equal = at;
        while (equal < pair_end && query.ptr[equal] != '=') ++equal;
        if (equal - at == name.len && memcmp(query.ptr + at, name.ptr, name.len) == 0) {
            const char *val_ptr = equal < pair_end ? query.ptr + equal + 1 : "";
            size_t val_len = equal < pair_end ? pair_end - equal - 1 : 0;
            if (val_len == 0) {
                *out = kiln_c99_static_text("", 0);
                return KILN_C99_OK;
            }
            if (memchr(val_ptr, '%', val_len) == NULL) {
                kiln_node_response *resp = kiln_c99_active_http_response;
                if (resp != NULL) {
                    if (resp->param_val.tag != KILN_VALUE_STRING || resp->param_val.ptr == NULL ||
                        val_len + 1 > resp->param_cap) {
                        size_t cap = val_len < 64 ? 64 : val_len + 64;
                        char *buf = kiln_c99_gc_alloc_text_tenured(cap);
                        if (buf == NULL) return KILN_C99_OUT_OF_MEMORY;
                        resp->param_val = kiln_c99_value(KILN_VALUE_STRING, 0, buf, (uint32_t)val_len);
                        resp->param_cap = cap;
                    }
                    char *pbuf = (char *)resp->param_val.ptr;
                    memcpy(pbuf, val_ptr, val_len);
                    pbuf[val_len] = '\0';
                    kiln_gc_heap *heap = kiln_c99_gc_heap();
                    kiln_gc_block *block = heap != NULL ? kiln_c99_gc_lookup(heap, pbuf) : NULL;
                    if (block != NULL) {
                        block->size = val_len;
                    }
                    resp->param_val.len = (uint32_t)val_len;
                    *out = resp->param_val;
                    return KILN_C99_OK;
                }
                *out = kiln_c99_copy_text(val_ptr, val_len);
                return KILN_C99_OK;
            }
            kiln_value encoded = kiln_c99_copy_text(val_ptr, val_len);
            const kiln_value decode_args[1] = {encoded};
            return kiln_c99_invoke_method(NULL, out, KILN_C99_METHOD_DECODE_URI_COMPONENT,
                                          kiln_c99_undefined(), decode_args, 1, 0, thrown);
        }
        if (pair_end == query.len) break;
        at = pair_end + 1;
    }
    *out = kiln_c99_null();
    return KILN_C99_OK;
}

/* Decode one application/x-www-form-urlencoded component. Unlike decodeURIComponent, malformed
 * percent escapes remain literal and `+` names a space. Query strings are deliberately forgiving:
 * one bad escape must not make the whole request unreadable. */
static int kiln_c99_form_decode(const char *bytes, size_t len, kiln_value *out) {
    kiln_text decoded = {NULL, 0, 0};
    size_t index = 0;
    int status;
    while (index < len) {
        char byte = bytes[index];
        if (byte == '+') {
            byte = ' ';
        } else if (byte == '%' && index + 2 < len) {
            int high = kiln_c99_hex_digit(bytes[index + 1]);
            int low = kiln_c99_hex_digit(bytes[index + 2]);
            if (high >= 0 && low >= 0) {
                byte = (char)((high << 4) | low);
                index += 2;
            }
        }
        status = kiln_c99_text_append(&decoded, &byte, 1);
        if (status != KILN_C99_OK) {
            free(decoded.bytes);
            return status;
        }
        index += 1;
    }
    if (kiln_c99_valid_utf8(decoded.bytes, decoded.len)) {
        *out = kiln_c99_text_value(&decoded);
        return KILN_C99_OK;
    }
    {
        kiln_value raw = kiln_c99_value(KILN_VALUE_STRING, 0, decoded.bytes, decoded.len);
        uint16_t *units = NULL;
        size_t count = 0;
        status = kiln_c99_to_units(raw, &units, &count);
        if (status == KILN_C99_OK) {
            *out = kiln_c99_from_units(units, count);
            if (out->tag != KILN_VALUE_STRING) status = KILN_C99_OUT_OF_MEMORY;
        }
        free(units);
        free(decoded.bytes);
        return status;
    }
}

/* The interpreter-first querystring surface: parse/decode form data into an own-property object,
 * preserving the first `=` and collecting repeated names in encounter order. Definition-style
 * insertion is intentional: a decoded `__proto__` is data, not a request to relink the result. */
static int kiln_c99_node_querystring_parse(kiln_value *out, const kiln_value *args, size_t argc) {
    kiln_value input = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_copy_text("", 0);
    size_t at = 0;
    int status;
    if (input.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) return status;
    while (at < input.len) {
        size_t pair_end = at;
        size_t equal;
        kiln_value name;
        kiln_value value;
        kiln_object *object;
        kiln_property *property = NULL;
        size_t index;
        while (pair_end < input.len && input.ptr[pair_end] != '&') pair_end += 1;
        if (pair_end == at) {
            at = pair_end + 1;
            continue;
        }
        equal = at;
        while (equal < pair_end && input.ptr[equal] != '=') equal += 1;
        status = kiln_c99_form_decode(input.ptr + at, equal - at, &name);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_form_decode(equal < pair_end ? input.ptr + equal + 1 : "",
                                      equal < pair_end ? pair_end - equal - 1 : 0, &value);
        if (status != KILN_C99_OK) {
            kiln_c99_gc_release((void *)name.ptr);
            return status;
        }
        object = kiln_c99_object_of(*out);
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
        }
        for (index = 0; index < object->count; ++index) {
            if (kiln_c99_same_key(&object->properties[index], name.ptr, name.len)) {
                property = &object->properties[index];
                break;
            }
        }
        if (property == NULL) {
            status = kiln_c99_own_slot(*out, name.ptr, name.len, &property);
            if (status == KILN_C99_OK) property->value = value;
        } else {
            kiln_value previous = property->value;
            kiln_c99_gc_release((void *)name.ptr);
            if (previous.tag == KILN_VALUE_ARRAY) {
                status = kiln_c99_array_push(previous, value);
            } else {
                kiln_value values;
                status = kiln_c99_new_array(&values);
                if (status == KILN_C99_OK) status = kiln_c99_array_push(values, previous);
                if (status == KILN_C99_OK) status = kiln_c99_array_push(values, value);
                if (status == KILN_C99_OK) property->value = values;
            }
        }
        if (status != KILN_C99_OK) return status;
        at = pair_end + 1;
    }
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
