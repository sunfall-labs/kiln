# ADR 0043: The builtin surface is one table and two emitters

## Status

Accepted.

`kiln-builtin-surface` holds the single declaration of which members each built-in has. The
interpreter reads its own lists out of it. `kiln-backend-c` generates the emitted C from a table of
its own whose completeness against the shared one is a `const` assertion, so drift fails a build
rather than a customer's bundle. Fifteen built-ins are on the seam; the prototype method tables,
`Temporal`'s eight types and the typed-array constructors are not, and are named in the backlog
rather than left unchecked.

## Context

Kiln has two engines. `kiln-interpreter` runs bytecode; `kiln-backend-c` emits C that an external
toolchain compiles. They consume the same bytecode and are supposed to answer the same language.

The *surface* — which members each built-in object has — was written down twice. Once in the
interpreter's Rust, as literal `&[&str]` lists in `intrinsic_tabulated_property_names`. Once in the
emitted C prelude, as hand-written `kiln_c99_same_name(key, key_len, "…")` chains inside
`kiln_c99_namespace_method` and the `KILN_VALUE_BUILTIN` arm of the member read. Nothing compared
them, and nothing could: they were a Rust list and a C control-flow graph.

They drifted, and the drift had a characteristic and expensive shape.

**A gap was found by whichever npm bundle happened to read it.** The native backend refuses rather
than answering wrongly, which is the right instinct — but it refuses at the moment a program reads
the member, so a missing static is invisible until real code touches it, and then it takes out the
whole module. `Number` had fifteen statics in the interpreter and five natively; `Number.MAX_VALUE`
is the first property a bundled `zod` reads. `Symbol` had thirteen well-known symbols and five;
`Symbol.isConcatSpreadable` refused every bundled `lodash`. Each was fixed the day it was found,
which is the point: the supply was unbounded.

**A refusal hides the rest of the list behind it.** A sweep that runs a probe per built-in reports
the *first* gap and nothing after. `Object` answered ten members and refused on
`getOwnPropertySymbols`, so `getPrototypeOf`, `groupBy`, `hasOwn`, `is` and every name
alphabetically later were never reached at all. That measurement badly understates the real count,
and understating it is how "we fixed the `Number` one" came to sound like progress.

**The other half of the drift never refused.** `Array[Symbol.species]` read `undefined` natively
and a function in the interpreter. `Math[Symbol.toStringTag]` read `undefined` and a string. No
refusal, no diagnostic — the program takes the other branch, because `typeof X === "undefined"` is
how real code feature-detects. That is a silent wrong answer, which is the one thing this backend
is not allowed to give (ADR 0042 states the rule; this is the same rule one layer down).

**And the native side could not describe itself.** `Object.getOwnPropertyNames` refused when the
receiver was a built-in constructor, so the only way to compare the two engines was to ask about
one member at a time and see whether the answer was a refusal.

There was already most of a mechanism in the backend. `METHOD_IDS` is a Rust table of
`(name, code, group)` that generates the `#define`s, the code list and the group-membership switch,
and `kiln_c99_lookup_method_group` resolves a name out of it. Several receivers — array lookup,
string dispatch, the Node adapters — were already table-driven. What was not table-driven was
exactly the part that kept breaking: the built-in constructors' statics.

## Decision

**1. The surface is declared once, in a crate beneath both engines.**

`kiln-builtin-surface` is a leaf crate with no dependencies holding one table: for each built-in
global, the members it owns. A `@@`-prefixed name is symbol-keyed, the spelling both engines
already use for a reserved store key, so `Array[Symbol.species]` is comparable like any other
member. `length`, `name` and `prototype` are not listed — they belong to every callable, and each
engine supplies them from its own knowledge of what a function is.

Only the *surface* is shared. What a member does stays each engine's own, and always will: the
interpreter has an `ObjectData` and a realm, the backend has a `kiln_value` and a switch. Sharing
the answer would be a rewrite of both. Sharing the question is the whole of the fix, because only
the question was ever written down twice.

The interpreter's arms bind a `const` from `kiln_builtin_surface::members`, so the lookup runs at
compile time and a global it names that the table does not declare fails to build.

**2. The emitted C is generated from a table, not written.**

