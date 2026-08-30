# 0001. The query layer sits behind our own trait

## Status

Accepted.

## Context

Design.md §2 models compilation as a graph of memoized queries over versioned inputs, from
`source(file)` down to `compile_object(module, target)`. That is precisely what salsa does, and
§2 names it as worth evaluating. The same section adds the constraint this decision is about:
"we should hide it behind our own query interface so the compiler is not permanently tied to
it."

There are three reasons that constraint is worth honouring rather than treating as a nicety.

The first is that salsa is explicitly a work in progress and has already been through one
ground-up redesign of its public API. A compiler whose every crate names salsa's types inherits
that churn at the worst possible granularity: not as "upgrade a dependency" but as "touch every
file that mentions a query." Priority 3 in the design's ordering is architectural flexibility,
ranked above native runtime performance, and a permanent commitment to a pre-1.0 library's
generics is the opposite of it.

The second is §12. Salsa's ergonomics come from procedural macros and from generic types that
every query participates in. §12 asks us to minimize procedural macros in frequently edited
crates and to keep foundational crates small, because those are the crates on the rebuild path
of loop 1. A derive-driven database type threaded through the whole compiler is exactly the
shape that makes an incremental Rust rebuild slow, which would mean paying for the user's
iteration speed with the compiler developer's.

The third is that we do not actually know yet what we need. The queries in §2 have properties
that are easy to state and easy to get wrong — early cutoff above all (ADR 0003) — and the
honest way to find out whether a candidate backend provides them is to have written down what
we require. A trait is that written form.

## Decision

`kiln-db` defines the `QueryEngine` trait together with the value types the compiler exchanges
with it: `FileId`, `Revision`, `Parsed`, `Interface`, `Project`, `Stats`. Everything above the
database — the daemon, the CLI, the timing harness, and in time the frontend, the interpreter
and the backend — depends on that trait and on those types, never on a query library.

The trait carries three guarantees in its documentation, and they are the substance of the
contract rather than commentary on it: a query is recomputed only when a transitive dependency's
value changed; recomputing a query to an equal value does not invalidate its dependents; and
reads at a fixed revision are pure. `Stats` exists so those guarantees are observable from
outside the engine. Recompute counts are a first-class output, not debug logging, because a
claim about invalidation that nothing asserts on is a claim that quietly stops being true.
The input side also exposes each active source's last mutation revision. That is ordering
metadata, not another invalidation mechanism: daemon adapters use it to collapse relative and
canonical spellings without letting an older disk or editor value win by accident. Every request
that resolves a source against the filesystem, including a single-file check, carries the caller's
project root and performs that reconciliation before entering the query pipeline; checking and
executing the same editor buffer cannot observe different source identities.

`kiln-db` has no parser, procedural-macro, or generic query-framework dependency, and v0 ships
`MemoEngine`, a hand-written implementation. Its dependencies are the compiler-owned HIR/IR
values it memoizes and the small concrete `kiln-digest` primitive required by ADR 0010 when a
query value has no full unhashed value behind it. `MemoEngine` is both the thing that works today
and the executable specification of what a backend must do. A salsa-backed engine, if we adopt
one, is a second implementation of the same trait living in its own crate behind a feature or a
constructor choice; no consumer changes, and the existing tests become the acceptance suite for
the swap. ADR 0002 records what we require of such a backend.

## Consequences

We own the memoization, which is real work and a real risk: if `MemoEngine` gets dependency
tracking wrong then everything above it is wrong, and the failure is the quiet kind where the
compiler produces a stale answer rather than crashing. This is why the engine's tests are about
invalidation behavior and not only about return values.

The trait is a lowest common denominator by construction, and it will constrain us. Most
concretely: the query methods take `&mut self`, which is what a memoizing engine needs and what
rules out sharing one engine across threads without external synchronisation. Parallel query
execution — §8 wants independent translation units compiled in parallel — will require revising
the trait. We prefer to meet that as a deliberate change to a small interface rather than as a
rewrite of code that had a library's concurrency model baked into it.

The trait must not accrete backend-specific concepts. In practice that means no library
`Database` handle in a signature, no attribute macros in the trait's vocabulary, and no leaking
of a particular backend's interning or durability model into the types. If a candidate backend
cannot be expressed behind this interface, that is information about the backend, and it belongs
in ADR 0002 rather than in a widening of the trait.

Finally, the cost of evaluating a backend is now bounded and known: write one implementation,
run the same tests. That is the property the constraint was for.
