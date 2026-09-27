/* ECMAScript StringNumericLiteral conversion for the native C backend. */

#include <math.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* ffc is locale-independent and correctly rounded for decimal input. */
#if defined(__clang__)
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wstatic-in-inline"
#endif
#define FFC_IMPL
#include "ffc.h"
#if defined(__clang__)
#pragma clang diagnostic pop
#endif

static bool kiln_es_whitespace(uint32_t cp) {
    return cp == UINT32_C(0x0009) || cp == UINT32_C(0x000a) ||
           cp == UINT32_C(0x000b) || cp == UINT32_C(0x000c) ||
           cp == UINT32_C(0x000d) || cp == UINT32_C(0x0020) ||
           cp == UINT32_C(0x00a0) || cp == UINT32_C(0x1680) ||
           (cp >= UINT32_C(0x2000) && cp <= UINT32_C(0x200a)) ||
           cp == UINT32_C(0x2028) || cp == UINT32_C(0x2029) ||
           cp == UINT32_C(0x202f) || cp == UINT32_C(0x205f) ||
           cp == UINT32_C(0x3000) || cp == UINT32_C(0xfeff);
}

/* Decode one strict UTF-8 sequence.  This also rejects surrogates. */
static bool kiln_utf8(const unsigned char *s, size_t n, size_t at,
                      uint32_t *cp, size_t *width) {
    unsigned char first;
    uint32_t value;
    size_t count;
    size_t i;

    if (at >= n) return false;
    first = s[at];
    if (first < 0x80u) {
        *cp = first;
        *width = 1;
        return true;
    }
    if (first >= 0xc2u && first <= 0xdfu) {
        value = (uint32_t)(first & 0x1fu);
        count = 2;
    } else if (first >= 0xe0u && first <= 0xefu) {
        value = (uint32_t)(first & 0x0fu);
        count = 3;
    } else if (first >= 0xf0u && first <= 0xf4u) {
        value = (uint32_t)(first & 0x07u);
        count = 4;
    } else {
        return false;
    }
    if (count > n - at) return false;
    for (i = 1; i < count; ++i) {
        unsigned char continuation = s[at + i];
        if ((continuation & 0xc0u) != 0x80u) return false;
        value = (value << 6) | (uint32_t)(continuation & 0x3fu);
    }
    if ((count == 3 && value < UINT32_C(0x800)) ||
        (count == 4 && value < UINT32_C(0x10000)) ||
        (value >= UINT32_C(0xd800) && value <= UINT32_C(0xdfff)) ||
        value > UINT32_C(0x10ffff)) {
        return false;
    }
    *cp = value;
    *width = count;
    return true;
}

static bool kiln_ascii_digit(char c) {
    return c >= '0' && c <= '9';
}

static int kiln_radix_digit(char c) {
    if (c >= '0' && c <= '9') return (int)(c - '0');
    if (c >= 'a' && c <= 'f') return (int)(c - 'a') + 10;
    if (c >= 'A' && c <= 'F') return (int)(c - 'A') + 10;
    return -1;
}

static unsigned kiln_high_bit(unsigned value) {
    unsigned result = 0;
    while (value != 0u) {
        ++result;
        value >>= 1;
    }
    return result;
}

/* Convert an unsigned 2/8/16-based integer, retaining guard and sticky bits. */
static double kiln_radix_integer(const char *s, size_t len, unsigned base,
                                unsigned bits_per_digit) {
    size_t digits = len - 2;
    size_t first_nonzero = digits;
    size_t i;
    unsigned first_high = 0;
    size_t tail;
    size_t bit_length;
    uint64_t top = 0;
    size_t seen = 0;
    bool guard = false;
    bool sticky = false;

    if (digits == 0) return NAN;
    for (i = 0; i < digits; ++i) {
        int digit = kiln_radix_digit(s[2 + i]);
        if (digit < 0 || (unsigned)digit >= base) return NAN;
        if (first_nonzero == digits && digit != 0) {
            first_nonzero = i;
            first_high = kiln_high_bit((unsigned)digit);
        }
    }
    if (first_nonzero == digits) return 0.0;

    tail = digits - first_nonzero - 1;
    if (tail > (SIZE_MAX - (size_t)first_high) / (size_t)bits_per_digit) {
        return INFINITY;
    }
    bit_length = (size_t)first_high + tail * (size_t)bits_per_digit;
    if (bit_length > 1024u) return INFINITY;

    for (i = first_nonzero; i < digits; ++i) {
        unsigned digit = (unsigned)kiln_radix_digit(s[2 + i]);
        unsigned width = i == first_nonzero ? first_high : bits_per_digit;
        int bit;
        for (bit = (int)width - 1; bit >= 0; --bit) {
            bool one = (digit & (1u << (unsigned)bit)) != 0u;
            if (seen < 53u) {
                top = (top << 1) | (one ? UINT64_C(1) : UINT64_C(0));
            } else if (seen == 53u) {
                guard = one;
            } else if (one) {
                sticky = true;
            }
            ++seen;
        }
    }
    if (bit_length <= 53u) return (double)top;
    if (guard && (sticky || (top & UINT64_C(1)) != 0u)) ++top;
    return ldexp((double)top, (int)(bit_length - 53u));
}

