/* The exact decimal expansion of `value`'s magnitude, little-endian: `digits[0]` is the least
 * significant, and `*fractional` of them sit below the decimal point. Returns the digit count.
 *
 * Shared by the three renderings that have to round -- `toFixed`, `toExponential`,
 * `toPrecision` -- because all three must round on the value's *own* digits. That is the whole
 * argument of the comment above: scale by a power of ten first and the deciding digit is gone
 * before the language ever looks at it. Writing the expansion once means the three cannot come
 * to different conclusions about the same double.
 *
 * `value` must be finite; the callers answer the infinities and NaN from `ToString` instead. */
static size_t kiln_c99_exact_digits(double value, unsigned char *digits, size_t capacity,
                                    size_t *fractional) {
    size_t count = 0;
    size_t index;
    double mantissa;
    uint64_t scaled;
    int exponent;
    mantissa = frexp(fabs(value), &exponent);
    scaled = (uint64_t)ldexp(mantissa, 53);
    exponent -= 53;
    while (scaled != 0 && count < capacity) {
        digits[count++] = (unsigned char)(scaled % 10u);
        scaled /= 10u;
    }
    if (count == 0) {
        digits[count++] = 0;
    }
    if (exponent >= 0) {
        int step;
        for (step = 0; step < exponent; ++step) {
            unsigned carry = 0;
            for (index = 0; index < count; ++index) {
                unsigned product = (unsigned)digits[index] * 2u + carry;
                digits[index] = (unsigned char)(product % 10u);
                carry = product / 10u;
            }
            while (carry != 0 && count < capacity) {
                digits[count++] = (unsigned char)(carry % 10u);
                carry /= 10u;
            }
        }
        *fractional = 0;
    } else {
        int step;
        for (step = 0; step < -exponent; ++step) {
            unsigned carry = 0;
            for (index = 0; index < count; ++index) {
                unsigned product = (unsigned)digits[index] * 5u + carry;
                digits[index] = (unsigned char)(product % 10u);
                carry = product / 10u;
            }
            while (carry != 0 && count < capacity) {
                digits[count++] = (unsigned char)(carry % 10u);
                carry /= 10u;
            }
        }
        *fractional = (size_t)(-exponent);
    }
    /* Pad so there is a digit at every place the answer names, above the point and below it. */
    while (count < *fractional + 1 && count < capacity) {
        digits[count++] = 0;
    }
    return count;
}

