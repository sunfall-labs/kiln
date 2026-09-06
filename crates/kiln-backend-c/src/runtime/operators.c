


static int kiln_c99_string_compare(kiln_value left, kiln_value right) {
    kiln_c99_utf16_iter left_iterator = {left.ptr, left.len, 0, 0, 0};
    kiln_c99_utf16_iter right_iterator = {right.ptr, right.len, 0, 0, 0};
    uint16_t left_unit;
    uint16_t right_unit;
    int left_has;
    int right_has;
    for (;;) {
        left_has = kiln_c99_utf16_next(&left_iterator, &left_unit);
        right_has = kiln_c99_utf16_next(&right_iterator, &right_unit);
        if (!left_has || !right_has) {
            return left_has ? 1 : (right_has ? -1 : 0);
        }
        if (left_unit != right_unit) {
            return left_unit < right_unit ? -1 : 1;
        }
    }
}

static kiln_value kiln_c99_unary(uint16_t operation, kiln_value value) {
    /* A BigInt stays one under negation, bitwise-not and the two updates; `+x` is the exception,
     * and is a TypeError in the specification precisely so that `+` cannot silently narrow one.
     * That refusal lives in `kiln_c99_unary_checked`, which has somewhere to put it. */
    if (value.tag == KILN_VALUE_BIGINT) {
        void *answer = NULL;
        if (operation == 7) {
            /* `ToNumeric` of a BigInt is the BigInt: the conversion an update performs keeps the
             * type, which is the whole difference from the `ToNumber` that unary plus applies. */
            return value;
        }
        if (operation == 8 || operation == 9) {
            const void *one = kiln_c99_bigint_one();
            void *stepped = NULL;
            if (one != NULL &&
                kiln_bigint_binary(operation == 8 ? 1 : 2, (const void *)(uintptr_t)value.bits,
                                   one, &stepped) == 0) {
                return kiln_c99_bigint_value(stepped);
            }
        }
        if (operation == 2 && kiln_bigint_negate((const void *)(uintptr_t)value.bits, &answer) == 0) {
            return kiln_c99_bigint_value(answer);
        }
        if (operation == 5) {
            /* `~x` is `-(x + 1)`, which the provider's own arithmetic gets right for a width
             * it does not have to guess at. */
            const void *one = kiln_c99_bigint_one();
            void *sum = NULL;
            if (one != NULL &&
                kiln_bigint_binary(1, (const void *)(uintptr_t)value.bits, one, &sum) == 0 &&
                kiln_bigint_negate(sum, &answer) == 0) {
                return kiln_c99_bigint_value(answer);
            }
        }
    }
    /* Only the numeric operators coerce.  `typeof` reports a type, `void` discards, and `!` asks
     * whether the value is truthy -- a function always is. */
    if (kiln_c99_is_reference(value) &&
        (operation == 1 || operation == 2 || operation == 5 || operation == 7 ||
         operation == 8 || operation == 9)) {
        value = kiln_c99_text(value);
    }
    switch (operation) {
    case 1: case 7: return kiln_c99_number(kiln_c99_as_number(value));
    case 2: return kiln_c99_number(-kiln_c99_as_number(value));
    case 3: return kiln_c99_boolean(!kiln_c99_truthy(value));
    case 4: return kiln_c99_typeof(value);
    case 5: return kiln_c99_number(kiln_c99_signed32(~kiln_c99_to_uint32(kiln_c99_as_number(value))));
    case 6: return kiln_c99_undefined();
    /* `x++` is `ToNumeric(x) + 1`, not `x + 1`: the conversion happens first, which is why
     * `x = "5"; x++` leaves 6 rather than "51". */
    case 8: return kiln_c99_number(kiln_c99_as_number(value) + 1.0);
    case 9: return kiln_c99_number(kiln_c99_as_number(value) - 1.0);
    default: return kiln_c99_undefined();
    }
}

