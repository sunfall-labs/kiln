# 0004. Workspace rules are rules, not aspirations

## Status

Accepted. Amended 2026-08-29: the original Decision justified `kiln-db`'s zero-dependency rule
with a claim about rebuild cost that is false. The rule survives in a narrower and more accurate
form; the correction is recorded in the Context below rather than edited out of it.

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

### Correction: what a dependency on the rebuild path actually costs

As first written, this ADR defended `kiln-db`'s zero-dependency rule by asserting that "every
dependency it takes is paid by every clean build and by a large share of incremental ones." The
first half is true and unremarkable. The second half is wrong. Cargo compiles a dependency once
per profile and feature set and caches the result; editing `kiln-db` does not rebuild `serde` or
anything else beneath it. A dependency that is already built is not on the per-edit path at all,
and no amount of editing puts it back there.

What does cost time on every edit is narrower and more specific. A procedural macro in the
edited crate is re-run and its expansion re-typechecked on every rebuild of that crate, so the
macro's own compile time is amortised but its expansion time is not. Generic code from a
dependency is instantiated in the crate that uses it, which means its codegen is charged to the
edited crate rather than to the dependency, and a dependency whose API is generic enough can put
more work on the per-edit path than its own source ever did. And a change to a foundational
crate's public surface rebuilds everything downstream of it, which is a property of where the
crate sits rather than of what it depends on.

That correction matters because the rule was load-bearing in the wrong direction. "Adding a
dependency to either crate is an ADR, not a commit" turned a false premise into a procedural
barrier, and the first thing it blocks is something the project needs: a real hash function
(ADR 0007) and a non-SipHash map for process-local tables. A rule that is expensive to follow
should at least be true.

## Decision

`kiln-proto` and `kiln-db` are foundational. They sit on the rebuild path of everything else and
they change additively or not at all. Neither crate takes a dependency that expands inside it:
no procedural macros and no generic-heavy libraries, which are the two shapes that move a
dependency's compile cost onto the crate that is being edited. `kiln-proto` depends on `serde`
alone, with default features off, and its derives are the stated exception — a wire format needs
serialization, and `kiln-proto` is by design one of the least frequently edited crates in the
workspace, which is where a macro belongs if it has to live anywhere.

A dependency that is a plain non-generic leaf — `rustc-hash` is the type: no macros, no build
script, a small API of concrete functions and types — is not what this rule is aimed at, and
adding one is a commit rather than an ADR. The commit records the check: what the dependency's
macro, build-script and generic surface is, and the `cargo build --timings` critical path before
and after. Anything that fails that test still needs an ADR first. The expectation is that most
such ADRs conclude no; ADR 0007 is the first one and does not, which is the argument for a
written case rather than a ban.

Salsa remains the rule's real target and is still excluded by it. Its ergonomics come from
attribute macros over the query definitions and from database and query types that every
consumer is generic over, so it is both of the disallowed shapes at once, in the crate that is
edited most. ADR 0002 records the evaluation; this is the workspace-side reason the answer is
"behind the trait, not in `kiln-db`."

Dev-dependencies are outside this rule entirely. `proptest` and anything like it never enter a
consumer's build graph and never appear in a rebuild of anything but the test target of the
crate that names them, so they are governed by whether they earn their keep as tests, not by
this ADR.

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
is `serde`'s, in `kiln-proto`. New derive-heavy dependencies belong in leaf crates or nowhere.
Build scripts are the same rule with a lower threshold: a `build.rs` anywhere in this workspace
needs an ADR first, because build scripts serialize the build graph in ways `--timings` makes
visible and nothing else does.

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
A change that adds a macro-bearing or generic-heavy dependency to `kiln-proto` or `kiln-db`,
introduces a build script, names an Oxc type outside `frontend-oxc`, or makes the runtime depend
on a compiler crate carries its justification in the commit message together with the
`cargo build --timings` critical path before and after. If it moves one of §13's recorded
numbers, it needs an ADR of its own. The failure mode this is aimed at is not the deliberate
exception; it is the accumulation of undiscussed ones.

The narrower rule is harder to apply than the old one, and that is a real cost. "Zero
dependencies" is checkable by reading a manifest; "no proc macros and nothing generic-heavy"
requires looking at what a crate actually exports and judging how much of it gets instantiated
here. The judgement is sometimes wrong. It is still the right rule, because the checkable
version was banning things it had no reason to ban and would have kept banning them.

Some of these rules will cost real ergonomics. Writing a translation layer between Oxc's AST and
our HIR is work that a direct dependency would have avoided, and a hand-written serialization
would be worse than a derive. We are choosing the version of the codebase that stays fast to
rebuild over the version that is slightly nicer to write, which is what putting iteration speed
second in the priority order means in practice.

Following the rules is not sufficient on its own. A workspace can obey all ten and still be slow
because one crate grew a function that takes forty seconds to codegen. That is why the
enforcement mechanism is the measurement rather than the rule list: `--timings` and the harness
are the authority, and §12 is how we expect to keep them healthy. This correction is an argument
for that mechanism rather than against it: the premise was wrong from the day it was written and
no measurement contradicted it, because a rule list is not the sort of thing that can be wrong in
a way that shows up in a number.
