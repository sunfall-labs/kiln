


/* The calendar and wall-clock field names, in the order the specification reads them.
 *
 * The order is observable: each name is a property *read*, a getter can run, and Test262 checks
 * the sequence. It is the same list the interpreter walks, for the same reason. `slot` is where the
 * value lands in the provider's numeric array, or -1 for the five that are text. */
typedef struct kiln_temporal_key {
    const char *name;
    int slot;
    int text_slot;
} kiln_temporal_key;

static const kiln_temporal_key KILN_C99_TEMPORAL_FIELD_KEYS[15] = {
    {"calendar", -1, 4},   {"day", 3, -1},        {"era", -1, 0},
    {"eraYear", 0, -1},    {"hour", 4, -1},       {"microsecond", 8, -1},
    {"millisecond", 7, -1},{"minute", 5, -1},     {"month", 2, -1},
    {"monthCode", -1, 1},  {"nanosecond", 9, -1}, {"offset", -1, 2},
    {"second", 6, -1},     {"timeZone", -1, 3},   {"year", 1, -1}};

/* The ten duration components, in the order the specification reads those. */
static const kiln_temporal_key KILN_C99_TEMPORAL_DURATION_KEYS[10] = {
    {"days", 3, -1},         {"hours", 4, -1},       {"microseconds", 8, -1},
    {"milliseconds", 7, -1}, {"minutes", 5, -1},     {"months", 1, -1},
    {"nanoseconds", 9, -1},  {"seconds", 6, -1},     {"weeks", 2, -1},
    {"years", 0, -1}};

/* The arithmetic and conversion operations, in the order their method codes run.
 *
 * One table read from both ends: by name when the property is read off a value, and by code when
 * the call arrives. The string it holds is also the string the provider dispatches on, so there is
 * no second spelling of `toPlainYearMonth` anywhere for the two to drift apart on. */
#define KILN_C99_TEMPORAL_OP_COUNT 23
static const char *const KILN_C99_TEMPORAL_OPS[KILN_C99_TEMPORAL_OP_COUNT] = {
    "add",           "subtract",          "until",        "since",
    "round",         "total",             "with",         "equals",
    "negated",       "abs",               "startOfDay",   "withCalendar",
    "withTimeZone",  "withPlainTime",     "toZonedDateTime", "toZonedDateTimeISO",
    "toPlainDate",   "toPlainTime",       "toPlainDateTime", "toPlainYearMonth",
    "toPlainMonthDay", "toInstant",       "toString"};

static const char *kiln_c99_temporal_op_name(uint64_t code) {
    if (!kiln_c99_method_in_group(code, KILN_C99_GROUP_TEMPORAL_OP)) {
        return NULL;
    }
    return KILN_C99_TEMPORAL_OPS[code - KILN_C99_METHOD_TEMPORAL_OP];
}

/* The method code an operation name reaches, or zero for a name that is not one of them. */
static uint64_t kiln_c99_temporal_op_code(const char *key, size_t key_len) {
    size_t index;
    for (index = 0; index < KILN_C99_TEMPORAL_OP_COUNT; ++index) {
        if (kiln_c99_same_name(key, key_len, KILN_C99_TEMPORAL_OPS[index])) {
            return KILN_C99_METHOD_TEMPORAL_OP + (uint64_t)index;
        }
    }
    return 0;
}

/* `Temporal.Now`'s five readers, by the kind their code carries. */
static const char *kiln_c99_temporal_now_name(uint64_t code) {
    switch (code - KILN_C99_METHOD_TEMPORAL_NOW_READ) {
    case 0: return "plainDateISO";
    case 1: return "plainTimeISO";
    case 2: return "plainDateTimeISO";
    case 6: return "instant";
    case 7: return "zonedDateTimeISO";
    default: return NULL;
    }
}

/* The option names, in the order the language reads them off an options bag.
 *
 * The reads are observable -- each can be a getter, and the corpus counts exactly which ones run
 * and in what order -- so this is the *vocabulary*, not the set any one operation reads. Reading
 * all eleven every time is not the conservative choice it looks like: `date.toString(bag)` ran
 * eleven getters where the specification runs one. Which subset an operation takes is a mask,
 * below. `slot` is where the string lands in the provider's option array; the two numeric options
 * are named here and handled apart, because one of them has a spelling (`"auto"`) that is not a
 * number. */
#define KILN_C99_TEMPORAL_OPTION_COUNT 11
#define KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS 9
#define KILN_C99_TEMPORAL_OPTION_NUMBER_SLOTS 2
static const kiln_temporal_key KILN_C99_TEMPORAL_OPTION_KEYS[KILN_C99_TEMPORAL_OPTION_COUNT] = {
    {"calendarName", -1, 0},          {"disambiguation", -1, 1},
    {"fractionalSecondDigits", 1, -1},{"largestUnit", -1, 2},
    {"offset", -1, 3},                {"overflow", -1, 4},
    {"relativeTo", -2, -1},           {"roundingIncrement", 0, -1},
    {"roundingMode", -1, 5},          {"smallestUnit", -1, 6},
    {"timeZoneName", -1, 7}};

/* One bit per entry of the table above, so an operation can name the subset it reads.
 *
 * Grouped by operation rather than by type, because that is how the specification groups them:
 * every difference takes the same four, every calendar conversion takes `overflow`. These are the
 * same sets the interpreter's `temporal::options` module holds, and the corpus probe in
 * `conformance/native/temporal.ts` is what keeps the two spellings honest. */
#define KILN_C99_TEMPORAL_OPT_CALENDAR_NAME (1u << 0)
#define KILN_C99_TEMPORAL_OPT_DISAMBIGUATION (1u << 1)
#define KILN_C99_TEMPORAL_OPT_FRACTIONAL (1u << 2)
#define KILN_C99_TEMPORAL_OPT_LARGEST_UNIT (1u << 3)
#define KILN_C99_TEMPORAL_OPT_OFFSET (1u << 4)
#define KILN_C99_TEMPORAL_OPT_OVERFLOW (1u << 5)
#define KILN_C99_TEMPORAL_OPT_RELATIVE_TO (1u << 6)
#define KILN_C99_TEMPORAL_OPT_ROUNDING_INCREMENT (1u << 7)
#define KILN_C99_TEMPORAL_OPT_ROUNDING_MODE (1u << 8)
#define KILN_C99_TEMPORAL_OPT_SMALLEST_UNIT (1u << 9)
#define KILN_C99_TEMPORAL_OPT_TIME_ZONE_NAME (1u << 10)