static kiln_value kiln_c99_binary(uint16_t operation, kiln_value left, kiln_value right) {
    /* Reference operands have already been reduced by `kiln_c99_binary_checked`, which is where
     * the reduction has to live: it is a *call* into the program, and this function has neither
     * a host to make one with nor anywhere to put what it throws. A reference reaching here is a
     * scalar program's, and a scalar program has no reference in it. */
    switch (operation) {
    case 1:
        if (left.tag == KILN_VALUE_STRING || right.tag == KILN_VALUE_STRING) {
            return kiln_c99_concat(left, right);
        }
        return kiln_c99_number(kiln_c99_as_number(left) + kiln_c99_as_number(right));
    case 2: return kiln_c99_number(kiln_c99_as_number(left) - kiln_c99_as_number(right));
    case 3: return kiln_c99_number(kiln_c99_as_number(left) * kiln_c99_as_number(right));
    case 4: return kiln_c99_number(kiln_c99_as_number(left) / kiln_c99_as_number(right));
    case 5: return kiln_c99_number(fmod(kiln_c99_as_number(left), kiln_c99_as_number(right)));
    case 6: return kiln_c99_number(pow(kiln_c99_as_number(left), kiln_c99_as_number(right)));
    case 7: return kiln_c99_boolean(kiln_c99_strict_equal(left, right));
    case 8: return kiln_c99_boolean(!kiln_c99_strict_equal(left, right));
    case 9:
        if (left.tag == KILN_VALUE_STRING && right.tag == KILN_VALUE_STRING) {
            return kiln_c99_boolean(kiln_c99_string_compare(left, right) < 0);
        }
        return kiln_c99_boolean(kiln_c99_as_number(left) < kiln_c99_as_number(right));
    case 10:
        if (left.tag == KILN_VALUE_STRING && right.tag == KILN_VALUE_STRING) {
            return kiln_c99_boolean(kiln_c99_string_compare(left, right) <= 0);
        }
        return kiln_c99_boolean(kiln_c99_as_number(left) <= kiln_c99_as_number(right));
    case 11:
        if (left.tag == KILN_VALUE_STRING && right.tag == KILN_VALUE_STRING) {
            return kiln_c99_boolean(kiln_c99_string_compare(left, right) > 0);
        }
        return kiln_c99_boolean(kiln_c99_as_number(left) > kiln_c99_as_number(right));
    case 12:
        if (left.tag == KILN_VALUE_STRING && right.tag == KILN_VALUE_STRING) {
            return kiln_c99_boolean(kiln_c99_string_compare(left, right) >= 0);
        }
        return kiln_c99_boolean(kiln_c99_as_number(left) >= kiln_c99_as_number(right));
    case 15: return kiln_c99_boolean(kiln_c99_loose_equal(left, right));
    case 16: return kiln_c99_boolean(!kiln_c99_loose_equal(left, right));
    case 17: return kiln_c99_number(kiln_c99_signed32(
        kiln_c99_to_uint32(kiln_c99_as_number(left)) &
        kiln_c99_to_uint32(kiln_c99_as_number(right))));
    case 18: return kiln_c99_number(kiln_c99_signed32(
        kiln_c99_to_uint32(kiln_c99_as_number(left)) |
        kiln_c99_to_uint32(kiln_c99_as_number(right))));
    case 19: return kiln_c99_number(kiln_c99_signed32(
        kiln_c99_to_uint32(kiln_c99_as_number(left)) ^
        kiln_c99_to_uint32(kiln_c99_as_number(right))));
    case 20: return kiln_c99_number(kiln_c99_signed32(
        kiln_c99_to_uint32(kiln_c99_as_number(left)) <<
        (kiln_c99_to_uint32(kiln_c99_as_number(right)) & UINT32_C(31))));
    case 21: return kiln_c99_number(kiln_c99_signed32(kiln_c99_shift_right(
        kiln_c99_to_uint32(kiln_c99_as_number(left)),
        kiln_c99_to_uint32(kiln_c99_as_number(right)))));
    case 22: return kiln_c99_number((double)(
        kiln_c99_to_uint32(kiln_c99_as_number(left)) >>
        (kiln_c99_to_uint32(kiln_c99_as_number(right)) & UINT32_C(31))));
    default: return kiln_c99_undefined();
    }
}


