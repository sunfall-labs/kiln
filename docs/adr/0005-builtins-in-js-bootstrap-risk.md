# 0005. Built-ins in JS/TS, and the bootstrap risk that comes with them

## Status

Accepted, with the mitigations below treated as part of the decision rather than as follow-up
work.

## Context

Design.md §5 says that as much runtime behavior as possible is written in JS/TS — Node argument
validation, error construction, streams, `EventEmitter`, the `Buffer` and filesystem facades,
promise/callback conversion, utility algorithms — so that changing a built-in does not require
recompiling Rust. Rust keeps only what genuinely needs native access or native performance.

This is the most valuable idea in the design document. It takes the largest and most frequently
edited part of a JS runtime and moves it out of loop 1, where a change costs a Rust rebuild
measured in seconds to minutes, into loop 2, where a change costs an incremental recompile of a
few JS modules and a targeted test run. §13's target for that loop is under three seconds. No
other single decision in the document moves as much work across that boundary.

It is also the highest-risk idea, for a structural reason worth naming precisely: it makes the
compiler depend on its own output. Built-ins must be compiled by kiln before any user program
can run, so the compiler's correctness becomes entangled with its ability to compile one
specific corpus, very early — well before the frontend is finished. And under §7 the compiled
built-ins become a cache input to every subsequent build, so a built-in artifact that is stale
or subtly wrong is not a local failure. It is a wrong answer that reproduces.

The Porffor compiler makes exactly this bet, and the sharpest edges in its codebase are all at
this seam. Three are worth recording because they are the failure modes we are inheriting, not
hypotheticals.

Its precompile pass has to be run twice. `compiler/precompile.js` compiles the built-in sources
and writes `compiler/builtins_precompiled.js`, and the compiler imports that generated file
while compiling built-ins. Cross-file references between built-ins therefore resolve against the
*previously written* artifact, so adding or renaming a built-in function needs a second pass
before the tree is self-consistent. It converges, but "run it again" is not a property one wants
in a build.

Its compile-time flags are one-shot. Values such as `__porfComptimeFlag` are resolved during
compilation and baked into the output. If the file that establishes a value is included one pass
too late, the reference resolves to something else and nothing complains: the build succeeds and
is wrong.

Its prototype objects are materialized lazily (the `lazyObjects` preference). Because
materialization is triggered by what the program happens to mention, a fix could appear to work
only when the test program touched something unrelated that forced materialization, and the
reproduction of a bug depended on program text far away from the bug.

The three share a shape. Each is an ordering dependency between the compiler and its own
built-in artifacts that is invisible in the source, which is what makes the resulting failures
non-local and order-sensitive.

## Decision

We keep §5. The mitigations below exist so that the ordering dependencies are explicit in the
dependency graph rather than implicit in the build script.

Built-in compilation is a normal query in the database, not a side channel. `builtin_source`,
`parse`, `interface`, and everything downstream are the same queries user code goes through, in
the same engine, with the same invalidation. This is the mitigation that matters most, because
Porffor's two-pass problem is a cycle between artifacts that has been broken by writing one of
them to disk and reading it back. Expressed as queries, a cross-built-in reference is an
ordinary dependency edge: the engine either resolves it in one pass or reports a cycle. A cycle
we can see is a much better outcome than a fixpoint we iterate by hand. Concretely this means no
separate precompile binary, no generated built-in artifact checked into the tree, and no import
of a previously written artifact during compilation.

§6's HostOps ABI is the seam, and it is versioned and stable before the built-in corpus grows.
Built-ins are written against HostOps and nothing else. This is what actually makes "changing a
built-in does not recompile Rust" true in both directions: if the only thing a built-in can
reach is a versioned ABI, then "this artifact is valid for compiler build X" reduces to "this
artifact is valid for ABI version N", which is a number we can compare rather than a judgement
call.

Cache keys carry the compiler version and the runtime ABI version, per §7, and built-in
artifacts are keyed exactly like everything else with no fast path. §7 notes that built-in
artifacts should be highly durable because they change far less often than application code,
which is true and is also precisely why they are the artifacts most likely to be silently stale.
Durability raises the stakes on the key rather than lowering them. There is no "close enough"
reuse: a built-in artifact whose key does not match is recompiled.

No compile-time value is one-shot. Anything resolved at compile time is a query result keyed on
its inputs, so resolving it twice yields the same answer and resolving it in a different order
is not observable. Where a value genuinely can be computed only once — a global layout decision,
an assigned index — it is produced by a single query that every consumer depends on, which turns
the ordering requirement into a dependency edge instead of a convention.

Laziness may affect performance and must not affect semantics. Whether a prototype is
materialized can legitimately depend on what a program uses; what that program observes cannot.
The conformance suite therefore includes cases that exercise a built-in with nothing else in the
program, because a single-feature program is exactly the shape Porffor's lazy-object bug hid
from, and it is the shape a large suite is least likely to contain by accident.

Built-ins are in the tier-0 smoke suite from the day built-ins exist, and §15's
interpreter/native equivalence check covers them. Everything else stands on the built-ins, so
they are the first thing the two backends must agree about, not the last.

## Consequences

The HostOps ABI has to be designed earlier than §14's ordering suggests. Item 9 places the Rust
runtime after the C backend, and the runtime *implementation* can stay there, but the ABI's
shape and its version discipline cannot: they are a precondition for the built-in corpus growing
past a handful of modules. This is a small, deliberate reordering within §14 rather than a
change to it.

The query engine meets a real cross-referencing corpus early, since the built-ins are one. That
is a good forcing function and consistent with §14's intent that incrementality is built before
the compiler is large: the built-ins are the first realistic test of whether invalidation
actually works, and they arrive years before a large user project would.

Built-in compilation sits on the critical path of a cold start, so it has to be cached across
sessions or daemon startup regresses. §13's warm-start and startup numbers should be measured
with the built-ins in the database, not against an empty one, or they will measure the wrong
thing and look good doing it.

If a stale built-in artifact is ever reused in a way the cache key did not catch, the defect is
in the key, not in the built-in. Fixing the built-in in that situation makes the symptom go away
and leaves the mechanism intact, which is the worst available outcome.
