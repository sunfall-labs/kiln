
/* Days-to-civil and back: Howard Hinnant's algorithm, the same one the interpreter uses. The two
 * have to agree exactly, so this is a transcription rather than a reimplementation. */
static void kiln_c99_civil_from_days(int64_t days, int64_t *year, int *month, int *day) {
    int64_t shifted = days + 719468;
    int64_t era = (shifted >= 0 ? shifted : shifted - 146096) / 146097;
    int64_t day_of_era = shifted - era * 146097;
    int64_t year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    int64_t computed_year = year_of_era + era * 400;
    int64_t day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    int64_t shifted_month = (5 * day_of_year + 2) / 153;
    int computed_day = (int)(day_of_year - (153 * shifted_month + 2) / 5 + 1);
    int computed_month = (int)(shifted_month < 10 ? shifted_month + 3 : shifted_month - 9);
    *year = computed_year + (computed_month <= 2 ? 1 : 0);
    *month = computed_month;
    *day = computed_day;
}

static int64_t kiln_c99_days_from_civil(int64_t year, int64_t month, int64_t day) {
    int64_t era;
    int64_t year_of_era;
    int64_t shifted_month;
    int64_t day_of_year;
    int64_t day_of_era;
    year -= month <= 2 ? 1 : 0;
    era = (year >= 0 ? year : year - 399) / 400;
    year_of_era = year - era * 400;
    shifted_month = month > 2 ? month - 3 : month + 9;
    day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    return era * 146097 + day_of_era - 719468;
}

/* Floor division, not truncation: an instant before the epoch is a negative count and has to
 * round *down* to the day containing it. */
static int64_t kiln_c99_floor_div(int64_t value, int64_t divisor) {
    int64_t quotient = value / divisor;
    return (value % divisor != 0 && ((value < 0) != (divisor < 0))) ? quotient - 1 : quotient;
}

static int64_t kiln_c99_floor_mod(int64_t value, int64_t divisor) {
    int64_t remainder = value % divisor;
    return remainder != 0 && ((remainder < 0) != (divisor < 0)) ? remainder + divisor : remainder;
}

static kiln_date_parts kiln_c99_date_parts(double time) {
    kiln_date_parts parts;
    int64_t millis;
    int64_t days;
    int64_t within;
    parts.valid = 0;
    if (!isfinite(time)) {
        return parts;
    }
    millis = (int64_t)time;
    days = kiln_c99_floor_div(millis, 86400000);
    within = kiln_c99_floor_mod(millis, 86400000);
    kiln_c99_civil_from_days(days, &parts.year, &parts.month, &parts.day);
    /* 1970-01-01 was a Thursday, which is day 4. */
    parts.weekday = (int)kiln_c99_floor_mod(days + 4, 7);
    parts.hour = (int)(within / 3600000);
    parts.minute = (int)(within / 60000 % 60);
    parts.second = (int)(within / 1000 % 60);
    parts.millisecond = (int)(within % 1000);
    parts.valid = 1;
    return parts;
}

static double kiln_c99_date_time(kiln_value value) {
    return *(double *)(uintptr_t)value.bits;
}

static int kiln_c99_new_date(kiln_value *out, double time) {
    kiln_date *slot = (kiln_date *)kiln_c99_gc_alloc(KILN_GC_DATE, sizeof *slot);
    if (slot == NULL) return KILN_C99_OUT_OF_MEMORY;
    slot->time = time;
    kiln_c99_heap_store_value(slot, &slot->properties, kiln_c99_undefined());
    *out = kiln_c99_value(KILN_VALUE_DATE, (uint64_t)(uintptr_t)slot, NULL, 0);
    return KILN_C99_OK;
}

/* Fixed-width decimal, because every field of an ISO instant is written to its own width and
 * `kiln_c99_double_to_js` writes the shortest form instead. */
static int kiln_c99_pad_number(kiln_text *text, int64_t value, int width) {
    char digits[24];
    int at = (int)sizeof digits;
    int negative = value < 0;
    uint64_t magnitude = negative ? (uint64_t)(-value) : (uint64_t)value;
    do {
        digits[--at] = (char)('0' + (magnitude % 10));
        magnitude /= 10;
    } while (magnitude != 0);
    while ((int)sizeof digits - at < width) {
        digits[--at] = '0';
    }
    if (negative && kiln_c99_text_append(text, "-", 1) != KILN_C99_OK) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    return kiln_c99_text_append(text, digits + at, (size_t)((int)sizeof digits - at));
}

/* `toISOString`, or a string with no contents when the instant has no ISO spelling -- the caller
 * decides whether that is a throw or a null, because `toISOString` and `toJSON` differ there. */
static int kiln_c99_iso_string(kiln_value *out, double time) {
    kiln_date_parts parts = kiln_c99_date_parts(time);
    kiln_text text = {NULL, 0, 0};
    if (!parts.valid) {
        KILN_C99_ABSENT("not a member read: an instant with no ISO spelling", out);
    }
    /* A year outside four digits is written with a sign and six, which is the only shape that
     * stays unambiguous. */
    if (parts.year >= 0 && parts.year <= 9999) {
        if (kiln_c99_pad_number(&text, parts.year, 4) != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
    } else {
        if (kiln_c99_text_append_c(&text, parts.year < 0 ? "-" : "+") != KILN_C99_OK ||
            kiln_c99_pad_number(&text, parts.year < 0 ? -parts.year : parts.year, 6) !=
                KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    if (kiln_c99_text_append_c(&text, "-") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.month, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, "-") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.day, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, "T") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.hour, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, ":") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.minute, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, ":") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.second, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, ".") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.millisecond, 3) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, "Z") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    *out = kiln_c99_text_value(&text);
    return KILN_C99_OK;
}

/* `String(date)`: the human form, which is UTC here because the realm has no other zone. */
static int kiln_c99_date_to_string(kiln_value *out, double time) {
    static const char *const WEEKDAYS[7] = {"Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"};
    static const char *const MONTHS[12] = {"Jan", "Feb", "Mar", "Apr", "May", "Jun",
                                           "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"};
    kiln_date_parts parts = kiln_c99_date_parts(time);
    kiln_text text = {NULL, 0, 0};
    if (!parts.valid) {
        *out = kiln_c99_copy_text("Invalid Date", 12);
        return KILN_C99_OK;
    }
    if (kiln_c99_text_append_c(&text, WEEKDAYS[parts.weekday]) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, " ") != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, MONTHS[parts.month - 1]) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, " ") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.day, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, " ") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.year, 4) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, " ") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.hour, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, ":") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.minute, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, ":") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.second, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, " GMT+0000 (Coordinated Universal Time)") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    *out = kiln_c99_text_value(&text);
    return KILN_C99_OK;
}