/* The three hints `ToPrimitive` can be asked for. The specification has three, not two, and the
 * third is not decoration: a `Date` is the one built-in whose `@@toPrimitive` answers a string for
 * "default" and a number for "number", which is the whole of why `date + date` concatenates while
 * `date - date` subtracts. For every other value the three coincide, which is why two of them
 * sufficed here for so long. NUMBER and STRING keep the 0 and 1 the two-valued flag used, so every
 * call site that already said what it wanted still says it.
 *
 * Defined here as well as in `prelude_declarations.h` because a single-unit prelude does not
 * include that header, and guarded so that the separately compiled build sees one definition. */
/* Whether reducing this value to a primitive is a *call* rather than a no-op.
 *
 * Deliberately not `kiln_c99_is_reference`, which is a primitive denylist that excludes a date on
 * purpose; using that one as "is an object" is a mistake this file has already paid for once. A
 * date is not a reference and still has to be reduced by calling, so the two predicates are
 * genuinely different questions and each has its own name. */
static int kiln_c99_reduces_to_primitive(kiln_value value) {
    return kiln_c99_is_reference(value) || value.tag == KILN_VALUE_DATE;
}

#ifndef KILN_C99_HINT_NUMBER
#define KILN_C99_HINT_NUMBER 0
#define KILN_C99_HINT_STRING 1
#define KILN_C99_HINT_DEFAULT 2
#endif

/* Reduce a value to a primitive, which is a *call* rather than a formatting rule.
 *
 * Which method is tried first depends on what the operator wanted: arithmetic wants a number and
 * asks `valueOf` first, string conversion asks `toString` first. Every date and decimal library
 * is used through the first of those, and `${x}` differs from `"" + x` only in this.
 *
 * A method that hands back another object is skipped rather than accepted: the point is to reach
 * a primitive, and a `valueOf` returning `this` -- which is what `Object.prototype.valueOf` does
 * -- would otherwise stop the search at its own receiver.
 *
 * A value with no method to call falls back to `kiln_c99_text`, which is where an array's join
 * and a function's source text already live. */
static int kiln_c99_to_primitive(kiln_host *host, kiln_value value, int hint,
                                 kiln_value *out, int depth, kiln_value *thrown) {
    static const char *const string_first[2] = {"toString", "valueOf"};
    static const char *const value_first[2] = {"valueOf", "toString"};
    /* `Date.prototype[@@toPrimitive]` is a real method in the interpreter, but this backend
     * models a date as a value kind rather than an object carrying that method, so the rule it
     * would apply is applied here instead: a date asked for "default" tries `toString` first,
     * every other value tries `valueOf`. That single line is the difference between
     * `date + date` concatenating and adding. */
    int prefers_string = hint == KILN_C99_HINT_STRING ||
                         (hint == KILN_C99_HINT_DEFAULT && value.tag == KILN_VALUE_DATE);
    const char *const *order = prefers_string ? string_first : value_first;
    int step;
    /* `${p}` and `p + 1` read `@@toPrimitive`, `valueOf` and `toString` off the proxy, which are
     * three trap calls whose results this backend has nowhere to put yet. */
    if (value.tag == KILN_VALUE_PROXY) {
        KILN_C99_REFUSE("converting a Proxy to a primitive");
    }
    /* A date is not a reference here -- `kiln_c99_is_reference` is a primitive denylist and
     * excludes it on purpose -- but it still reduces by *calling*, and it is the one value whose
     * answer depends on which hint was asked for. Letting it through as its own primitive is what
     * made `date + date` add two instants. */
    if (!kiln_c99_is_reference(value) && value.tag != KILN_VALUE_DATE) {
        *out = value;
        return KILN_C99_OK;
    }
    {
        /* `@@toPrimitive` overrides both, and is told which hint it is answering. */
        kiln_value exotic;
        int status = kiln_c99_get(host, value, "@@toPrimitive", 13, &exotic, depth, thrown);
        if (status == KILN_C99_THROWN) {
            return status;
        }
        if (status == KILN_C99_OK && kiln_c99_is_callable(exotic)) {
            const char *spelling = hint == KILN_C99_HINT_STRING
                                       ? "string"
                                       : (hint == KILN_C99_HINT_DEFAULT ? "default" : "number");
            kiln_value hint_text = kiln_c99_copy_text(spelling, strlen(spelling));
            kiln_value produced;
            status = kiln_c99_call(host, &produced, exotic, value, &hint_text, 1, depth,
                                   thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (kiln_c99_is_reference(produced)) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot convert object to primitive value");
            }
            *out = produced;
            return KILN_C99_OK;
        }
    }
    for (step = 0; step < 2; ++step) {
        kiln_value method;
        kiln_value produced;
        int status = kiln_c99_get(host, value, order[step], strlen(order[step]), &method, depth,
                                  thrown);
        if (status == KILN_C99_THROWN) {
            return status;
        }
        if (status != KILN_C99_OK || !kiln_c99_is_callable(method)) {
            continue;
        }
        status = kiln_c99_call(host, &produced, method, value, NULL, 0, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (!kiln_c99_is_reference(produced)) {
            *out = produced;
            return KILN_C99_OK;
        }
    }
    /* Some values' text form is not held in a property store, so the walk above cannot reach it:
     * this backend models `Function.prototype.toString` -- and, for a namespace, a promise, a
     * generator and a collection, the tagged `Object.prototype.toString` -- as `kiln_c99_text`
     * rather than as a method a lookup finds. Failing here would report a missing conversion for
     * values that plainly have one, which is how `String(Math)` came to throw where the language
     * says "[object Math]", and how `String(new Map())` still did.
     *
     * Still scoped, and to the same principle: these are the values whose conversion this file
     * defines *for the tag*, not the blanket fallback this used to be. A plain object and an
     * array are deliberately outside it -- their text depends on what they hold, the walk above
     * is the only thing that can ask, and answering "[object Object]" here instead is what made
     * `Object.create(null) + 1` produce a string where the language stops the program. */
    if (kiln_c99_has_intrinsic_text(value)) {
        kiln_value shown = kiln_c99_text(value);
        if (shown.tag == KILN_VALUE_STRING) {
            *out = shown;
            return KILN_C99_OK;
        }
    }
    /* Neither was reachable, or neither answered with a primitive. The specification stops the
     * program here, and this used to answer `kiln_c99_text(value)` instead -- which is not merely
     * a wrong value but a wrong *program*: everything after the fault ran, and the process exited
     * 0 where the language says 1. `Object.create(null) + 1` and an object whose `valueOf` and
     * `toString` both return objects are the two ways in. */
    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                "Cannot convert object to primitive value");
}

