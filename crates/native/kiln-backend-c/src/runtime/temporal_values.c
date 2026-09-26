
/* The `Temporal` handle, and the readback that keeps every crossing scalar.
 *
 * The engine is the same `kiln-temporal` the interpreter links (ADR 0025), so the two cannot
 * disagree about a calendar. What is here is the object model around it, which stays in C for the
 * reason the regexp match result does: it is the part that is already written once. */
static kiln_value kiln_c99_temporal_value(void *handle) {
    return kiln_c99_value(KILN_VALUE_TEMPORAL, (uint64_t)(uintptr_t)handle, NULL, 0);
}

static void *kiln_c99_temporal_of(kiln_value value) {
    return (void *)(uintptr_t)value.bits;
}

/* Turn a provider failure into the error the engine chose.
 *
 * The class travels in the return code rather than being guessed from the message: `Temporal`
 * distinguishes RangeError from TypeError from SyntaxError on nearly every path, and Test262
 * checks the distinction, so a caller that collapsed them would be wrong in a way no probe that
 * only prints values would notice. */
static int kiln_c99_temporal_raise(kiln_value *thrown, int32_t code) {
    char inline_buffer[192];
    int32_t class_code = 0;
    int32_t written = kiln_temporal_last_error(&class_code, (unsigned char *)inline_buffer,
                                               sizeof inline_buffer - 1);
    const char *message = "Temporal rejected this operation";
    uint64_t kind = KILN_C99_BUILTIN_RANGE_ERROR;
    char *spilled = NULL;
    if (written < 0) {
        /* The one negative answer says exactly how much it wanted, so a second call always fits. */
        size_t wanted = (size_t)(-written);
        spilled = (char *)malloc(wanted + 1);
        if (spilled != NULL) {
            written = kiln_temporal_last_error(&class_code, (unsigned char *)spilled, wanted);
            if (written >= 0) {
                spilled[written] = '\0';
                message = spilled;
            }
        }
    } else if (written > 0) {
        inline_buffer[written] = '\0';
        message = inline_buffer;
    }
    /* -1 is a violated argument contract rather than a rejection the program can see, and it has
     * no message of its own; the classes below are the engine's own classification. */
    if (class_code == -3) {
        kind = KILN_C99_BUILTIN_TYPE_ERROR;
    } else if (class_code == -4) {
        kind = KILN_C99_BUILTIN_SYNTAX_ERROR;
    }
    (void)code;
    {
        int status = kiln_c99_throw_error(thrown, kind, message);
        free(spilled);
        return status;
    }
}

/* A value's own ISO text, under the provider's one-retry length protocol. */
static int kiln_c99_temporal_string(kiln_value *out, kiln_value value, kiln_value *thrown) {
    char inline_buffer[128];
    void *handle = kiln_c99_temporal_of(value);
    int32_t written =
        kiln_temporal_to_string(handle, (unsigned char *)inline_buffer, sizeof inline_buffer);
    if (written >= 0) {
        *out = kiln_c99_copy_text(inline_buffer, (size_t)written);
        return KILN_C99_OK;
    }
    {
        size_t wanted = (size_t)(-written);
        char *spilled = (char *)malloc(wanted);
        int status;
        if (spilled == NULL) {
            return kiln_c99_temporal_raise(thrown, written);
        }
        written = kiln_temporal_to_string(handle, (unsigned char *)spilled, wanted);
        if (written < 0) {
            free(spilled);
            return kiln_c99_temporal_raise(thrown, written);
        }
        *out = kiln_c99_copy_text(spilled, (size_t)written);
        status = KILN_C99_OK;
        free(spilled);
        return status;
    }
}

/* Which of the eight types a constructor builtin stands for, or -1 if it is not one. */
static int32_t kiln_c99_temporal_kind_of_builtin(uint64_t builtin) {
    if (builtin < KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE ||
        builtin > KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME) {
        return -1;
    }
    return (int32_t)(builtin - KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE);
}

