# ADR 0014: String numeric literals use a bounded runtime parser

## Status

Accepted.

## Context

JavaScript string-to-number coercion is a language grammar, not a call to the host language's
default float parser. `StringNumericLiteral` trims a specific Unicode set, treats an empty result
as positive zero, accepts exact signed `Infinity`, accepts unsigned binary/octal/hexadecimal
integers of arbitrary length, preserves negative zero, and rejects partial parses. Rust's
`str::parse` and libc `strtod` both disagree: they accept spellings such as `inf` or C hex floats,
trim different whitespace, and do not implement all three ECMAScript radix prefixes. A NUL-
terminated libc adapter also loses the length boundary supplied by an embedding host.

Formatting and parsing happen inside emitted executables and shared libraries after Kiln's Rust
process exits. A Rust-only crate therefore cannot be the target runtime. Requiring Zig or a C++
parser would also remove the supported C11 Clang/GCC path. Locale-explicit `strtod_l` fixed ambient
locale dependence on macOS and Linux, but retained platform branches, allocation, and the wrong
grammar; it was an interim boundary rather than the desired semantics.

## Decision

`kiln-native-number` owns the complete hidden number-conversion runtime. Decimal conversion uses a
pinned, unmodified snapshot of `ffc.h`, the C99 port of fast_float, selected under its Boost 1.0
license. It consumes a bounded byte span without allocation or locale state and produces the
nearest IEEE-754 binary64 value with ties to even. The exact upstream commit and checksum live in
`UPSTREAM.md`.

A small Kiln C adapter owns everything that is ECMAScript-specific. It validates UTF-8, trims only
the specification's WhiteSpace and LineTerminator code points, validates decimal syntax before
calling ffc, recognizes only exact `Infinity`, and rejects embedded NULs and partial parses.
Unsigned `0b`, `0o`, and `0x` integers use a fixed-state converter: it scans arbitrary-length input,
retains 53 significand bits plus guard and sticky bits, and rounds directly to binary64. It neither
allocates a bigint nor truncates the grammar to a machine integer.

The Rust interpreter exposes the same language operation from `kiln-host-abi`, with its own
grammar/radix implementation and bit-exact shared vectors. Generated C calls one internal
length-delimited parser symbol instead of embedding libc policy. Formatter and parser versions are
independent constants but compose into one cached native-number object. Changing either version
changes that object's ABI identity and source/link keys; the native runtime version also advances.
Neither conversion symbol is added to the public embedding or HostOps ABI.

## Consequences

- Interpreter and native coercion accept the same whitespace, decimal, infinity, and radix forms,
  including arbitrary-length literals and negative zero.
- Embedded modules no longer allocate or consult the process locale to coerce a string to number.
- Invalid UTF-8 or malformed host `(ptr, len)` values fail deterministically as `NaN`; `(NULL, 0)`
  remains the valid empty-string representation.
- The native number source grows by the decimal parser, but remains one reusable hidden cached
  object for scalar and graph artifacts rather than being duplicated into each module.
- `ffc.h` is a reviewed supply-chain input. Refreshes require an explicit pin/checksum update,
  retained notices, strict C11 compilation, semantic differential tests, and a parser version bump.