static bool kiln_decimal_grammar(const char *s, size_t len) {
    size_t at = 0;
    size_t digits = 0;
    size_t exponent_digits = 0;

    while (at < len && kiln_ascii_digit(s[at])) {
        ++at;
        ++digits;
    }
    if (at < len && s[at] == '.') {
        ++at;
        while (at < len && kiln_ascii_digit(s[at])) {
            ++at;
            ++digits;
        }
    }
    if (digits == 0) return false;
    if (at < len && (s[at] == 'e' || s[at] == 'E')) {
        ++at;
        if (at < len && (s[at] == '+' || s[at] == '-')) ++at;
        while (at < len && kiln_ascii_digit(s[at])) {
            ++at;
            ++exponent_digits;
        }
        if (exponent_digits == 0) return false;
    }
    return at == len;
}

static bool kiln_trim_and_validate(const char *bytes, size_t len,
                                   size_t *start, size_t *end) {
    const unsigned char *input = (const unsigned char *)bytes;
    size_t at = 0;
    size_t width;
    uint32_t cp;

    while (at < len) {
        if (input[at] == 0) return false;
        if (!kiln_utf8(input, len, at, &cp, &width)) return false;
        at += width;
    }
    at = 0;
    while (at < len) {
        if (!kiln_utf8(input, len, at, &cp, &width)) return false;
        if (!kiln_es_whitespace(cp)) break;
        at += width;
    }
    *start = at;
    at = len;
    while (at > *start) {
        size_t candidate = at - 1;
        while (candidate > *start && (input[candidate] & 0xc0u) == 0x80u) --candidate;
        if (!kiln_utf8(input, len, candidate, &cp, &width) || candidate + width != at) {
            return false;
        }
        if (!kiln_es_whitespace(cp)) break;
        at = candidate;
    }
    *end = at;
    return true;
}

double kiln_string_to_number(const char *bytes, size_t len) {
    size_t start;
    size_t end;
    const char *s;
    size_t n;
    bool negative = false;
    bool signed_value = false;
    ffc_result parsed;
    double value;

    if (len == 0) return 0.0;
    if (bytes == NULL || !kiln_trim_and_validate(bytes, len, &start, &end)) return NAN;
    if (start == end) return 0.0;
    s = bytes + start;
    n = end - start;
    while (start < end && (bytes[start] & 0x80u) == 0) ++start;
    if (start != end) return NAN;

    if (s[0] == '+' || s[0] == '-') {
        negative = s[0] == '-';
        signed_value = true;
        ++s;
        --n;
        if (n == 0) return NAN;
    }
    if (n == 8 && s[0] == 'I' && s[1] == 'n' && s[2] == 'f' &&
        s[3] == 'i' && s[4] == 'n' && s[5] == 'i' && s[6] == 't' &&
        s[7] == 'y') {
        return negative ? -INFINITY : INFINITY;
    }
    if (n >= 2 && s[0] == '0' && (s[1] == 'x' || s[1] == 'X')) {
        if (signed_value) return NAN;
        return kiln_radix_integer(s, n, 16u, 4u);
    }
    if (n >= 2 && s[0] == '0' && (s[1] == 'o' || s[1] == 'O')) {
        if (signed_value) return NAN;
        return kiln_radix_integer(s, n, 8u, 3u);
    }
    if (n >= 2 && s[0] == '0' && (s[1] == 'b' || s[1] == 'B')) {
        if (signed_value) return NAN;
        return kiln_radix_integer(s, n, 2u, 1u);
    }
    if (!kiln_decimal_grammar(s, n)) return NAN;
    value = 0.0;
    parsed = ffc_parse_double(n, s, &value);
    if (parsed.ptr != s + n || parsed.outcome == FFC_OUTCOME_INVALID_INPUT) return NAN;
    return negative ? -value : value;
}
