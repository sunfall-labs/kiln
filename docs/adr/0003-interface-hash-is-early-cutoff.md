# 0003. The interface hash is an early-cutoff query, not a hand-maintained second hash

## Status

Accepted. This is a refinement of design.md §3 and a deliberate deviation from it as written.

## Context

§3 gives every module two hashes. The implementation hash changes whenever internal code
changes; the interface hash changes only when exports or externally relevant semantics change.
It then states the property the whole scheme exists for: "dependents are recompiled only when
the interface hash changes."

Read literally, that sentence describes a rule. Something computes an interface hash, something
stores the previous one, something compares them, and every query that depends on another
module's interface has to remember to consult the comparison before deciding to recompute. Rules
of that shape hold exactly as long as everyone remembers them, and both ways of forgetting are
bad in ways that are hard to notice. A dependent that skips the check recompiles too often,
which is invisible except as a compiler that is mysteriously slower than it should be. A hash
that omits a field recompiles too rarely, which produces a wrong binary and surfaces much later,
somewhere else.

The engine of §2 already has to make this comparison for its own purposes. A memoizing engine
that recomputes a query and gets the same value back has no reason to invalidate that query's
dependents; the recompute happened because an input moved, but nothing an observer can see has
changed. That is early cutoff, and it is a property of the engine rather than of the compiler's
query schema. If the engine has it, §3's sentence is not a rule anyone has to follow. It is a
consequence.

## Decision

`interface(file)` is an ordinary derived query returning an ordinary value, and the engine
guarantees that recomputing a query to an equal value does not invalidate its dependents. That
guarantee is written into `QueryEngine`'s documentation as the second of its three obligations
and is asserted on by tests, so it is part of the contract a backend must satisfy rather than an
implementation detail of `MemoEngine`.

Given that, §3 falls out of the dependency graph. `project()` depends on `interface(f)`, not on
`parse(f)`. An edit inside a function body bumps the revision, reparses the file, recomputes the
interface, finds it equal to the cached one, and stops there. Nothing downstream runs, and no
code anywhere had to know that this was the intended behavior.

Both hashes survive as values, but their role changes. `implementation_hash` and
`interface_hash` are now compact summaries: they cross the wire in `kiln-proto::ModuleSummary`,
they make cache keys legible, and tests assert on them because comparing two `u64`s is a clearer
statement of intent than comparing two structs. The mechanism is not the hash. The mechanism is
`Interface: PartialEq`.

## Consequences

The engine has to compare values, so query results need cheap equality. This is a genuine
constraint on the query schema and not a free win. A large IR body is expensive to compare, and
a value carrying arena indices or interior pointers may have no equality that means anything.
The remedy in those cases is to make the query that sits at the cutoff point small rather than
to give up the mechanism — which is exactly what `Interface` is. It carries the export names and
a hash, not the parse tree. Where a big value must be compared, hash it and cut off on the hash;
where a value has no stable equality, that value is not a cutoff point and something derived
from it is.

Equality has to be semantic rather than incidental, and this is where the mistakes will be.
`Interface` carries `exports: Vec<String>`, so export order participates in equality: reorder
two exports without changing anything a dependent can observe and every dependent is
invalidated anyway. That is the safe direction to fail — slower, never wrong — but it should be
a decision rather than an accident, which means whoever produces an `Interface` normalizes it.
The same reasoning applies to anything else we later add to the interface: if a dependent cannot
observe it, it does not belong in the value.

Cutoff makes recompute counts non-obvious, so they have to be observable. `Stats::early_cutoff`
counts recomputes that produced an identical value, and the tests that matter here assert on
counters rather than on outputs: after an edit to a function body, `parse_computed` increments
and `project_computed` does not. Without that, an engine that quietly lost cutoff would still
pass every correctness test and would only show up as a slow compiler months later.

The deviation should be stated plainly, because someone will read §3 and go looking. There is no
place in the codebase where an interface hash is compared against a stored previous value in
order to decide whether to recompile dependents. That comparison is the engine's generic value
comparison, applied to `Interface`. A backend that provides memoization but not early cutoff
would satisfy every signature in `QueryEngine` and would break §3 silently, which is why the
guarantee is documented on the trait and tested rather than left to the implementation. It is
also the hardest requirement in ADR 0002's list, and much of why salsa is the candidate: it
provides exactly this.
