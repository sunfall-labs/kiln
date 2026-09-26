#!/usr/bin/env python3
"""Generate the two 128-bit multiplier tables Ryu needs, as C.

The tables are *data*: `5^i` and its reciprocal, each normalised to 125 significant bits so
that a 64x128 multiply and one shift answers `m * 2^e2 / 10^q` to enough precision for the
shortest-round-trip proof in `kiln_c99_double_to_js` (runtime/numbers.c). Nothing here is a
transcription of anyone's table -- the two closed forms below are the whole definition, and
running this script reproduces the file byte for byte.

    python3 scripts/gen-ryu-tables.py > crates/native/kiln-backend-c/src/runtime/ryu_tables.c

`pow5bits(e)` is `bitlength(5^e)`, i.e. `floor(e * log2(5)) + 1`; the C prelude computes it
with the integer approximation `((e * 1217359) >> 19) + 1`, which this script checks is exact
over the whole range it is asked for.
"""

BITCOUNT = 125
POW5_ENTRIES = 326  # index `-e2 - q` reaches 325 at the smallest subnormal.
POW5_INV_ENTRIES = 292  # index `q` reaches 290 at the largest finite double.


def pow5bits(e: int) -> int:
    return (5**e).bit_length()


def pow5_split(i: int) -> int:
    """`5^i` normalised to exactly `BITCOUNT` significant bits, rounded down."""
    shift = BITCOUNT - pow5bits(i)
    return 5**i << shift if shift >= 0 else 5**i >> -shift


def pow5_inv_split(q: int) -> int:
    """The reciprocal partner: `floor(2^(pow5bits(q) - 1 + BITCOUNT) / 5^q) + 1`.

    The `+ 1` rounds the reciprocal *up*, which is what makes the truncating multiply below an
    upper bound rather than an unknown-signed error.
    """
    return (1 << (pow5bits(q) - 1 + BITCOUNT)) // 5**q + 1


def rows(name: str, count: int, entry) -> str:
    lines = [f"static const uint64_t {name}[{count}][2] = {{"]
    for index in range(count):
        value = entry(index)
        assert 0 <= value < 1 << 128, (name, index)
        lines.append(
            f"    {{UINT64_C({value & ((1 << 64) - 1)}), UINT64_C({value >> 64})}},"
        )
    lines.append("};")
    return "\n".join(lines)


def main() -> None:
    for e in range(0, 3529):
        assert ((e * 1217359) >> 19) + 1 == pow5bits(e), e
    for e in range(0, 1651):
        assert (e * 78913) >> 18 == len(str(2**e)) - 1, e
    for e in range(0, 2621):
        assert (e * 732923) >> 20 == len(str(5**e)) - 1, e

    print(HEADER)
    print(rows("kiln_c99_ryu_pow5_inv", POW5_INV_ENTRIES, pow5_inv_split))
    print()
    print(rows("kiln_c99_ryu_pow5", POW5_ENTRIES, pow5_split))


HEADER = """
/* Ryu's two 128-bit multiplier tables. GENERATED -- do not edit by hand.
 *
 *     python3 scripts/gen-ryu-tables.py > crates/native/kiln-backend-c/src/runtime/ryu_tables.c
 *
 * `kiln_c99_ryu_pow5[i]` is 5^i normalised to 125 significant bits; `kiln_c99_ryu_pow5_inv[q]`
 * is `floor(2^(pow5bits(q) - 1 + 125) / 5^q) + 1`, the same normalisation of its reciprocal
 * rounded away from zero. Each entry is `{low 64 bits, high 64 bits}`, which is the order
 * `kiln_c99_ryu_mul_shift` (runtime/numbers.c) multiplies them in.
 *
 * They are data and nothing else: the two closed forms above are in the generator, the
 * generator asserts the integer approximations of `pow5bits`, `log10(2^e)` and `log10(5^e)`
 * that the C uses are exact over the range it asks for, and re-running it reproduces this
 * file byte for byte. */
""".strip()


if __name__ == "__main__":
    main()
