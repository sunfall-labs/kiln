#include <float.h>
#include <limits.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

int kiln_number_to_js_string(double value, char *buffer, size_t capacity);
double kiln_string_to_number(const char *bytes, size_t length);

_Static_assert(CHAR_BIT == 8, "Kiln's number ABI requires eight-bit bytes");
_Static_assert(sizeof(int) == sizeof(int32_t), "Kiln's number ABI requires a 32-bit C int");
_Static_assert(sizeof(double) == sizeof(uint64_t) && DBL_MANT_DIG == 53,
               "Kiln's number ABI requires IEEE-754 binary64 doubles");

static int same_bits(double left, double right) {
    uint64_t left_bits;
    uint64_t right_bits;
    memcpy(&left_bits, &left, sizeof(left_bits));
    memcpy(&right_bits, &right, sizeof(right_bits));
    return left_bits == right_bits;
}

static int formats(double value, const char *expected) {
    char output[64];
    const int length = kiln_number_to_js_string(value, output, sizeof(output));
    return length == (int)strlen(expected) && strcmp(output, expected) == 0;
}

int main(void) {
    char short_output[5] = {'X', 'X', 'X', 'X', 'X'};
    const char malformed[] = {(char)0xc0, (char)0xaf};

    if (!formats(1e20, "100000000000000000000") ||
        !formats(1e21, "1e+21") ||
        !formats(157651540651918.12, "157651540651918.12") ||
        !formats(-0.0, "0") ||
        !formats(NAN, "NaN") ||
        !formats(INFINITY, "Infinity")) return 1;

    if (kiln_number_to_js_string(1.234, short_output, sizeof(short_output)) != -1 ||
        memcmp(short_output, "XXXXX", sizeof(short_output)) != 0 ||
        kiln_number_to_js_string(1.0, NULL, 0) != -1) return 2;

    if (!same_bits(kiln_string_to_number(NULL, 0), 0.0) ||
        !isnan(kiln_string_to_number(NULL, 1)) ||
        !isnan(kiln_string_to_number(malformed, sizeof(malformed))) ||
        !same_bits(kiln_string_to_number("-0", 2), -0.0) ||
        !same_bits(kiln_string_to_number("0x20000000000003",
                                         sizeof("0x20000000000003") - 1),
                   9007199254740996.0)) return 3;

    return 0;
}