/* ToNumber, for a caller that has a host and somewhere to throw.
 *
 * `kiln_c99_as_number` is pure, so on a reference it can only answer NaN -- it cannot run the
 * `valueOf` that ToNumber is specified to call. Every builtin that wants a number from a value the
 * program supplied goes through here instead, which is the same split `kiln_c99_unary_checked`
 * makes below for the operators. */
static int kiln_c99_to_number_checked(kiln_host *host, kiln_value value, double *out, int depth,
                                      kiln_value *thrown) {
    kiln_value reduced = value;
    int status;
    if (kiln_c99_is_reference(value)) {
        status = kiln_c99_to_primitive(host, value, 0, &reduced, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    if (reduced.tag == KILN_VALUE_SYMBOL) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot convert a Symbol value to a number");
    }
    if (reduced.tag == KILN_VALUE_BIGINT) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot convert a BigInt value to a number");
    }
    *out = kiln_c99_as_number(reduced);
    return KILN_C99_OK;
}

/* `+x`, `-x` and `~x` on something that is not a primitive yet.
 *
 * The same reduction the binary operators do, for the same reason: all three want a number, and
 * a number is what `valueOf` is for. `!`, `typeof` and `void` do not coerce -- they answer about
 * the value they were given. */

static int kiln_c99_unary_checked(uint16_t operation, kiln_host *host, kiln_value value,
                                  kiln_value *out, int depth, kiln_value *thrown) {
    int numeric = operation == 1 || operation == 2 || operation == 5 || operation == 7 ||
                  operation == 8 || operation == 9;
    if (numeric && kiln_c99_is_reference(value)) {
        int status = kiln_c99_to_primitive(host, value, 0, &value, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    /* A symbol has no number of its own, and a BigInt refuses the one narrowing conversion --
     * unary plus -- that would take it out of its own type. Both were answering with a value
     * instead: `NaN` for the symbol and the narrowed number for the BigInt. */
    if (numeric && value.tag == KILN_VALUE_SYMBOL) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot convert a Symbol value to a number");
    }
    if (operation == 1 && value.tag == KILN_VALUE_BIGINT) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot convert a BigInt value to a number");
    }
    *out = kiln_c99_unary(operation, value);
    return KILN_C99_OK;
}