static int kiln_c99_to_fixed(kiln_value *out, double value, int places) {
    /* Little-endian decimal digits: `digits[0]` is the least significant. */
    unsigned char digits[1200];
    size_t count;
    size_t fractional;
    size_t index;
    kiln_text text = {NULL, 0, 0};
    int negative = value < 0.0;
    int round_up;
    if (!isfinite(value) || fabs(value) >= 1e21) {
        *out = kiln_c99_text(kiln_c99_number(value));
        return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
    }
    count = kiln_c99_exact_digits(value, digits, sizeof digits, &fractional);
    if (fractional <= (size_t)places) {
        round_up = 0;
    } else {
        /* Round half *up*: "if there are two such n, pick the larger n". Only the deciding digit
         * is consulted, because at or above five the answer is up whatever follows. */
        round_up = digits[fractional - (size_t)places - 1] >= 5;
        for (index = 0; index < fractional - (size_t)places; ++index) {
            digits[index] = 0;
        }
        memmove(digits, digits + (fractional - (size_t)places),
                count - (fractional - (size_t)places));
        count -= fractional - (size_t)places;
        fractional = (size_t)places;
    }
    while (fractional < (size_t)places && count < sizeof digits) {
        /* The value terminated above the requested precision, so the rest is zeros. */
        memmove(digits + 1, digits, count);
        digits[0] = 0;
        count += 1;
        fractional += 1;
    }
    if (round_up) {
        for (index = 0; index < count; ++index) {
            if (digits[index] == 9) {
                digits[index] = 0;
                continue;
            }
            digits[index] += 1;
            break;
        }
        if (index == count && count < sizeof digits) {
            digits[count++] = 1;
        }
    }
    while (count > fractional + 1 && digits[count - 1] == 0) {
        /* A leading zero above the point is not part of the spelling, but the one before the
         * point is: `.5` is `0.5`. */
        count -= 1;
    }
    /* The sign survives rounding: `(-0.0004).toFixed(2)` is `"-0.00"`, which says the value was
     * negative rather than that it was nonzero. Negative zero is not negative here, so
     * `(-0).toFixed(2)` is `"0.00"`. */
    if (negative && kiln_c99_text_append_c(&text, "-") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    for (index = count; index > 0; --index) {
        char character = (char)('0' + digits[index - 1]);
        if (index == fractional && kiln_c99_text_append_c(&text, ".") != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (kiln_c99_text_append(&text, &character, 1) != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    *out = kiln_c99_text_value(&text);
    return KILN_C99_OK;
}

/* The `n` and `e` the specification's exponential formats name, for a finite value.
 *
 * `count` significant digits of the *exact* expansion of the magnitude, rounded half up -- "if
 * there are two such sets, pick the larger n" -- written most significant first into `mantissa`
 * and NUL terminated, and the decimal exponent of the first of them in `*exponent`. `mantissa`
 * must have room for `count + 1` bytes. */
static void kiln_c99_significant_digits(double value, size_t count, char *mantissa,
                                        int *exponent) {
    unsigned char digits[1200];
    size_t total;
    size_t fractional;
    size_t point;
    size_t first;
    size_t index;
    size_t carry_at;
    total = kiln_c99_exact_digits(value, digits, sizeof digits, &fractional);
    /* `digits` is little-endian, so big-endian position `i` is `digits[total - 1 - i]`, and
     * `point` of those sit above the decimal point. */
    point = total - fractional;
    for (first = 0; first < total && digits[total - 1 - first] == 0; ++first) {
    }
    if (first == total) {
        /* Zero has no significant digit: the specification names its mantissa as zeros and its
         * exponent as zero. */
        for (index = 0; index < count; ++index) {
            mantissa[index] = '0';
        }
        mantissa[count] = '\0';
        *exponent = 0;
        return;
    }
    /* The digit at `first` sits in the place worth 10^(point - 1 - first). */
    *exponent = (int)point - 1 - (int)first;
    for (index = 0; index < count; ++index) {
        size_t at = first + index;
        mantissa[index] = at < total ? (char)('0' + digits[total - 1 - at]) : '0';
    }
    mantissa[count] = '\0';
    /* Only the deciding digit is consulted: at or above five the answer rounds up whatever
     * follows it. A place past the end of the exact expansion is a zero and decides nothing. */
    if (first + count < total && digits[total - 1 - (first + count)] >= 5) {
        carry_at = count;
        for (;;) {
            if (carry_at == 0) {
                /* The carry ran off the front: every digit was a nine, so the mantissa is a one
                 * followed by zeros and the exponent moved up one. */
                mantissa[0] = '1';
                for (index = 1; index < count; ++index) {
                    mantissa[index] = '0';
                }
                *exponent += 1;
                break;
            }
            carry_at -= 1;
            if (mantissa[carry_at] == '9') {
                mantissa[carry_at] = '0';
                continue;
            }
            mantissa[carry_at] += 1;
            break;
        }
    }
}

/* The *shortest* mantissa and decimal exponent that name a finite non-negative value exactly,
 * which is what `toExponential` with no argument asks for.
 *
 * Read off the number provider's own shortest spelling rather than decided again here. That
 * spelling is already the fewest digits that round-trip (ADR 0018), so reading it is the only
 * way `(1234.5678).toExponential()` and `String(1234.5678)` cannot come to disagree about how
 * many digits the value needs. */
static int kiln_c99_shortest_digits(double value, char *mantissa, size_t capacity, int *exponent) {
    kiln_value spelled = kiln_c99_text(kiln_c99_number(value));
    const char *bytes;
    size_t len;
    size_t body;
    size_t index;
    size_t produced = 0;
    size_t point;
    size_t first;
    size_t last;
    char raw[64];
    int suffix = 0;
    int suffix_sign = 1;
    if (spelled.tag != KILN_VALUE_STRING) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    bytes = (const char *)spelled.ptr;
    len = spelled.len;
    /* `1e+21` and above, and `1e-7` and below, are spelled with an exponent already; everything
     * between is spelled positionally. Both forms reduce to the same digits-and-a-place. */
    body = len;
    for (index = 0; index < len; ++index) {
        if (bytes[index] == 'e' || bytes[index] == 'E') {
            size_t at = index + 1;
            body = index;
            if (at < len && (bytes[at] == '+' || bytes[at] == '-')) {
                suffix_sign = bytes[at] == '-' ? -1 : 1;
                at += 1;
            }
            for (; at < len; ++at) {
                if (bytes[at] < '0' || bytes[at] > '9') {
                    break;
                }
                suffix = suffix * 10 + (bytes[at] - '0');
            }
            suffix *= suffix_sign;
            break;
        }
    }
    point = (size_t)-1;
    for (index = 0; index < body && produced < sizeof raw; ++index) {
        if (bytes[index] == '.') {
            point = produced;
            continue;
        }
        if (bytes[index] < '0' || bytes[index] > '9') {
            /* A sign; the caller asked about a magnitude. */
            continue;
        }
        raw[produced++] = bytes[index];
    }
    if (point == (size_t)-1) {
        point = produced;
    }
    for (first = 0; first < produced && raw[first] == '0'; ++first) {
    }
    if (first == produced) {
        if (capacity < 2) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        mantissa[0] = '0';
        mantissa[1] = '\0';
        *exponent = 0;
        return KILN_C99_OK;
    }
    /* Trailing zeros are place-holding, not significant: `100` is `1e+2`, not `100e+0`. */
    for (last = produced; last > first + 1 && raw[last - 1] == '0'; --last) {
    }
    if (last - first + 1 > capacity) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    for (index = first; index < last; ++index) {
        mantissa[index - first] = raw[index];
    }
    mantissa[last - first] = '\0';
    *exponent = (int)point - 1 - (int)first + suffix;
    return KILN_C99_OK;
}

/* One mantissa and exponent as both exponential formats write them: `1.25e+2`. */
static int kiln_c99_exponential_text(kiln_value *out, int negative, const char *mantissa,
                                     int exponent) {
    kiln_text text = {NULL, 0, 0};
    char rendered[16];
    size_t length = strlen(mantissa);
    unsigned magnitude = exponent < 0 ? (unsigned)-(long)exponent : (unsigned)exponent;
    size_t at = sizeof rendered;
    int failed = 0;
    if (negative) {
        failed = failed || kiln_c99_text_append_c(&text, "-") != KILN_C99_OK;
    }
    failed = failed || kiln_c99_text_append(&text, mantissa, 1) != KILN_C99_OK;
    if (length > 1) {
        failed = failed || kiln_c99_text_append_c(&text, ".") != KILN_C99_OK;
        failed = failed || kiln_c99_text_append(&text, mantissa + 1, length - 1) != KILN_C99_OK;
    }
    failed = failed || kiln_c99_text_append_c(&text, "e") != KILN_C99_OK;
    /* The sign is always written, which is the whole difference from `String(n)`'s own
     * exponential form for a positive exponent. */
    failed = failed || kiln_c99_text_append_c(&text, exponent < 0 ? "-" : "+") != KILN_C99_OK;
    do {
        rendered[--at] = (char)('0' + magnitude % 10u);
        magnitude /= 10u;
    } while (magnitude != 0 && at > 0);
    failed = failed ||
             kiln_c99_text_append(&text, rendered + at, sizeof rendered - at) != KILN_C99_OK;
    if (failed) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    *out = kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
    return KILN_C99_OK;
}

/* `toPrecision`'s positional form, and its own window for when to give up on one.
 *
 * The switch to the exponential form is the specification's, and it is what keeps
 * `(0.0000001).toPrecision(2)` from being a string of zeros with no digits in it. */
static int kiln_c99_precision_text(kiln_value *out, int negative, const char *mantissa,
                                   size_t digits, int exponent) {
    kiln_text text = {NULL, 0, 0};
    size_t length = strlen(mantissa);
    size_t split;
    int index;
    int failed = 0;
    if (exponent < -6 || exponent >= (int)digits) {
        return kiln_c99_exponential_text(out, negative, mantissa, exponent);
    }
    if (negative) {
        failed = failed || kiln_c99_text_append_c(&text, "-") != KILN_C99_OK;
    }
    if (exponent == (int)digits - 1) {
        /* Every significant digit is above the point, so there is no point. */
        failed = failed || kiln_c99_text_append(&text, mantissa, length) != KILN_C99_OK;
    } else if (exponent >= 0) {
        split = (size_t)exponent + 1;
        failed = failed || kiln_c99_text_append(&text, mantissa, split) != KILN_C99_OK;
        failed = failed || kiln_c99_text_append_c(&text, ".") != KILN_C99_OK;
        failed = failed ||
                 kiln_c99_text_append(&text, mantissa + split, length - split) != KILN_C99_OK;
    } else {
        failed = failed || kiln_c99_text_append_c(&text, "0.") != KILN_C99_OK;
        for (index = 0; index < -exponent - 1; ++index) {
            failed = failed || kiln_c99_text_append_c(&text, "0") != KILN_C99_OK;
        }
        failed = failed || kiln_c99_text_append(&text, mantissa, length) != KILN_C99_OK;
    }
    if (failed) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    *out = kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
    return KILN_C99_OK;
}

/* A number written in a base other than ten, which `Number.prototype.toString` is the only way
 * to ask for. Digits are produced by repeated division rather than by the number formatter,
 * because the formatter only knows base ten -- and the fraction is bounded at 52 digits, which is
 * every bit a double has to say. */
static int kiln_c99_number_in_radix(kiln_value *out, double value, int radix) {
    kiln_text text = {NULL, 0, 0};
    static const char alphabet[] = "0123456789abcdefghijklmnopqrstuvwxyz";
    char digits[1120];
    size_t count = 0;
    double whole;
    double fraction;
    int negative;
    if (!isfinite(value)) {
        *out = kiln_c99_text(kiln_c99_number(value));
        return KILN_C99_OK;
    }
    negative = value < 0.0;
    value = fabs(value);
    whole = trunc(value);
    fraction = value - whole;
    while (whole >= 1.0 && count < sizeof digits) {
        digits[count++] = alphabet[(int)fmod(whole, (double)radix)];
        whole = trunc(whole / (double)radix);
    }
    if (negative && kiln_c99_text_append_c(&text, "-") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    if (count == 0 && kiln_c99_text_append_c(&text, "0") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    while (count > 0) {
        count -= 1;
        if (kiln_c99_text_append(&text, digits + count, 1) != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    if (fraction > 0.0) {
        int place;
        if (kiln_c99_text_append_c(&text, ".") != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (place = 0; place < 52 && fraction > 0.0; ++place) {
            double digit;
            char character;
            fraction *= (double)radix;
            digit = trunc(fraction);
            character = alphabet[(int)digit];
            fraction -= digit;
            if (kiln_c99_text_append(&text, &character, 1) != KILN_C99_OK) {
                free(text.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
    }
    *out = kiln_c99_text_value(&text);
    return KILN_C99_OK;
}