/* `from`, `with`, `add` and `subtract` on the four types with a calendar to fall off the end of. */
#define KILN_C99_TEMPORAL_OPTS_OVERFLOW KILN_C99_TEMPORAL_OPT_OVERFLOW
/* The same on a `ZonedDateTime`, which also has to be told what to do with a wall-clock time its
 * zone skips or repeats. */
#define KILN_C99_TEMPORAL_OPTS_ZONED_OVERFLOW                                                      \
    (KILN_C99_TEMPORAL_OPT_DISAMBIGUATION | KILN_C99_TEMPORAL_OPT_OFFSET |                          \
     KILN_C99_TEMPORAL_OPT_OVERFLOW)
/* `until` and `since`, on every type that has them. */
#define KILN_C99_TEMPORAL_OPTS_DIFFERENCE                                                          \
    (KILN_C99_TEMPORAL_OPT_LARGEST_UNIT | KILN_C99_TEMPORAL_OPT_ROUNDING_INCREMENT |                \
     KILN_C99_TEMPORAL_OPT_ROUNDING_MODE | KILN_C99_TEMPORAL_OPT_SMALLEST_UNIT)
/* `round`, on the four types whose rounding has no calendar in it. */
#define KILN_C99_TEMPORAL_OPTS_ROUND                                                               \
    (KILN_C99_TEMPORAL_OPT_ROUNDING_INCREMENT | KILN_C99_TEMPORAL_OPT_ROUNDING_MODE |               \
     KILN_C99_TEMPORAL_OPT_SMALLEST_UNIT)
/* `Temporal.Duration.prototype.round`, which does: a duration in months rounds differently
 * depending on which months, so it takes an anchor and a largest unit as well. */
#define KILN_C99_TEMPORAL_OPTS_DURATION_ROUND                                                      \
    (KILN_C99_TEMPORAL_OPTS_ROUND | KILN_C99_TEMPORAL_OPT_LARGEST_UNIT |                            \
     KILN_C99_TEMPORAL_OPT_RELATIVE_TO)
/* `Temporal.Duration.prototype.total` and `Temporal.Duration.compare`. `total` also reads `unit`,
 * which is not in the shared vocabulary and is read after this set, which is where the
 * specification's alphabetical order puts it. */
#define KILN_C99_TEMPORAL_OPTS_RELATIVE_TO KILN_C99_TEMPORAL_OPT_RELATIVE_TO
/* `Temporal.PlainDateTime.prototype.toZonedDateTime`. */
#define KILN_C99_TEMPORAL_OPTS_DISAMBIGUATION KILN_C99_TEMPORAL_OPT_DISAMBIGUATION
/* A date has only a calendar to name in its string form. */
#define KILN_C99_TEMPORAL_OPTS_CALENDAR_NAME KILN_C99_TEMPORAL_OPT_CALENDAR_NAME
/* A time has only a precision to round to. */
#define KILN_C99_TEMPORAL_OPTS_TIME_PRECISION                                                      \
    (KILN_C99_TEMPORAL_OPT_FRACTIONAL | KILN_C99_TEMPORAL_OPT_ROUNDING_MODE |                       \
     KILN_C99_TEMPORAL_OPT_SMALLEST_UNIT)
/* A date-and-time has both. */
#define KILN_C99_TEMPORAL_OPTS_DATE_TIME_STRING                                                    \
    (KILN_C99_TEMPORAL_OPTS_TIME_PRECISION | KILN_C99_TEMPORAL_OPT_CALENDAR_NAME)
/* A zoned date-time has both, plus the two annotations only it carries. */
#define KILN_C99_TEMPORAL_OPTS_ZONED_STRING                                                        \
    (KILN_C99_TEMPORAL_OPTS_DATE_TIME_STRING | KILN_C99_TEMPORAL_OPT_OFFSET |                       \
     KILN_C99_TEMPORAL_OPT_TIME_ZONE_NAME)
/* An operation that validates its bag is an object and reads nothing out of it. */
#define KILN_C99_TEMPORAL_OPTS_NONE 0u

/* The keys `from` and `with` read on this kind: a `ZonedDateTime` has a zone to resolve as well as
 * a calendar to overflow, and the two kinds without a calendar read nothing. */
static unsigned kiln_c99_temporal_overflow_keys(int32_t kind) {
    if (kind == 7) {
        return KILN_C99_TEMPORAL_OPTS_ZONED_OVERFLOW;
    }
    if (kind == 5 || kind == 6) {
        return KILN_C99_TEMPORAL_OPTS_NONE;
    }
    return KILN_C99_TEMPORAL_OPTS_OVERFLOW;
}

/* The keys `toString` reads on this kind. */
static unsigned kiln_c99_temporal_string_keys(int32_t kind) {
    switch (kind) {
    case 0:
    case 3:
    case 4:
        return KILN_C99_TEMPORAL_OPTS_CALENDAR_NAME;
    case 2:
        return KILN_C99_TEMPORAL_OPTS_DATE_TIME_STRING;
    case 7:
        return KILN_C99_TEMPORAL_OPTS_ZONED_STRING;
    default:
        return KILN_C99_TEMPORAL_OPTS_TIME_PRECISION;
    }
}

/* Whether `add`/`subtract` on this kind reads an options bag at all. A `PlainTime` wraps around the
 * clock, and a `Duration` and an `Instant` have no month to fall off the end of, so all three take
 * a one-argument signature and ignore whatever follows. */
static int kiln_c99_temporal_takes_arithmetic_options(int32_t kind) {
    return kind == 0 || kind == 2 || kind == 3 || kind == 7;
}

/* The field string slots: era, monthCode, offset, timeZone, calendar. `overflow` is not among
 * them any more -- it is an option, and options travel in their own array. */
#define KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS 5

/* One text slot: the bytes a field carried, owned by the packing buffer below. */
typedef struct kiln_temporal_text {
    const char *bytes;
    int64_t length;
} kiln_temporal_text;

/* Pack text slots into the one buffer plus one length per slot the provider takes.
 *
 * Answers a buffer the caller frees, or NULL when the allocation failed. A length of -1 marks a
 * slot the program did not supply at all, which is not the same as supplying `""`. */
