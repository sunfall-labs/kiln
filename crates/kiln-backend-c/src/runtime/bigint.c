
/* A BigInt written out in `radix`.
 *
 * The provider owns the digits, so this asks twice: once for the length and once for the bytes.
 * A negative answer is the length it wanted, which is what makes one retry enough. */
static kiln_value kiln_c99_bigint_text(kiln_value value, uint32_t radix) {
    const void *handle = (const void *)(uintptr_t)value.bits;
    char inline_buffer[64];
    int32_t written = kiln_bigint_to_string(handle, radix, (unsigned char *)inline_buffer,
                                            sizeof inline_buffer);
    if (written >= 0) {
        return kiln_c99_copy_text(inline_buffer, (size_t)written);
    }
    {
        size_t needed = (size_t)(-(int64_t)written);
        char *wide = (char *)malloc(needed == 0 ? 1 : needed);
        kiln_value text;
        if (wide == NULL) {
            return kiln_c99_undefined();
        }
        written = kiln_bigint_to_string(handle, radix, (unsigned char *)wide, needed);
        if (written < 0) {
            free(wide);
            return kiln_c99_undefined();
        }
        text = kiln_c99_copy_text(wide, (size_t)written);
        free(wide);
        return text;
    }
}

/* A BigInt literal, parsed on first use and kept.
 *
 * The handle is shared by every load of the same literal, which is sound because a BigInt is
 * immutable: the provider's arithmetic always allocates its answer rather than writing back. */
static int kiln_c99_bigint_constant(void **cache, const char *digits, size_t len,
                                    kiln_value *out) {
    if (*cache == NULL) {
        void *handle = NULL;
        if (kiln_bigint_parse((const unsigned char *)digits, len, 10, &handle) != 0) {
            KILN_C99_REFUSE("a BigInt literal the provider would not parse");
        }
        *cache = handle;
    }
    *out = kiln_c99_bigint_value(*cache);
    return KILN_C99_OK;
}

/* `1n`, built once. Nothing frees a BigInt handle here, so a shared one is safe to keep. */
static const void *kiln_c99_bigint_one(void) {
    static void *one = NULL;
    if (one == NULL) {
        static const unsigned char digit[] = {'1'};
        if (kiln_bigint_parse(digit, sizeof digit, 10, &one) != 0) {
            return NULL;
        }
    }
    return one;
}

/* Wrap a handle the provider just produced. */
static kiln_value kiln_c99_bigint_value(void *handle) {
    return kiln_c99_value(KILN_VALUE_BIGINT, (uint64_t)(uintptr_t)handle, NULL, 0);
}