/* The constructor builtin for a live handle, which is what `constructor` and the tag both want. */
static uint64_t kiln_c99_temporal_builtin_of(kiln_value value) {
    int32_t kind = kiln_temporal_kind(kiln_c99_temporal_of(value));
    if (kind < 0) {
        return KILN_C99_BUILTIN_TEMPORAL;
    }
    return KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE + (uint64_t)kind;
}

/* `Symbol.toStringTag`, which every one of the eight carries and which decides how it prints. */
static const char *kiln_c99_temporal_tag(kiln_value value) {
    switch (kiln_c99_temporal_builtin_of(value)) {
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE: return "Temporal.PlainDate";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_TIME: return "Temporal.PlainTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE_TIME: return "Temporal.PlainDateTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_YEAR_MONTH: return "Temporal.PlainYearMonth";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_MONTH_DAY: return "Temporal.PlainMonthDay";
    case KILN_C99_BUILTIN_TEMPORAL_DURATION: return "Temporal.Duration";
    case KILN_C99_BUILTIN_TEMPORAL_INSTANT: return "Temporal.Instant";
    case KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME: return "Temporal.ZonedDateTime";
    default: return "Temporal";
    }
}

/* The name a `Temporal` constructor answers to, for `.name` and for a refusal that says which. */
static const char *kiln_c99_temporal_builtin_name(uint64_t builtin) {
    switch (builtin) {
    case KILN_C99_BUILTIN_TEMPORAL: return "Temporal";
    case KILN_C99_BUILTIN_TEMPORAL_NOW: return "Temporal.Now";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE: return "Temporal.PlainDate";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_TIME: return "Temporal.PlainTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE_TIME: return "Temporal.PlainDateTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_YEAR_MONTH: return "Temporal.PlainYearMonth";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_MONTH_DAY: return "Temporal.PlainMonthDay";
    case KILN_C99_BUILTIN_TEMPORAL_DURATION: return "Temporal.Duration";
    case KILN_C99_BUILTIN_TEMPORAL_INSTANT: return "Temporal.Instant";
    case KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME: return "Temporal.ZonedDateTime";
    default: return NULL;
    }
}

/* A member that is there in the language and not here yet.
 *
 * It refuses *by name* rather than reading as `undefined`, because an undefined member turns into
 * "x is not a function" three frames away, and the whole point of the two-engine arrangement is
 * that a gap announces itself as a gap. */
static int kiln_c99_temporal_unsupported(kiln_value *thrown, const char *owner, const char *member,
                                         size_t member_len) {
    char message[192];
    size_t at = 0;
    const char *prefix = "unsupported Temporal member: ";
    while (*prefix != '\0' && at + 1 < sizeof message) {
        message[at++] = *prefix++;
    }
    while (*owner != '\0' && at + 1 < sizeof message) {
        message[at++] = *owner++;
    }
    if (at + 1 < sizeof message) {
        message[at++] = '.';
    }
    {
        size_t index;
        for (index = 0; index < member_len && at + 1 < sizeof message; ++index) {
            message[at++] = member[index];
        }
    }
    message[at] = '\0';
    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, message);
}

/* The member spelling is a canonical unit string at the generic read boundary. Convert it only
 * for the human-readable error payload; temporal dispatch and identity stay unit based. */
static int kiln_c99_temporal_unsupported_key(kiln_value *thrown, const char *owner,
                                             const kiln_string *member) {
    uint8_t *encoded = NULL;
    size_t encoded_len = 0;
    kiln_value value;
    int status;
    if (member == NULL) return kiln_c99_temporal_unsupported(thrown, owner, "", 0);
    value = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)member, member->unit_len);
    status = kiln_c99_string_to_utf8_lossy(value, &encoded, &encoded_len);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_temporal_unsupported(thrown, owner, (const char *)encoded, encoded_len);
    free(encoded);
    return status;
}