static char *kiln_c99_temporal_pack(const kiln_temporal_text *texts, size_t count,
                                    int64_t *lengths) {
    size_t total = 0;
    size_t at = 0;
    size_t slot;
    char *packed;
    for (slot = 0; slot < count; ++slot) {
        lengths[slot] = texts[slot].bytes == NULL ? -1 : texts[slot].length;
        if (lengths[slot] > 0) {
            total += (size_t)lengths[slot];
        }
    }
    packed = (char *)malloc(total == 0 ? 1 : total);
    if (packed == NULL) {
        return NULL;
    }
    for (slot = 0; slot < count; ++slot) {
        if (lengths[slot] > 0) {
            memcpy(packed + at, texts[slot].bytes, (size_t)lengths[slot]);
            at += (size_t)lengths[slot];
        }
    }
    return packed;
}

/* `Temporal.X.from(item)`, declared here because reading an options bag can reach it: a
 * `relativeTo` is itself a date, and coercing it is the same operation. */
static int kiln_c99_temporal_coerce(kiln_host *host, int32_t kind, kiln_value item,
                                    kiln_value options, kiln_value *out, int depth,
                                    kiln_value *thrown);

/* One options bag, read into the shape the provider takes. */
typedef struct kiln_temporal_options {
    double numbers[KILN_C99_TEMPORAL_OPTION_NUMBER_SLOTS];
    kiln_temporal_text texts[KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS];
    /* `relativeTo`, already coerced, or an undefined value when the bag carried none. */
    kiln_value relative_to;
} kiln_temporal_options;

static void kiln_c99_temporal_options_init(kiln_temporal_options *options) {
    size_t index;
    for (index = 0; index < KILN_C99_TEMPORAL_OPTION_NUMBER_SLOTS; ++index) {
        options->numbers[index] = 0.0 / 0.0;
    }
    for (index = 0; index < KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS; ++index) {
        options->texts[index].bytes = NULL;
        options->texts[index].length = -1;
    }
    options->relative_to = kiln_c99_undefined();
}

/* Read the options this operation takes, in the order the specification reads them.
 *
 * `keys` is the mask above: the reads are observable, so which of them run is part of the answer
 * and not an implementation detail. The bag is still checked for objecthood when the mask is
 * empty, because `GetOptionsObject` runs whether or not the operation goes on to read anything. */
