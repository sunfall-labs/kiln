# ADR 0048: `node:path` is one implementation, and both engines link it

## Status

Accepted. Applies to `node:path`'s pure decompositions the reasoning ADR 0025 and ADR 0029 §4
already applied to the calendar, and the provider's dependency comments already applied to
`regress` and `num-bigint`.

## Context

Both engines answered `path.dirname` and its siblings, and each carried its own implementation. The
interpreter scanned with `std::path`; the native backend hand-wrote the same four decompositions in
C, about 160 lines of pointer arithmetic over separators, dots and suffixes.

Measured against node across a thirty-case matrix, they were both wrong, in different places:

| | disagreements with node |
| --- | --- |
| interpreter, over `std::path` | 7 |
| native backend, hand-written C | 5 |

`basename("/foo/bar.js", "bar.js")` was `""` in the interpreter and `"bar.js"` natively; node says
`"bar.js"`. `dirname("a//b")` was `"a"` in both; node says `"a/"`. Neither engine's answer was a
rounding error against the other -- a program that moved from `kiln run` to `kiln build` changed
what its paths meant.

This is the failure mode the provider crate's own dependency comments describe, for `regress` and
for `num-bigint`: *two implementations of one exactly specified thing agree until one of them has a
bug, and then nothing can tell you which.* `node:path` had reached the second half of that
sentence.

`std::path` is not the fix, and was part of the problem. It knows a *normalising* dialect of paths;
node's decompositions are lexical. `Path::parent` collapses the run in `a//b` and resolves the `.`
in `/a/./b`, and no `extension()` call models a component of nothing but dots, where `extname("..")`
is `""` but `extname("...")` is `"."`.

## Decision

### One crate, linked by both engines

`kiln-node-path` is a leaf crate with no dependencies, holding `dirname`, `basename`, `extname`,
`normalize` and `relative`. The three scans are transcribed from node's own implementation, which
is the specification here in the way ECMA-262 is elsewhere, and the unit tests are node's answers
rather than a reading of them. A change that "fixes" one of those expectations has changed what
`node:path` means.

The interpreter calls it directly. The native backend reaches it through two new provider exports,
`kiln_node_path_part` and `kiln_node_path_relative`, so the archive a generated program links
carries the same code the interpreter runs.

### The answer is borrowed, and the call is not doubled

`dirname`, `basename` and `extname` always answer with a slice of the caller's own path, so the
crate returns `Cow` and allocates nothing for them; only `normalize` and `relative` build a string.

The generated C sizes its buffer from its input -- no decomposition outgrows the path it came from,
and `relative` is bounded by three bytes per component of `from` plus `to` -- and so makes a single
call, rather than the ask-the-length-then-ask-again pair the other string-returning provider
entry points use. A 512-byte inline buffer covers every ordinary path; only an absurd one reaches
the heap.

## Consequences

Both engines answer node exactly across the matrix: 7 and 5 disagreements become 0 and 0. Twelve
live bugs are fixed by deleting code -- 118 lines from the interpreter and 160 from the C prelude,
against 96 lines of provider glue and one 260-line crate that includes its tests.

The backend's own tests link a C shim rather than the shipped archive, so they stub these two
symbols and refuse. That is deliberate: a working implementation in the test host would be a third
copy, which is the thing this ADR exists to prevent. `scripts/native.sh` and the conformance
corpora run against the real archive and are where these are exercised.

`kiln-node-path` compiles *into* the provider archive, so it belongs in the provider's source
identity -- in all three places that list it. Leaving it out is how a native run keeps linking
yesterday's `basename` while the interpreter has already moved; the hygiene test that holds the
three lists equal caught exactly that omission while this change was being written.