/* The operators that can throw.
 *
 * Only BigInt makes an operator throw before anything is called, so this wraps
 * `kiln_c99_binary` rather than replacing it: everything without a BigInt operand takes the
 * same path it always did, and only the three throwing cases are spelled out here.
 *
 * The provider owns the arithmetic (ADR 0025) -- writing arbitrary-precision division twice is
 * exactly the duplication that pattern exists to prevent -- so this is the type dispatch and the
 * error surface around it, nothing more. */

static int kiln_c99_binary_checked(uint16_t operation, kiln_host *host, kiln_value left,
                                   kiln_value right, kiln_value *out, int depth,
                                   kiln_value *thrown) {
    int left_big;
    int right_big;
    int numeric;
    if (kiln_c99_reduces_to_primitive(left) || kiln_c99_reduces_to_primitive(right)) {
        /* Strict equality is identity and never coerces, `in` and `instanceof` take their
         * operands as they are, and a nullish `==` is answered without any conversion at all --
         * `value != null` is the commonest guard in JavaScript, and running a wrapper's `valueOf`
         * there is both wrong and, for a wrapper that assumes it is being used properly, fatal. */
        int coerces = !(operation == 7 || operation == 8 || operation == 13 || operation == 14);
        if (operation == 15 || operation == 16) {
            /* `==` reduces only across a type boundary. Two objects compare as themselves --
             * `{} == {}` is false -- and reducing both to text would make every pair of empty
             * objects equal. A nullish side is answered without any conversion at all: `value
             * != null` is the commonest guard in JavaScript, and running a wrapper's `valueOf`
             * there is both wrong and, for a wrapper that assumes it is being used properly,
             * fatal. */
            if ((kiln_c99_reduces_to_primitive(left) && kiln_c99_reduces_to_primitive(right)) ||
                left.tag == KILN_VALUE_UNDEFINED || left.tag == KILN_VALUE_NULL ||
                right.tag == KILN_VALUE_UNDEFINED || right.tag == KILN_VALUE_NULL) {
                coerces = 0;
            }
        }
        if (coerces) {
            /* Every operator here asks `valueOf` first; only `${}` and `String(x)` use the
             * other order. But `+` and loose equality state no *preference* -- each chooses
             * between a string reading and a numeric one only once both sides have reduced --
             * while the rest have already committed to a number. That distinction is invisible
             * for every value but a date, and it is why `date + date` concatenates. */
            int hint = (operation == 1 || operation == 15 || operation == 16)
                           ? KILN_C99_HINT_DEFAULT
                           : KILN_C99_HINT_NUMBER;
            int status = kiln_c99_to_primitive(host, left, hint, &left, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            /* `ToNumeric(lhs)` completes before the right operand is reduced at all, for the
             * operators with no string reading to choose first: `Symbol() * f()` refuses here
             * rather than after calling `f`'s `valueOf`, which is one observable call the
             * language says does not happen. `+` and the four relational operators are absent
             * deliberately -- each decides between a string and a numeric reading, and cannot
             * know which conversion it is performing until both sides are reduced. */
            if (left.tag == KILN_VALUE_SYMBOL &&
                ((operation >= 2 && operation <= 6) || (operation >= 17 && operation <= 22))) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot convert a Symbol value to a number");
            }
            status = kiln_c99_to_primitive(host, right, hint, &right, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    /* A symbol has no number and no string, and the operators that need one say so rather than
     * inventing a value: reading `Symbol() - 1` as `NaN` is the kind of plausible answer that
     * hides the mistake it came from. `+` is told apart because it decides between the two
     * conversions, and both refuse a symbol with a different message; `==` converts nothing. */
    numeric = (operation >= 2 && operation <= 6) || (operation >= 9 && operation <= 12) ||
              (operation >= 17 && operation <= 22);
    if (left.tag == KILN_VALUE_SYMBOL || right.tag == KILN_VALUE_SYMBOL) {
        if (numeric) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Cannot convert a Symbol value to a number");
        }
        if (operation == 1) {
            return kiln_c99_throw_error(
                thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                (left.tag == KILN_VALUE_STRING || right.tag == KILN_VALUE_STRING)
                    ? "Cannot convert a Symbol value to a string"
                    : "Cannot convert a Symbol value to a number");
        }
    }
    left_big = left.tag == KILN_VALUE_BIGINT;
    right_big = right.tag == KILN_VALUE_BIGINT;
    const void *a;
    const void *b;
    uint32_t provider_op = 0;
    void *answer = NULL;
    int32_t status;
    if (!left_big && !right_big) {
        *out = kiln_c99_binary(operation, left, right);
        return KILN_C99_OK;
    }
    if (!left_big || !right_big) {
        /* `"n = " + 1n` is a *string* operation: `+` concatenates when either side is a string,
         * and a BigInt has a string spelling like any other value. Only the numeric addition
         * mixes the two. */
        int concatenating = operation == 1 &&
                            (left.tag == KILN_VALUE_STRING || right.tag == KILN_VALUE_STRING);
        /* Comparison across the two *is* allowed -- only arithmetic is not -- and so is `==`.
         * Both are answered by the ordinary numeric path, which reads a BigInt as a double.
         * A shift and a bitwise operator are arithmetic here too: `1n << 1` had fallen through
         * to the 32-bit path and answered the *number* 2, which is a BigInt silently narrowed. */
        if (!concatenating && ((operation >= 1 && operation <= 6) ||
                               (operation >= 17 && operation <= 22))) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Cannot mix BigInt and other types, use explicit conversions");
        }
        *out = kiln_c99_binary(operation, left, right);
        return KILN_C99_OK;
    }
    a = (const void *)(uintptr_t)left.bits;
    b = (const void *)(uintptr_t)right.bits;
    switch (operation) {
    case 7: case 15: *out = kiln_c99_boolean(kiln_bigint_compare(a, b) == 0); return KILN_C99_OK;
    case 8: case 16: *out = kiln_c99_boolean(kiln_bigint_compare(a, b) != 0); return KILN_C99_OK;
    case 9:  *out = kiln_c99_boolean(kiln_bigint_compare(a, b) < 0); return KILN_C99_OK;
    case 10: *out = kiln_c99_boolean(kiln_bigint_compare(a, b) <= 0); return KILN_C99_OK;
    case 11: *out = kiln_c99_boolean(kiln_bigint_compare(a, b) > 0); return KILN_C99_OK;
    case 12: *out = kiln_c99_boolean(kiln_bigint_compare(a, b) >= 0); return KILN_C99_OK;
    /* The provider numbers its arithmetic 1..6 the same way; only the bitwise three differ. */
    case 1: case 2: case 3: case 4: case 5: case 6: provider_op = operation; break;
    case 17: provider_op = 7; break;
    case 18: provider_op = 8; break;
    case 19: provider_op = 9; break;
    case 20: provider_op = 10; break;
    case 21: provider_op = 11; break;
    /* There is no unsigned shift on a BigInt, and it is refused rather than approximated: `>>>`
     * fills from a *fixed* width and a BigInt has none, so there is no answer to give. */
    case 22:
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "BigInts have no unsigned right shift, use >> instead");
    default:
        /* `in` and `instanceof` are not BigInt operators here; the numeric path answers them the
         * way it answers any other value. */
        *out = kiln_c99_binary(operation, left, right);
        return KILN_C99_OK;
    }
    status = kiln_bigint_binary(provider_op, a, b, &answer);
    if (status != 0) {
        /* Division truncates toward zero and by zero is a RangeError, neither of which matches
         * what a number would do. A negative exponent is the other one, and a shift that would
         * widen past what any allocation could hold is the third. */
        if (provider_op == 6) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "Exponent must be non-negative");
        }
        if (provider_op == 10 || provider_op == 11) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "Maximum BigInt size exceeded");
        }
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR, "Division by zero");
    }
    *out = kiln_c99_bigint_value(answer);
    return KILN_C99_OK;
}