static int kiln_c99_temporal_read_options(kiln_host *host, kiln_value bag, unsigned keys,
                                          kiln_temporal_options *options, int depth,
                                          kiln_value *thrown) {
    size_t index;
    if (bag.tag == KILN_VALUE_UNDEFINED) {
        return KILN_C99_OK;
    }
    if (bag.tag != KILN_VALUE_OBJECT && bag.tag != KILN_VALUE_ARRAY &&
        bag.tag != KILN_VALUE_FUNCTION) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Temporal options must be an object");
    }
    for (index = 0; index < KILN_C99_TEMPORAL_OPTION_COUNT; ++index) {
        const kiln_temporal_key *key = &KILN_C99_TEMPORAL_OPTION_KEYS[index];
        kiln_value read;
        if ((keys & (1u << index)) == 0u) {
            continue;
        }
        int status = kiln_c99_get(host, bag, key->name, strlen(key->name), &read, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (read.tag == KILN_VALUE_UNDEFINED) {
            continue;
        }
        if (key->text_slot >= 0) {
            kiln_value text = kiln_c99_text(read);
            if (text.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            options->texts[key->text_slot].bytes = text.ptr;
            options->texts[key->text_slot].length = (int64_t)text.len;
            continue;
        }
        if (key->slot == -2) {
            /* A `relativeTo` is a date or a zoned date, and which one decides whether a day is
             * always twenty-four hours -- so it is coerced here rather than guessed at. */
            if (read.tag == KILN_VALUE_TEMPORAL) {
                options->relative_to = read;
                continue;
            }
            status = kiln_c99_temporal_coerce(host, 0, read, kiln_c99_undefined(),
                                              &options->relative_to, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            continue;
        }
        if (kiln_c99_same_name(key->name, strlen(key->name), "fractionalSecondDigits")) {
            /* `"auto"` is a legal spelling and is not a digit count, so it travels as a negative
             * one rather than as a number that would round to zero digits. */
            if (read.tag == KILN_VALUE_STRING && kiln_c99_same_name(read.ptr, read.len, "auto")) {
                options->numbers[key->slot] = -1.0;
                continue;
            }
            {
                double digits;
                status = kiln_c99_to_number_checked(host, read, &digits, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                digits = trunc(digits);
                if (!(digits >= 0.0 && digits <= 9.0)) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                "fractionalSecondDigits must be between 0 and 9");
                }
                options->numbers[key->slot] = digits;
            }
            continue;
        }
        {
            double number;
            status = kiln_c99_to_number_checked(host, read, &number, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            options->numbers[key->slot] = number;
        }
    }
    return KILN_C99_OK;
}

/* Read a bag into the provider's two arrays, in the specified order.
 *
 * The reads happen here rather than behind the boundary because they are observable, and a
 * boundary that took the object could not express that. */
static int kiln_c99_temporal_read_bag(kiln_host *host, kiln_value bag, int duration,
                                      double *numbers, kiln_temporal_text *texts, int depth,
                                      kiln_value *thrown) {
    const kiln_temporal_key *keys =
        duration ? KILN_C99_TEMPORAL_DURATION_KEYS : KILN_C99_TEMPORAL_FIELD_KEYS;
    size_t count = duration ? 10u : 15u;
    size_t index;
    int touched = 0;
    for (index = 0; index < count; ++index) {
        kiln_value read;
        int status = kiln_c99_get(host, bag, keys[index].name, strlen(keys[index].name), &read,
                                  depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (read.tag == KILN_VALUE_UNDEFINED) {
            continue;
        }
        touched = 1;
        if (keys[index].text_slot >= 0) {
            kiln_value text = kiln_c99_text(read);
            if (text.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            texts[keys[index].text_slot].bytes = text.ptr;
            texts[keys[index].text_slot].length = (int64_t)text.len;
            continue;
        }
        {
            double number;
            status = kiln_c99_to_number_checked(host, read, &number, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            /* NaN is the provider's "absent", so a field that really is NaN has to be rejected
             * here rather than silently dropped -- the language throws for one anyway. */
            if (number != number) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "a Temporal field cannot be NaN");
            }
            numbers[keys[index].slot] = number;
        }
    }
    if (!touched) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "a Temporal bag needs at least one recognized field");
    }
    return KILN_C99_OK;
}

/* Pack the field and option slots into the buffers the provider takes, and build the value. */
static int kiln_c99_temporal_build(int32_t kind, const double *numbers,
                                   const kiln_temporal_text *texts,
                                   const kiln_temporal_options *options, kiln_value *out,
                                   kiln_value *thrown) {
    int64_t lengths[KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS];
    int64_t option_lengths[KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS];
    char *packed = kiln_c99_temporal_pack(texts, KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS, lengths);
    char *packed_options;
    void *handle = NULL;
    int32_t answered;
    if (packed == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    packed_options = kiln_c99_temporal_pack(options->texts, KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS,
                                            option_lengths);
    if (packed_options == NULL) {
        free(packed);
        return KILN_C99_OUT_OF_MEMORY;
    }
    if (kind == 5) {
        /* A duration takes no options at all, and rejects a fractional component. */
        answered = kiln_temporal_duration_from_fields(numbers, 10, &handle);
    } else {
        answered = kiln_temporal_from_fields(
            (uint32_t)kind, numbers, 10, (const unsigned char *)packed, lengths,
            KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS, (const unsigned char *)packed_options,
            option_lengths, KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS, &handle);
    }
    free(packed);
    free(packed_options);
    if (answered != 0) {
        return kiln_c99_temporal_raise(thrown, answered);
    }
    *out = kiln_c99_temporal_value(handle);
    return KILN_C99_OK;
}

/* One positional argument of a `Temporal` constructor, coerced where the language coerces.
 *
 * `absent` is what an argument the caller left off becomes: 0 for a component the language
 * defaults, and NaN for one it requires -- because NaN is the provider's "absent" and the engine
 * rejects a required component that is missing with the RangeError the language gives, rather
 * than quietly building a value out of a zero nobody wrote. */
static int kiln_c99_temporal_argument(kiln_host *host, const kiln_value *args, size_t argc,
                                      size_t at, double absent, double *out, int depth,
                                      kiln_value *thrown) {
    if (at >= argc || args[at].tag == KILN_VALUE_UNDEFINED) {
        *out = absent;
        return KILN_C99_OK;
    }
    return kiln_c99_to_number_checked(host, args[at], out, depth, thrown);
}

/* One positional *string* argument, left absent when the caller omitted it. */
static void kiln_c99_temporal_text_argument(const kiln_value *args, size_t argc, size_t at,
                                            kiln_temporal_text *slot) {
    kiln_value text;
    if (at >= argc || args[at].tag == KILN_VALUE_UNDEFINED) {
        return;
    }
    text = kiln_c99_text(args[at]);
    if (text.tag != KILN_VALUE_STRING) {
        return;
    }
    slot->bytes = text.ptr;
    slot->length = (int64_t)text.len;
}

/* `new Temporal.X(...)`: the eight positional constructors.
 *
 * NOT `from` with a bag, and the difference is observable in both directions. `from` reads named
 * fields, resolves them through the calendar and honours an `overflow` option that defaults to
 * `constrain`; a constructor takes *ISO* components positionally and rejects one out of range.
 * `new Temporal.PlainDate(2000, 300, 1)` is a RangeError where the `from` of the same numbers is
 * 2000-12-01. So this reaches `kiln_temporal_construct` -- the engine's own constructors, the
 * same ones the interpreter reaches under ADR 0025 -- rather than being routed through the bag
 * path, which would have been a silent wrong answer wearing the right shape.
 *
 * The arguments are read left to right because a `valueOf` can count its own calls, and the
 * specification reads them in that order. */
static int kiln_c99_temporal_construct(kiln_host *host, kiln_value *result, int32_t kind,
                                       const kiln_value *args, size_t argc, int depth,
                                       kiln_value *thrown) {
    double numbers[10];
    kiln_temporal_text texts[KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS];
    int64_t lengths[KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS];
    char *packed;
    int64_t high = 0;
    uint64_t low = 0;
    void *handle = NULL;
    int32_t answered;
    size_t index;
    int status = KILN_C99_OK;
    const double missing = 0.0 / 0.0;
    for (index = 0; index < 10; ++index) {
        numbers[index] = missing;
    }
    for (index = 0; index < KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS; ++index) {
        texts[index].bytes = NULL;
        texts[index].length = -1;
    }
    switch (kind) {
    case 0: /* PlainDate(isoYear, isoMonth, isoDay, calendar) */
        for (index = 0; index < 3 && status == KILN_C99_OK; ++index) {
            status = kiln_c99_temporal_argument(host, args, argc, index, missing,
                                                &numbers[1 + index], depth, thrown);
        }
        kiln_c99_temporal_text_argument(args, argc, 3, &texts[4]);
        break;
    case 1: /* PlainTime(hour, minute, second, millisecond, microsecond, nanosecond) */
        for (index = 0; index < 6 && status == KILN_C99_OK; ++index) {
            status = kiln_c99_temporal_argument(host, args, argc, index, 0.0,
                                                &numbers[4 + index], depth, thrown);
        }
        break;
    case 2: /* PlainDateTime(isoYear, isoMonth, isoDay, then the six time components, calendar) */
        for (index = 0; index < 9 && status == KILN_C99_OK; ++index) {
            status = kiln_c99_temporal_argument(host, args, argc, index,
                                                index < 3 ? missing : 0.0, &numbers[1 + index],
                                                depth, thrown);
        }
        kiln_c99_temporal_text_argument(args, argc, 9, &texts[4]);
        break;
    case 3: /* PlainYearMonth(isoYear, isoMonth, calendar, referenceISODay) */
        for (index = 0; index < 2 && status == KILN_C99_OK; ++index) {
            status = kiln_c99_temporal_argument(host, args, argc, index, missing,
                                                &numbers[1 + index], depth, thrown);
        }
        kiln_c99_temporal_text_argument(args, argc, 2, &texts[4]);
        /* The reference day rides in the `day` slot and stays NaN when it is left out, which is
         * not the same as a day of zero: the engine then supplies the first of the month. */
        if (status == KILN_C99_OK) {
            status = kiln_c99_temporal_argument(host, args, argc, 3, missing, &numbers[3], depth,
                                                thrown);
        }
        break;
    case 4: /* PlainMonthDay(isoMonth, isoDay, calendar, referenceISOYear) */
        for (index = 0; index < 2 && status == KILN_C99_OK; ++index) {
            status = kiln_c99_temporal_argument(host, args, argc, index, missing,
                                                &numbers[2 + index], depth, thrown);
        }
        kiln_c99_temporal_text_argument(args, argc, 2, &texts[4]);
        /* The reference year rides in the `year` slot; absent means the engine's 1972, which is
         * the first ISO leap year after the epoch and the reason the slot exists at all. */
        if (status == KILN_C99_OK) {
            status = kiln_c99_temporal_argument(host, args, argc, 3, missing, &numbers[1], depth,
                                                thrown);
        }
        break;
    case 5: /* Duration(years .. nanoseconds), in the duration slot order. */
        for (index = 0; index < 10 && status == KILN_C99_OK; ++index) {
            status = kiln_c99_temporal_argument(host, args, argc, index, 0.0, &numbers[index],
                                                depth, thrown);
        }
        break;
    case 6: /* Instant(epochNanoseconds) */
    case 7: /* ZonedDateTime(epochNanoseconds, timeZone, calendar) */
        /* A BigInt and nothing else -- not a number coerced to one. Nanoseconds since the epoch
         * pass through a double exactly only within four months of 1970, so the language refuses
         * a number here rather than accepting one it would have to round. */
        if (argc == 0 || args[0].tag != KILN_VALUE_BIGINT) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "a Temporal constructor needs a BigInt epochNanoseconds");
        }
        if (kiln_bigint_to_i128((const void *)(uintptr_t)args[0].bits, &high, &low) != 0) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "epochNanoseconds is outside the representable range");
        }
        if (kind == 7) {
            kiln_c99_temporal_text_argument(args, argc, 1, &texts[3]);
            kiln_c99_temporal_text_argument(args, argc, 2, &texts[4]);
        }
        break;
    default: KILN_C99_REFUSE("a Temporal type this backend cannot construct");
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    packed = kiln_c99_temporal_pack(texts, KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS, lengths);
    if (packed == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    answered = kiln_temporal_construct((uint32_t)kind, numbers, 10, (const unsigned char *)packed,
                                       lengths, KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS, high, low,
                                       &handle);
    free(packed);
    if (answered != 0) {
        return kiln_c99_temporal_raise(thrown, answered);
    }
    *result = kiln_c99_temporal_value(handle);
    return KILN_C99_OK;
}

