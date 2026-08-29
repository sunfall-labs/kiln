# 0004. Workspace rules are rules, not aspirations

## Status

Accepted.

## Context

Design.md §12 lists ten design rules for keeping Rust rebuild time under control: keep
foundational type crates small and stable, avoid changing central generic types frequently, keep
Oxc isolated behind `frontend-oxc`, separate interfaces from implementations, minimize
procedural macros in frequently edited crates, avoid expensive build scripts, disable
unnecessary dependency features, keep the dependency graph shallow, put experimental optimizers
in leaf crates, and avoid forcing the runtime to rebuild when the compiler changes.

Every one of those is correct and every one of them is the sort of thing that gets violated on a
Tuesday afternoon by someone who needs a hash function and reaches for a crate. The violations
are individually reasonable and collectively fatal: a workspace does not become slow to rebuild
in one commit, it becomes slow in forty, none of which was worth arguing about at the time. Loop
1 in the design's list of three — compiler developer edits Rust, rebuilds, runs tests — is the
loop the compiler's own authors live in, and it is the only one that degrades without anyone
filing a bug.

This ADR restates §12 as rules with a subject, a scope, and a stated cost for breaking them,
while the workspace is four crates and the rules are still cheap to follow.

## Decision

`kiln-proto` and `kiln-db` are foundational. They sit on the rebuild path of everything else and
they change additively or not at all. `kiln-db` has zero dependencies and keeps it: the query
database is the hottest crate in the workspace, and every dependency it takes is paid by every
clean build and by a large share of incremental ones. `kiln-proto` depends on `serde` alone,
with default features off, because a wire format needs serialization and nothing else. Adding a
dependency to either crate is an ADR, not a commit — not because a dependency is forbidden but
because the argument is worth writing down once.

Interfaces are separated from implementations at the crate boundary. `kiln-db` defines
`QueryEngine` and ships `MemoEngine`, and consumers name the trait; ADR 0001 has the full
reasoning. The same shape applies as the compiler grows: the crate that defines what an
optimizer is is not the crate that contains one.

Oxc is isolated behind a `frontend-oxc` crate whose public surface is kiln's own HIR. No type
from Oxc appears in a signature outside that crate. Oxc is a large dependency tree and a moving
target, and the two things this buys are worth the cost separately: an Oxc upgrade touches one
crate rather than every crate, and an edit anywhere else in the compiler does not put Oxc's
codegen on the critical path of the rebuild. The cost is a translation layer we would not
otherwise write, and we pay it deliberately.

Procedural macros stay out of frequently edited crates. Today the only derive in the workspace
is `serde`'s, in `kiln-proto`, which is by design one of the least frequently edited crates. New
derive-heavy dependencies belong in leaf crates or nowhere. Build scripts are the same rule with
a lower threshold: a `build.rs` anywhere in this workspace needs an ADR first, because build
scripts serialize the build graph in ways `--timings` makes visible and nothing else does.

Dependency features are opted into rather than out of. The workspace manifest sets
`default-features = false` and names what it needs; new dependencies follow suit.

Experimental optimizers and anything else speculative go in leaf crates that nothing depends on,
so that abandoning them is a deletion rather than an excavation.

The runtime does not depend on compiler crates. This is §6's stable HostOps ABI seen from the
workspace side: if the runtime library's dependency edges do not reach the compiler, then no
compiler change can rebuild it, which is the property §6 is buying. It is easier to preserve
from the start than to recover later.

`cargo build --timings` is inspected on a schedule and whenever a rebuild subjectively feels
slower than it did. The numbers that §13 gates on — no-op rebuild, typical leaf-crate rebuild,
rebuild after an IR-crate edit — are recorded by the timing harness alongside the JS-side
numbers, because §13 treats compiler build time and compiled-program latency as the same class
of thing and so should we.

## Consequences

Breaking a rule is allowed and sometimes right. What is not allowed is breaking one silently.
A change that adds a dependency to `kiln-proto` or `kiln-db`, introduces a build script, names
an Oxc type outside `frontend-oxc`, or makes the runtime depend on a compiler crate carries its
justification in the commit message together with the `cargo build --timings` critical path
before and after. If it moves one of §13's recorded numbers, it needs an ADR of its own. The
failure mode this is aimed at is not the deliberate exception; it is the accumulation of
undiscussed ones.

Some of these rules will cost real ergonomics. Writing a translation layer between Oxc's AST and
our HIR is work that a direct dependency would have avoided, and a hand-written serialization
would be worse than a derive. We are choosing the version of the codebase that stays fast to
rebuild over the version that is slightly nicer to write, which is what putting iteration speed
second in the priority order means in practice.

Following the rules is not sufficient on its own. A workspace can obey all ten and still be slow
because one crate grew a function that takes forty seconds to codegen. That is why the
enforcement mechanism is the measurement rather than the rule list: `--timings` and the harness
are the authority, and §12 is how we expect to keep them healthy.