`BUILTIN_SURFACE` in `kiln-backend-c` is the only place the native surface is written down. A row
carries what the member *is* — a method code, a number, a text, a well-known symbol, or a built-in
handed back by name — and the emitted `kiln_c99_builtin_members[]`, the read path, the call path
and `Object.getOwnPropertyNames` are all generated from it.

That is what makes a member which is *implemented but unreachable* inexpressible. Reachability is
the row. The three hand-written constant lists that stood in the read arm — `Math`'s, `Number`'s,
and the well-known symbols — are one table lookup, and adding a member is adding a row.

**3. Completeness is a compile error, not a test failure.**

A member of the shared surface must appear either in `BUILTIN_SURFACE` or in
`BUILTIN_SURFACE_BACKLOG`, which names it and says why it is still open. That is
`assert_surface_is_covered`, a `const fn` evaluated at compile time, and it checks both directions:
the backend cannot lack a shared member silently, and cannot invent one either.

A `const` panic carries a literal and nothing else, so the build says only *that* the two disagree.
`builtin_surface_drift` in `kiln-native` reads the same three tables and says which members, with
each backlog row's reason, and it compiles no C and runs no JavaScript to do it. **The compiler is
the gate; the test is the explanation.** A test alone would have been enough to catch drift and not
enough to prevent it — somebody has to run it, and the failure would arrive after the change rather
than during it.

**4. A built-in is converted, or it is backlog. It is never merely absent.**

The backlog is a list of rows with reasons rather than a count. A gate that says "fifty-one gaps"
teaches people to ignore it; one that names them is a work queue. The rows also carry the
distinction the sweep could not: `Atomics` and `Iterator` refuse *by name* because shared memory
and the iterator helpers have no representation here, which is a decision; `Object.seal` is absent
because sealing is weaker than freezing and answering `freeze` would be a silent wrong answer,
which is a different decision; `Math.f16round` is simply not implemented. A gate that cannot tell a
deliberate refusal from an oversight is one people learn to ignore.

## What is *not* on the seam, and why

**The prototype method tables.** `Array.prototype`, `String.prototype`, `Object.prototype` and the
rest resolve a name by scanning a contiguous *code range* through `kiln_c99_method_name`. That is a
different mechanism with a different failure mode, it is already single-sourced through
`METHOD_IDS`, and it is not where the drift was.

**`Temporal`.** Its codes are dispatched by contiguous range and must stay contiguous, and the
eight types' `from`/`compare` are derived by adding the kind to a base code. A row per member would
have to carry "ask the kind", which is not what a row is for.

**The typed-array constructors.** `Uint8Array.of` and `Float64Array.of` build different things, so
which constructor was named is part of the answer and it comes from the receiver's element kind.
These are in the backlog with that reason rather than converted.

**`length` on a built-in.** `Object.getOwnPropertyNames(Object)` now answers, and lists the statics
plus `name` and `prototype` — everything a read can actually answer. It does not list `length`,
because reading it refuses, and a name in that list which a read cannot answer is the same lie in
the other direction.

## Consequences

- Adding a member to a built-in is now one row in one place, or two if the backend is not going to
  implement it. That is more ceremony than editing a list, and the ceremony is the product.
- `kiln-builtin-surface` sits beneath both engines and is therefore on their rebuild path. It holds
  `&'static str` and `const fn`s, with no macros, no generics and no dependencies, which is the
  shape ADR 0004 permits without an ADR of its own; this one records it anyway because the crate is
  new.
- The compile error names no member. That is a real cost of putting the gate in `const`
  evaluation, and it is paid down by the test, which the error message points at.
- `Object.getOwnPropertyNames` on a built-in constructor answers where it used to refuse, so
  `conformance/native/number-statics.ts` can enumerate rather than list — which its own header
  comment invited.
- The backlog is fifty-one members. That number is larger than any previous estimate, and it is
  larger for the reason the whole ADR exists: the previous estimates were the first refusal on each
  probe, and everything behind it was invisible.
- Nothing here makes the two engines *agree*; it makes them disagree loudly, in a build, at the
  moment somebody changes one of them. Closing the fifty-one is separate work, and now it is work
  with a list.