/* `Temporal.X.from(item)`: a value of the same type, ISO text, or a bag of fields.
 *
 * The options are read whichever shape `item` has, and before the value is built, because the
 * reads are observable and the specification runs them. */
static int kiln_c99_temporal_coerce(kiln_host *host, int32_t kind, kiln_value item,
                                    kiln_value options, kiln_value *out, int depth,
                                    kiln_value *thrown) {
    double numbers[10];
    kiln_temporal_text texts[KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS];
    kiln_temporal_options settings;
    size_t index;
    int status;
    for (index = 0; index < 10; ++index) {
        numbers[index] = 0.0 / 0.0;
    }
    for (index = 0; index < KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS; ++index) {
        texts[index].bytes = NULL;
        texts[index].length = -1;
    }
    kiln_c99_temporal_options_init(&settings);
    if (item.tag == KILN_VALUE_TEMPORAL) {
        /* Same type in, same value out: every one of them is immutable, so there is nothing to
         * copy. A *wider* type narrows -- a `PlainDateTime` is a `PlainDate` with a time attached,
         * and asking for the date is a projection rather than a conversion that can be wrong. A
         * pair the language does not define is the engine's TypeError, not one invented here. */
        void *narrowed = NULL;
        int32_t answered;
        if (kiln_temporal_kind(kiln_c99_temporal_of(item)) == kind) {
            *out = item;
            return KILN_C99_OK;
        }
        answered = kiln_temporal_to_kind(kiln_c99_temporal_of(item), (uint32_t)kind, &narrowed);
        if (answered != 0) {
            return kiln_c99_temporal_raise(thrown, answered);
        }
        *out = kiln_c99_temporal_value(narrowed);
        return KILN_C99_OK;
    }
    if (item.tag == KILN_VALUE_STRING) {
        int64_t option_lengths[KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS];
        char *packed_options;
        void *handle = NULL;
        int32_t answered;
        status = kiln_c99_temporal_read_options(host, options,
                                                kiln_c99_temporal_overflow_keys(kind), &settings,
                                                depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        packed_options = kiln_c99_temporal_pack(
            settings.texts, KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS, option_lengths);
        if (packed_options == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        /* A `ZonedDateTime` string is the one `from` whose options change the answer, because it
         * has to decide what a wall-clock time that does not exist, or exists twice, means. */
        answered = kiln_temporal_parse((uint32_t)kind, (const unsigned char *)item.ptr, item.len,
                                       (const unsigned char *)packed_options, option_lengths,
                                       KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS, &handle);
        free(packed_options);
        if (answered != 0) {
            return kiln_c99_temporal_raise(thrown, answered);
        }
        *out = kiln_c99_temporal_value(handle);
        return KILN_C99_OK;
    }
    if (item.tag != KILN_VALUE_OBJECT) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Temporal.from needs an object or an ISO string");
    }
    status = kiln_c99_temporal_read_bag(host, item, kind == 5, numbers, texts, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* The options are read after the fields, which is the order the specification puts them in. */
    status = kiln_c99_temporal_read_options(host, options, kiln_c99_temporal_overflow_keys(kind),
                                            &settings, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    return kiln_c99_temporal_build(kind, numbers, texts, &settings, out, thrown);
}

/* Turn the provider's answer into a value, or into the throw its error class asks for. */
static int kiln_c99_temporal_answer(int32_t answered, void *handle, double number,
                                    int32_t length, const char *inline_buffer,
                                    kiln_value *result, kiln_value *thrown) {
    switch (answered) {
    case 0:
        *result = kiln_c99_temporal_value(handle);
        return KILN_C99_OK;
    case 1:
        *result = kiln_c99_number(number);
        return KILN_C99_OK;
    case 3:
        *result = kiln_c99_boolean(number != 0.0);
        return KILN_C99_OK;
    case 2:
        if (length < 0) {
            /* The retry protocol is handled by the caller, which owns the buffer. */
            KILN_C99_REFUSE("a Temporal answer that did not fit the buffer the caller had grown "
                            "for it");
        }
        *result = kiln_c99_copy_text(inline_buffer, (size_t)length);
        return KILN_C99_OK;
    default:
        return kiln_c99_temporal_raise(thrown, answered);
    }
}

/* One arithmetic or conversion operation, from the arguments the program passed.
 *
 * Every argument the language specifies is read here, in the specified order, and what crosses the
 * boundary is scalars: a handle, a field bag, an option bag flattened into two arrays. The engine
 * on the other side is the same `kiln-temporal` the interpreter calls directly (ADR 0025), so a
 * disagreement between the two engines about a calendar is not reachable from here. */
static int kiln_c99_temporal_operation(kiln_host *host, kiln_value *result, uint64_t code,
                                       kiln_value receiver, const kiln_value *args, size_t argc,
                                       int depth, kiln_value *thrown) {
    const char *name = kiln_c99_temporal_op_name(code);
    int32_t kind = kiln_temporal_kind(kiln_c99_temporal_of(receiver));
    kiln_value first = argc > 0 ? args[0] : kiln_c99_undefined();
    kiln_value second = argc > 1 ? args[1] : kiln_c99_undefined();
    kiln_temporal_options options;
    double fields[10];
    kiln_temporal_text field_texts[KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS];
    int64_t field_lengths[KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS];
    int64_t option_lengths[KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS];
    char *packed_fields = NULL;
    char *packed_options = NULL;
    char inline_buffer[64];
    double number = 0.0;
    int32_t length = 0;
    void *handle = NULL;
    kiln_value operand = kiln_c99_undefined();
    kiln_value zone = kiln_c99_undefined();
    int carries_fields = 0;
    int status = KILN_C99_OK;
    int32_t answered;
    kiln_temporal_call call;
    size_t index;
    if (name == NULL || kind < 0) {
        KILN_C99_REFUSE("a Temporal operation with no name or no kind");
    }
    kiln_c99_temporal_options_init(&options);
    for (index = 0; index < 10; ++index) {
        fields[index] = 0.0 / 0.0;
    }
    for (index = 0; index < KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS; ++index) {
        field_texts[index].bytes = NULL;
        field_texts[index].length = -1;
    }

    /* The arguments, by the shape each operation takes them in. */
    if (kiln_c99_same_name(name, strlen(name), "add") ||
        kiln_c99_same_name(name, strlen(name), "subtract")) {
        /* Every type but `Duration` adds a duration; a `Duration` adds another duration. Either
         * way the operand is a duration, and reaching one that is not is the engine's TypeError. */
        status = kiln_c99_temporal_coerce(host, 5, first, kiln_c99_undefined(), &operand, depth,
                                          thrown);
        if (status == KILN_C99_OK && kiln_c99_temporal_takes_arithmetic_options(kind)) {
            status = kiln_c99_temporal_read_options(host, second, KILN_C99_TEMPORAL_OPTS_OVERFLOW,
                                                    &options, depth, thrown);
        }
    } else if (kiln_c99_same_name(name, strlen(name), "until") ||
               kiln_c99_same_name(name, strlen(name), "since") ||
               kiln_c99_same_name(name, strlen(name), "equals")) {
        status = kiln_c99_temporal_coerce(host, kind, first, kiln_c99_undefined(), &operand, depth,
                                          thrown);
        if (status == KILN_C99_OK && !kiln_c99_same_name(name, strlen(name), "equals")) {
            status = kiln_c99_temporal_read_options(host, second, KILN_C99_TEMPORAL_OPTS_DIFFERENCE,
                                                    &options, depth, thrown);
        }
    } else if (kiln_c99_same_name(name, strlen(name), "round") ||
               kiln_c99_same_name(name, strlen(name), "total")) {
        /* A bare unit string is shorthand for `{ smallestUnit: it }` -- or `{ unit: it }` for
         * `total` -- which is how nearly every call in the wild spells it. */
        int totalling = kiln_c99_same_name(name, strlen(name), "total");
        if (first.tag == KILN_VALUE_STRING) {
            options.texts[totalling ? 8 : 6].bytes = first.ptr;
            options.texts[totalling ? 8 : 6].length = (int64_t)first.len;
        } else if (first.tag == KILN_VALUE_UNDEFINED) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        totalling ? "total needs a unit" : "round needs a unit");
        } else {
            unsigned keys = totalling ? KILN_C99_TEMPORAL_OPTS_RELATIVE_TO
                                      : (kind == 5 ? KILN_C99_TEMPORAL_OPTS_DURATION_ROUND
                                                   : KILN_C99_TEMPORAL_OPTS_ROUND);
            status = kiln_c99_temporal_read_options(host, first, keys, &options, depth, thrown);
            if (status == KILN_C99_OK && totalling) {
                /* `unit` is not in the shared option list, because only `total` takes it. */
                kiln_value read;
                status = kiln_c99_get(host, first, "unit", 4, &read, depth, thrown);
                if (status == KILN_C99_OK && read.tag != KILN_VALUE_UNDEFINED) {
                    kiln_value text = kiln_c99_text(read);
                    if (text.tag != KILN_VALUE_STRING) {
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    options.texts[8].bytes = text.ptr;
                    options.texts[8].length = (int64_t)text.len;
                }
            }
        }
    } else if (kiln_c99_same_name(name, strlen(name), "with")) {
        if (first.tag != KILN_VALUE_OBJECT) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "with needs a property bag");
        }
        status = kiln_c99_temporal_read_bag(host, first, kind == 5, fields, field_texts, depth,
                                            thrown);
        carries_fields = 1;
        if (status == KILN_C99_OK) {
            status = kiln_c99_temporal_read_options(
                host, second, kiln_c99_temporal_overflow_keys(kind), &options, depth, thrown);
        }
    } else if (kiln_c99_same_name(name, strlen(name), "withCalendar") ||
               kiln_c99_same_name(name, strlen(name), "withTimeZone")) {
        if (first.tag == KILN_VALUE_UNDEFINED) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        kiln_c99_same_name(name, strlen(name), "withCalendar")
                                            ? "withCalendar needs a calendar"
                                            : "withTimeZone needs a time zone");
        }
        zone = kiln_c99_text(first);
        if (zone.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    } else if (kiln_c99_same_name(name, strlen(name), "withPlainTime")) {
        /* An absent argument means midnight rather than "leave it alone". */
        if (first.tag != KILN_VALUE_UNDEFINED) {
            status = kiln_c99_temporal_coerce(host, 1, first, kiln_c99_undefined(), &operand, depth,
                                              thrown);
        }
    } else if (kiln_c99_same_name(name, strlen(name), "toZonedDateTime") ||
               kiln_c99_same_name(name, strlen(name), "toZonedDateTimeISO")) {
        if (first.tag == KILN_VALUE_OBJECT) {
            kiln_value read;
            status = kiln_c99_get(host, first, "timeZone", 8, &read, depth, thrown);
            if (status == KILN_C99_OK && read.tag != KILN_VALUE_UNDEFINED) {
                zone = kiln_c99_text(read);
                if (zone.tag != KILN_VALUE_STRING) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, first, "plainTime", 9, &read, depth, thrown);
                if (status == KILN_C99_OK && read.tag != KILN_VALUE_UNDEFINED) {
                    status = kiln_c99_temporal_coerce(host, 1, read, kiln_c99_undefined(), &operand,
                                                      depth, thrown);
                }
            }
        } else if (first.tag != KILN_VALUE_UNDEFINED) {
            zone = kiln_c99_text(first);
            if (zone.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        if (status == KILN_C99_OK && zone.tag != KILN_VALUE_STRING) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "toZonedDateTime needs a time zone");
        }
        if (status == KILN_C99_OK) {
            /* A `PlainDate` names a whole day, so nothing about it can be skipped or repeated by a
             * zone; a `PlainDateTime` names a wall-clock time, which can be both. */
            status = kiln_c99_temporal_read_options(host, second,
                                                    kind == 2 ? KILN_C99_TEMPORAL_OPTS_DISAMBIGUATION
                                                              : KILN_C99_TEMPORAL_OPTS_NONE,
                                                    &options, depth, thrown);
        }
    } else if (kiln_c99_same_name(name, strlen(name), "toPlainDate") && (kind == 3 || kind == 4)) {
        /* On a year-month or a month-day, `toPlainDate` supplies the component that was missing
         * rather than dropping one that was there. */
        if (first.tag != KILN_VALUE_OBJECT) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "toPlainDate needs a property bag");
        }
        status = kiln_c99_temporal_read_bag(host, first, 0, fields, field_texts, depth, thrown);
        carries_fields = 1;
    } else if (kiln_c99_same_name(name, strlen(name), "toPlainDateTime") && kind == 0) {
        if (first.tag == KILN_VALUE_TEMPORAL) {
            operand = first;
        } else if (first.tag == KILN_VALUE_OBJECT) {
            status = kiln_c99_temporal_read_bag(host, first, 0, fields, field_texts, depth, thrown);
            carries_fields = 1;
        }
    } else if (kiln_c99_same_name(name, strlen(name), "toString")) {
        status = kiln_c99_temporal_read_options(host, first, kiln_c99_temporal_string_keys(kind),
                                                &options, depth, thrown);
    }
    if (status != KILN_C99_OK) {
        return status;
    }

    packed_fields = kiln_c99_temporal_pack(field_texts, KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS,
                                           field_lengths);
    if (packed_fields == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    packed_options = kiln_c99_temporal_pack(options.texts, KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS,
                                            option_lengths);
    if (packed_options == NULL) {
        free(packed_fields);
        return KILN_C99_OUT_OF_MEMORY;
    }

    call.receiver = kiln_c99_temporal_of(receiver);
    call.name = (const unsigned char *)name;
    call.name_len = strlen(name);
    call.operand = operand.tag == KILN_VALUE_TEMPORAL ? kiln_c99_temporal_of(operand) : NULL;
    call.relative_to = options.relative_to.tag == KILN_VALUE_TEMPORAL
                           ? kiln_c99_temporal_of(options.relative_to)
                           : NULL;
    call.argument = zone.tag == KILN_VALUE_STRING ? (const unsigned char *)zone.ptr : NULL;
    call.argument_len = zone.tag == KILN_VALUE_STRING ? zone.len : 0;
    call.fields = carries_fields ? fields : NULL;
    call.field_count = carries_fields ? 10u : 0u;
    call.field_text = (const unsigned char *)packed_fields;
    call.field_text_lengths = field_lengths;
    call.field_text_count = KILN_C99_TEMPORAL_FIELD_TEXT_SLOTS;
    call.option_numbers = options.numbers;
    call.option_number_count = KILN_C99_TEMPORAL_OPTION_NUMBER_SLOTS;
    call.option_text = (const unsigned char *)packed_options;
    call.option_text_lengths = option_lengths;
    call.option_text_count = KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS;
    call.out_handle = &handle;
    call.out_number = &number;
    call.out_buffer = (unsigned char *)inline_buffer;
    call.out_capacity = sizeof inline_buffer;
    call.out_length = &length;
    answered = kiln_temporal_invoke(&call);
    if (answered == 2 && length < 0) {
        /* The retry protocol: one negative answer says exactly how much to allocate. */
        size_t wanted = (size_t)(-length);
        char *spilled = (char *)malloc(wanted);
        if (spilled == NULL) {
            free(packed_fields);
            free(packed_options);
            return KILN_C99_OUT_OF_MEMORY;
        }
        call.out_buffer = (unsigned char *)spilled;
        call.out_capacity = wanted;
        answered = kiln_temporal_invoke(&call);
        if (answered != 2 || length < 0) {
            free(spilled);
            free(packed_fields);
            free(packed_options);
            KILN_C99_REFUSE("a Temporal answer the provider would not produce into a grown buffer");
        }
        *result = kiln_c99_copy_text(spilled, (size_t)length);
        free(spilled);
        free(packed_fields);
        free(packed_options);
        return KILN_C99_OK;
    }
    free(packed_fields);
    free(packed_options);
    if (answered == 6) {
        /* The engine has no such operation on this type -- `startOfDay` on a `PlainDate`. That is
         * a TypeError in the language, which the engine reports when the operation is attempted;
         * reaching here means the name is one this backend routes and the type is not one that
         * carries it. */
        return kiln_c99_temporal_unsupported(thrown, kiln_c99_temporal_tag(receiver), name,
                                             strlen(name));
    }
    return kiln_c99_temporal_answer(answered, handle, number, length, inline_buffer, result,
                                    thrown);
}

