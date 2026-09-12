
static kiln_regexp *kiln_c99_regexp_of(kiln_value value) {
    return (kiln_regexp *)(uintptr_t)value.bits;
}

/* Compile a pattern, or report that the engine rejected it.
 *
 * A rejection is a SyntaxError rather than a refusal, because that is what `new RegExp` with the
 * same text produces -- and a literal the frontend parsed can still be one the engine will not
 * take, so the program has to be able to catch it. */
static int kiln_c99_new_regexp(kiln_value *out, const char *source, size_t source_len,
                               const char *flags, size_t flags_len, kiln_value *thrown) {
    kiln_regexp *regexp;
    void *handle = NULL;
    size_t index;
    if (kiln_regexp_compile((const unsigned char *)source, source_len,
                            (const unsigned char *)flags, flags_len, &handle) != 0) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                    "Invalid regular expression");
    }
    regexp = (kiln_regexp *)kiln_c99_gc_alloc(KILN_GC_REGEXP, sizeof *regexp);
    if (regexp == NULL) {
        kiln_regexp_free(handle);
        return KILN_C99_OUT_OF_MEMORY;
    }
    regexp->handle = handle;
    regexp->matcher = NULL;
    regexp->matcher_ascii = NULL;
    regexp->source = source;
    regexp->source_len = source_len;
    regexp->flags = flags;
    regexp->flags_len = flags_len;
    regexp->global = 0;
    regexp->sticky = 0;
    regexp->last_index = 0.0;
    regexp->cached_subject_ptr = NULL;
    regexp->cached_subject_len = 0;
    regexp->cached_is_ascii = 0;
    regexp->cached_group_count = 0;
    for (index = 0; index < flags_len; ++index) {
        if (flags[index] == 'g') {
            regexp->global = 1;
        } else if (flags[index] == 'y') {
            regexp->sticky = 1;
        }
    }
    *out = kiln_c99_value(KILN_VALUE_REGEXP, (uint64_t)(uintptr_t)regexp, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_new_regexp_matcher(kiln_value *out, const char *source, size_t source_len,
                                       const char *flags, size_t flags_len,
                                       int (*matcher)(const uint16_t *units, size_t count, size_t start,
                                                      int64_t *ranges, size_t capacity, size_t *groups,
                                                      int *matched),
                                       int (*matcher_ascii)(const uint8_t *bytes, size_t count, size_t start,
                                                            int64_t *ranges, size_t capacity, size_t *groups,
                                                            int *matched),
                                       kiln_value *thrown) {
    kiln_regexp *regexp;
    size_t index;
    if (matcher == NULL) {
        return kiln_c99_new_regexp(out, source, source_len, flags, flags_len, thrown);
    }
    regexp = (kiln_regexp *)kiln_c99_gc_alloc(KILN_GC_REGEXP, sizeof *regexp);
    if (regexp == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    regexp->handle = NULL;
    regexp->matcher = matcher;
    regexp->matcher_ascii = matcher_ascii;
    regexp->source = source;
    regexp->source_len = source_len;
    regexp->flags = flags;
    regexp->flags_len = flags_len;
    regexp->global = 0;
    regexp->sticky = 0;
    regexp->last_index = 0.0;
    regexp->cached_subject_ptr = NULL;
    regexp->cached_subject_len = 0;
    regexp->cached_is_ascii = 0;
    regexp->cached_group_count = 0;
    for (index = 0; index < flags_len; ++index) {
        if (flags[index] == 'g') {
            regexp->global = 1;
        } else if (flags[index] == 'y') {
            regexp->sticky = 1;
        }
    }
    *out = kiln_c99_value(KILN_VALUE_REGEXP, (uint64_t)(uintptr_t)regexp, NULL, 0);
    return KILN_C99_OK;
}

static void kiln_c99_materialize_match_properties(kiln_array *arr) {
    kiln_value props;
    if (kiln_c99_new_object(&props) != KILN_C99_OK) {
        return;
    }
    kiln_c99_put_mode(NULL, props, "index", 5, kiln_c99_number((double)arr->match_index), 0, 0, NULL);
    kiln_c99_put_mode(NULL, props, "input", 5, arr->match_input, 0, 0, NULL);
    kiln_c99_put_mode(NULL, props, "groups", 6, kiln_c99_undefined(), 0, 0, NULL);
    arr->properties = props;
}

/* One search, and the match it found.
 *
 * Returns 1 for a match, 0 for none. A sticky pattern must match exactly at the cursor, not merely
 * at or after it -- the engine reports the first match from `start`, and that check is what turns
 * it into the stickier promise. */
static int kiln_c99_regexp_find(kiln_regexp *regexp, const uint16_t *units, size_t count,
                                size_t start, int64_t *ranges, size_t capacity, size_t *groups,
                                int *matched) {
    *matched = 0;
    *groups = 0;
    if (start > count) {
        return KILN_C99_OK;
    }
    if (regexp->matcher != NULL) {
        int status = regexp->matcher(units, count, start, ranges, capacity, groups, matched);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (*matched) {
            if (regexp->sticky && ranges[0] != (int64_t)start) {
                *matched = 0;
                return KILN_C99_OK;
            }
        }
        return KILN_C99_OK;
    }
    if (kiln_regexp_search(regexp->handle, units, count, start, ranges, capacity / 2) != 1) {
        return KILN_C99_OK;
    }
    if (regexp->sticky && ranges[0] != (int64_t)start) {
        return KILN_C99_OK;
    }
    *groups = kiln_regexp_group_count(regexp->handle);
    *matched = 1;
    return KILN_C99_OK;
}
