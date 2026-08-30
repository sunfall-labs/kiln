# ADR 0025: Language semantics that need an engine are linked, not reimplemented

## Status

Accepted, unimplemented. The decision is recorded before the work because the work touches
packaging, and packaging is where two people editing the same repository collide.

## Context

The native backend refuses `NewRegExp`. Five differential probes stop there, and it is the largest
single entry left in the refusal tally.

A regular expression is not a small function. The interpreter uses `regress`, which targets
ECMAScript semantics specifically rather than the linear-time subset: backreferences, lookahead
and lookbehind, sticky matching and named groups are all things a JavaScript program expects and a
linear-time engine cannot offer. It also matches over UTF-16, so the indices it reports are
already in the units the language counts in (ADR 0022) and need no conversion.

There are three places that behaviour could come from, and the choice is not obvious.

**Reimplement it in C, inside the emitted runtime.** This is what every other builtin does, and
for `Math.hypot` or `Array.prototype.reduce` it is right: the behaviour is a page of code and two
implementations of a page can be checked against each other. A regex engine is not a page of code.
Two independent engines would agree on `/a+/` and disagree somewhere in the backreference and
lookbehind corners, and the disagreement would surface as a *wrong answer* in a program the corpus
does not cover -- which is the one failure mode the whole three-corpus arrangement exists to
prevent. `conformance/native` could not catch it either: it compares the two engines, and this
would be the two engines being genuinely different.

**Ask the host, through the HostOps ABI.** Cheap: the native runtime is already Rust (ADR 0018),
so `regress` is already linked into the launcher, and a host operation would reach the very same
engine the interpreter uses -- identical by construction rather than by testing.

It is still wrong. HostOps are *services of the environment*: write to stdout, read the clock,
open a file. They are what an embedder implements, and the contract is deliberately tiny and
deliberately scalar (`kiln-host-abi` is one operation and five value kinds). A regular expression
is not a service of the environment; it is part of what the language means. Putting it behind
HostOps would say that an embedder writing their own host has to bring a regex engine with them,
which is a much larger promise than "give me somewhere to print".

**Link a Rust provider.** There is already a precedent for exactly this, and it was adopted for
exactly this reason: `kiln-native-number-ffi` is a `no_std` staticlib with a C ABI, built and
packaged by `crates/kiln-native/src/number_provider.rs` and linked into every artifact. It exists
because ECMAScript's `Number::toString` cannot be expressed by any printf conversion, so the
behaviour had to come from one implementation rather than two.

## Decision

Regular expressions come from a linked Rust provider, built and packaged the way the number
provider is, and the engine is `regress` -- the same crate the interpreter uses.

The FFI surface is an opaque handle and index-based readback, so nothing that is not a scalar
crosses the boundary:

```c
/* 0 on success; -1 for a pattern the engine rejects, which the caller turns into a SyntaxError. */
int32_t kiln_regexp_compile(const uint8_t *pattern, size_t pattern_len,
                            const uint8_t *flags, size_t flags_len, void **out_handle);
size_t  kiln_regexp_group_count(void *handle);
/* 1 for a match, 0 for none, -1 for an error. Writes 2*group_count int64 offsets in UTF-16 code
 * units -- start and end per group, (-1, -1) for a group that did not participate. */
int32_t kiln_regexp_search(void *handle, const uint16_t *subject, size_t subject_len,
                           size_t start, int64_t *out_ranges, size_t out_capacity);
/* ABI v2: the `index`th unique named group from the last match. Returns its UTF-8 byte length,
 * 0 when there is no such group, and -1 when the buffer is too small. Writes (-1,-1) for an
 * unmatched group; callers retry with a larger buffer after -1. */
int32_t kiln_regexp_named_group(void *handle, size_t index, uint8_t *buffer, size_t capacity,
                                int64_t *out_range);
void    kiln_regexp_free(void *handle);
```

The emitted runtime slices the subject itself from the offsets, so no string comes back except a
group name, and `lastIndex`, stickiness and the match-result array stay in C where the rest of the
object model is.

## Consequences

**The provider crate stops being about numbers.** It is `kiln-native-number-ffi` today, and the
alternative -- a second archive -- means threading it through the five places
`crates/kiln-native/src/lib.rs` materialises and links `libkiln_native_number_provider.a`. One
archive with a wider scope is much less code than two archives, so the crate should be renamed
(`kiln-native-provider-ffi`) along with `PROVIDER_NAME`, the archive filename, the manifest
`provider` field and `KILN_NATIVE_NUMBER_PROVIDER_MANIFEST`. That is a packaging contract between
scripts and code that both live in this repository, and nothing is released yet, so renaming is
cheap now and expensive later.

**`regress` needs an allocator.** It has an `alloc` feature, so the crate stays `no_std` and gains
a `#[global_allocator]` over `malloc`/`free`. The crate can no longer describe itself as
allocation-free; the number core it wraps still is, and that distinction is worth keeping in the
doc comment rather than losing.

**The vendored C in `kiln-native-number` is a test fixture, not a fallback.** It defines
`kiln_number_to_js_string` and `kiln_string_to_number` and is linked only by unit tests in
`kiln-backend-c` and `kiln-native-runtime`. If the emitted runtime references the regex symbols
unconditionally, those tests stop linking. Two ways out, and the second is better:

1. Emit the regex block only for programs that need it. The obvious predicate -- "does this
   program contain `NewRegExp`?" -- is **wrong**, and wrong in a way this repository has already
   been bitten by once (`produces_references`, see its doc comment). A regex value made in module
   A can be passed to module B and used there: `b.replace(re, x)` needs the engine in B, which
   contains no pattern of its own.
2. Give those tests the real archive. They are testing the emitted C, not the provider, and
   linking the thing production links is what makes them mean anything.

**Every regex-adjacent string method comes with it.** `NewRegExp` alone is not useful:
`String.prototype.match`, `matchAll`, `replace`, `replaceAll`, `split` and `search` all take a
pattern, and the interpreter already routes them through one `RegExpMatch` shape. The backend
should do the same rather than growing six independent paths.

## Alternatives reconsidered

If a future host genuinely needs to *replace* the regex engine -- a sandbox with a hard linear-time
requirement, say -- that is a provider swap, not a HostOps addition: the manifest already names the
archive, and pointing it at a different one is the supported way to do it. That is another point in
the provider's favour rather than the host's.