/* The `Temporal` methods this backend answers. Everything else refuses by name where it is read. */
static int kiln_c99_temporal_method(kiln_host *host, kiln_value *result, uint64_t code,
                                    kiln_value receiver, const kiln_value *args, size_t argc,
                                    int depth, kiln_value *thrown) {
    if (kiln_c99_method_in_group(code, KILN_C99_GROUP_TEMPORAL_FROM)) {
        int32_t kind = (int32_t)(code - KILN_C99_METHOD_TEMPORAL_FROM);
        kiln_value item = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value options = argc > 1 ? args[1] : kiln_c99_undefined();
        return kiln_c99_temporal_coerce(host, kind, item, options, result, depth, thrown);
    }
    if (kiln_c99_method_in_group(code, KILN_C99_GROUP_TEMPORAL_COMPARE)) {
        int32_t kind = (int32_t)(code - KILN_C99_METHOD_TEMPORAL_COMPARE);
        kiln_value left;
        kiln_value right;
        kiln_temporal_options options;
        int64_t option_lengths[KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS];
        char *packed_options;
        int32_t order = 0;
        int32_t answered;
        int status = kiln_c99_temporal_coerce(host, kind, argc > 0 ? args[0] : kiln_c99_undefined(),
                                              kiln_c99_undefined(), &left, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        status = kiln_c99_temporal_coerce(host, kind, argc > 1 ? args[1] : kiln_c99_undefined(),
                                          kiln_c99_undefined(), &right, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        kiln_c99_temporal_options_init(&options);
        if (kind == 5) {
            /* `Temporal.Duration.compare` takes its options at the *third* argument, because the
             * first two are the durations. Nothing else here takes any. */
            status = kiln_c99_temporal_read_options(host, argc > 2 ? args[2] : kiln_c99_undefined(),
                                                    KILN_C99_TEMPORAL_OPTS_RELATIVE_TO, &options,
                                                    depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        packed_options = kiln_c99_temporal_pack(options.texts, KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS,
                                                option_lengths);
        if (packed_options == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        answered = kiln_temporal_compare(
            kiln_c99_temporal_of(left), kiln_c99_temporal_of(right),
            options.relative_to.tag == KILN_VALUE_TEMPORAL
                ? kiln_c99_temporal_of(options.relative_to)
                : NULL,
            (const unsigned char *)packed_options, option_lengths,
            KILN_C99_TEMPORAL_OPTION_TEXT_SLOTS, &order);
        free(packed_options);
        if (answered != 0) {
            return kiln_c99_temporal_raise(thrown, answered);
        }
        *result = kiln_c99_number((double)order);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID) {
        /* ADR 0015: the realm's zone is UTC, and it says so rather than leaving it to be found. */
        *result = kiln_c99_copy_text("UTC", 3);
        return KILN_C99_OK;
    }
    if (kiln_c99_method_in_group(code, KILN_C99_GROUP_TEMPORAL_NOW_READ)) {
        /* The realm's clock, not the host's -- the same virtual instant `Date.now()` reads, so
         * two reads with no timer between them agree (ADR 0015). It arrives in whole nanoseconds
         * because that is the unit `Temporal` measures instants in; reading it through the
         * millisecond accessor would round the clock and put the two engines a fraction apart. */
        int32_t kind = (int32_t)(code - KILN_C99_METHOD_TEMPORAL_NOW_READ);
        kiln_value zone = argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED
                              ? kiln_c99_text(args[0])
                              : kiln_c99_undefined();
        int64_t high = 0;
        uint64_t low = 0;
        void *handle = NULL;
        int32_t answered;
        if (zone.tag != KILN_VALUE_UNDEFINED && zone.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (kiln_c99_wall_clock_nanoseconds(host, &high, &low) != KILN_C99_OK) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "the realm clock is not readable here");
        }
        answered = kiln_temporal_now(
            (uint32_t)kind, high, low,
            zone.tag == KILN_VALUE_STRING ? (const unsigned char *)zone.ptr : NULL,
            zone.tag == KILN_VALUE_STRING ? zone.len : 0, &handle);
        if (answered != 0) {
            return kiln_c99_temporal_raise(thrown, answered);
        }
        *result = kiln_c99_temporal_value(handle);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS) {
        /* `ToNumber` first, and through the checked path, because the argument may be an object
         * with a `valueOf` and running it is observable. A fractional or out-of-range count is the
         * engine's RangeError, not a truncation. */
        double milliseconds = 0.0;
        void *handle = NULL;
        int32_t answered;
        int status = kiln_c99_to_number_checked(host, argc > 0 ? args[0] : kiln_c99_undefined(),
                                                &milliseconds, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        answered = kiln_temporal_instant_from_epoch_milliseconds(milliseconds, &handle);
        if (answered != 0) {
            return kiln_c99_temporal_raise(thrown, answered);
        }
        *result = kiln_c99_temporal_value(handle);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS) {
        /* A BigInt and nothing else -- not a number coerced to one. Nanoseconds since the epoch
         * pass what a double holds exactly within four months of 1970, so the language refuses a
         * number here rather than accepting one it would have to round. */
        kiln_value argument = argc > 0 ? args[0] : kiln_c99_undefined();
        int64_t high = 0;
        uint64_t low = 0;
        void *handle = NULL;
        int32_t answered;
        if (argument.tag != KILN_VALUE_BIGINT) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Temporal.Instant.fromEpochNanoseconds needs a BigInt");
        }
        if (kiln_bigint_to_i128((const void *)(uintptr_t)argument.bits, &high, &low) != 0) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "epochNanoseconds is outside the representable range");
        }
        answered = kiln_temporal_instant_from_epoch_nanoseconds(high, low, &handle);
        if (answered != 0) {
            return kiln_c99_temporal_raise(thrown, answered);
        }
        *result = kiln_c99_temporal_value(handle);
        return KILN_C99_OK;
    }
    if (receiver.tag != KILN_VALUE_TEMPORAL) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "a Temporal method needs a Temporal receiver");
    }
    if (code == KILN_C99_METHOD_TEMPORAL_TO_STRING || code == KILN_C99_METHOD_TEMPORAL_TO_JSON) {
        return kiln_c99_temporal_string(result, receiver, thrown);
    }
    if (code == KILN_C99_METHOD_TEMPORAL_VALUE_OF) {
        /* Specified to throw: `date1 < date2` would otherwise compare two NaNs and answer false,
         * which is exactly the silent wrong answer the language added this refusal to prevent. */
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "a Temporal value cannot be converted to a primitive; use compare or equals");
    }
    if (kiln_c99_method_in_group(code, KILN_C99_GROUP_TEMPORAL_OP)) {
        return kiln_c99_temporal_operation(host, result, code, receiver, args, argc, depth, thrown);
    }
    KILN_C99_REFUSE("a Temporal method this backend has not implemented");
}
