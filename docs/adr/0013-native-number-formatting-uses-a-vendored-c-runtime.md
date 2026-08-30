# ADR 0013: Native number formatting uses a vendored C runtime

## Status

Accepted.

## Context

JavaScript requires the shortest decimal that round-trips to the same IEEE-754 binary64 value,
then chooses fixed or exponential notation at language-specific boundaries. The original native
backend used C `snprintf("%.17g")`. Seventeen digits round-trip, but they are often not shortest:
`0.1` became `0.10000000000000001`, `1e-6` became a padded approximation, and `5e-324` expanded to
seventeen digits. Libc also chooses different spellings for special values and can inherit the
embedding process's numeric locale.

Rust already has good implementations. `ryu-js` is explicitly ECMAScript-oriented, and the Oxc
dependency graph already contains `dragonbox_ecma`. Neither solves the target-runtime problem:
formatting happens inside the emitted executable or shared library after the Rust compiler process
has exited. A normal Cargo dependency does not place a callable formatter inside that artifact.
Zig's standard library has the same packaging problem and would also turn the preferred Zig driver
into a mandatory build dependency, dropping the Clang/GCC fallback. Google's `double-conversion`
has a ready-made ECMAScript converter but would add C++ compilation, runtime-link, and cache policy
to a deliberately C11 backend.

## Decision

Kiln vendors the binary64 shortest-conversion portion of Ulf Adams' portable C Ryu implementation,
pinned to commit `4c0618b0e44f7ef027ebae05d2cc7812048f7c8f` under the Boost Software License
1.0. `kiln-native-number` owns that source, its attribution, and `NUMBER_FORMAT_VERSION`. A small
Kiln adapter turns Ryu's shortest scientific digits into ECMAScript presentation: fixed notation
for decimal exponents in `[-6, 21)`, lowercase `e` otherwise, an explicit `+` for positive
exponents, canonical `NaN`/infinities, and `-0` converted to `0` for string coercion.

Generated modules call one Kiln-prefixed internal function with a caller-owned fixed buffer. The
command-line runtime composes the same deterministic source into its reusable runtime translation
unit and keeps the separate console rule that displays negative zero as `-0`. Scalar and graph
shared-library builds cache and compile one position-independent number object with hidden
visibility, reuse that object across programs and graph shapes, and include its digest in the final
link key. No formatter symbol becomes part of the public embedding header or HostOps ABI.

The implementation remains C11 and requires no Zig or Rust compiler at target build time. Refreshes
retain upstream notices, update the pinned commit, run the strict C tests, and intentionally bump
`NUMBER_FORMAT_VERSION`. Runtime identities assert the formatter version they compose, preventing
a source refresh from silently masquerading as the old internal contract.

This decision covers formatting only. ADR 0014 subsequently replaces the interim libc
string-to-number path with a bounded fast_float C port plus an ECMAScript grammar adapter.

## Consequences

- Native console output and number-to-string coercion match the interpreter on shortest decimals,
  threshold boundaries, subnormals, infinities, and negative zero.
- Formatter behavior is independent of process and thread locale. ADR 0014 later makes parsing
  locale-free as well.
- One roughly 74 KB reviewed source component replaces a small libc call, but it compiles as a durable
  cache layer rather than being duplicated into every graph module object.
- Shared artifacts stay self-contained and keep their existing public ABI while gaining one hidden
  internal object and link-key input.
- Rust formatters remain useful differential oracles, not undeclared runtime dependencies.
