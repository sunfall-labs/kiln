# ADR 0037: Module cycles use the specification's own walk, not a graph library

## Status

Accepted.

Applies ADR 0029's adoption rule to `kiln-module-graph`, and ADR 0004's corrected account of what
a dependency costs on the rebuild path. Supersedes nothing; the graph builder's previous refusal
of ESM cycles was an implementation limit rather than a recorded decision.

## Context

`kiln-module-graph` rejected every ECMAScript module cycle during construction. Circular imports
are legal JavaScript and ordinary in real code, and a refusal is worse than a wrong answer only in
the sense that it is honest: 61 Test262 variants stopped at `module dependency cycle`, and a
further 11 at the adjacent refusal a module importing from itself produced.

Supporting cycles means implementing two algorithms from the specification:

- `InnerModuleLinking` / `InnerModuleEvaluation` (16.2.1.5.1, 16.2.1.5.2), which carry
  `[[DFSIndex]]` and `[[DFSAncestorIndex]]` alongside a `[[Status]]` state machine; and
- `ResolveExport` (16.2.1.6.3), which walks re-export chains carrying a *resolve set* of the
  `(module, name)` pairs already on the current question's path.

The first is, on its face, Tarjan's strongly-connected-components algorithm with a module-specific
state machine on top. `petgraph` provides `tarjan_scc` and a mature set of traversal primitives,
it is the obvious ecosystem answer, and the workspace has no graph library today. ADR 0029 asks
that such a candidate be evaluated against its adoption rule and the reasoning recorded, so this
records it.

## Decision

**`kiln-module-graph` implements the specification's walks directly. `petgraph` is not adopted.**

Four reasons, in descending order of how much they decided it.

### 1. The graph does not exist before the walk

This is the one that settles it. `petgraph::algo::tarjan_scc` — like every algorithm in the crate
— takes a graph that has already been built. Kiln's traversal *discovers* the graph as it goes: at
each module it calls the source provider, parses and compiles the module, reads the dependency
specifiers out of the compiled `ModuleProgram`, and resolves each one through the
`ResolutionPolicy` before it knows what the edges even are. Loading is interleaved with the walk
because the walk is what decides which modules get loaded at all.

There is therefore no moment at which a complete `Graph` exists to hand to an algorithm. Producing
one would mean traversing the module graph in order to build a copy of the module graph, and the
first of those traversals is the one that has to cope with the cycle. The library would sit on the
far side of the problem it was brought in to solve.

This is ADR 0029's criterion 1 failing outright: there is no existing Seam here, only a place
where one could be carved if the loader were split from the walk — which would be a larger and
worse change than the one this ADR is about.

### 2. The evaluation order does not need strongly-connected components

`InnerModuleEvaluation` calls `ExecuteModule` immediately after its dependency loop and *before*
the stack pop that closes a strongly-connected component. A module already `[[evaluating]]` is
returned from at once. So the order a cyclic graph evaluates in is exactly the order a depth-first
walk leaves behind when back edges are skipped — plain post-order, no components involved.

`[[DFSIndex]]` and `[[DFSAncestorIndex]]` are not there to produce that order. They compute a
cycle *root*, and the root decides two things: where an asynchronous cycle's completion is
collected when top-level `await` is involved, and across which set of modules a single evaluation
error is recorded so that a later import of any member re-throws it. Kiln evaluates synchronously,
module by module, and implements neither of those yet. Adopting `tarjan_scc` today would import an
algorithm to compute a value nothing reads.

The builder's back-edge case is now one line — `continue` — with a comment explaining that this
*is* the specification's algorithm rather than an approximation of it. That is the whole of the
ordering change.

### 3. `ResolveExport` is not a graph traversal

The half of the work that is genuinely intricate is export resolution, and no graph library helps
with it. Its cut is not on modules but on `(module, name)` pairs, and the set is per-question
rather than global: one module is legitimately revisited many times under different names, and
revisiting it under the *same* name is the definition of the circular import request whose answer
is "no such export". `export *` needs a third rule again — the set is copied per branch, so that
two stars reaching the same name by different routes is ambiguity rather than a cycle.

That is a recursive descent over an export table with a path set, which is what the specification
writes and what this crate now contains. It is not expressible as a traversal of a `petgraph`
graph, because its nodes are not the graph's nodes.

### 4. Cost, on ADR 0004's corrected terms

ADR 0004's 2026-08-29 amendment is precise about what a dependency actually costs per edit:
generic code from a dependency is instantiated in the crate that uses it, so its codegen is
charged to the edited crate rather than to the dependency. `petgraph`'s core type is
`Graph<N, E, Ty, Ix>`, generic in four parameters and used through generic algorithms, which is
exactly the shape that moves compile cost onto the editing loop. `kiln-module-graph` sits on the
daemon's rebuild path.

Against that, adoption would also require a `NodeIndex` ↔ `PathBuf` bimap maintained beside the
existing `BTreeMap<PathBuf, ModuleNode>`, purely so the two representations agree — an Adapter
with no second use, which criterion 1 also rejects. And it fails criterion 6, the deletion test:
removing `petgraph` again would put back roughly twenty lines of depth-first walk, not substantial
complexity.

`default-features = false` would have been mandatory had we adopted it (`petgraph` defaults pull
in `serde` support and the stable-graph implementations Kiln has no use for), but the question does
not arise.

## Consequences

The crate keeps one traversal, in one place, that both discovers and orders the graph, and an
export resolver that reads like the specification clause it implements. A reader comparing either
against ECMA-262 can do so line by line, which is worth more here than the reuse would have been.

The recursion in `ResolveExport` and `GetExportedNames` is bounded by the sets they cut on, so both
terminate; their depth is bounded by the number of distinct `(module, name)` pairs, which for a
pathological re-export chain is the graph's size. The previous bottom-up pass was iterative and
noted that it avoided this. A graph deep enough to overflow the stack here would be a graph no
engine resolves iteratively either, but it is a real difference and it is recorded rather than
hidden.

**Revisit when top-level `await` participates in a cycle.** That is the point at which
`[[CycleRoot]]` becomes load-bearing — an asynchronous cycle's completion genuinely is collected at
the condensation's root — and, importantly, it is also the point at which the objection in §1 goes
away: linking completes before evaluation begins, so by then a fully built graph does exist. If
that work lands, `petgraph`'s `tarjan_scc` should be reconsidered on its merits over the
already-constructed graph, against the same six criteria.
