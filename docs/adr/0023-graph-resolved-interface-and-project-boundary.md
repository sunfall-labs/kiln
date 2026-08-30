# ADR 0023: Per-file interfaces, project rollups, and graph resolution remain separate

## Status

Accepted.

This decision records the boundary between the query database's per-file and whole-project
values and the entry-rooted module graph. It refines ADR 0003's query-level early-cutoff rule and
ADR 0011's graph/linkage boundary; it does not supersede either decision.

## Context

Kiln has three related views of source code, and treating them as one value makes their different
invalidation rules easy to lose.

`Interface(file)` is a query result for one active source file. It describes that file's declared
runtime export surface after normalization, together with the normalized linkage identity used by
the early-cutoff boundary. It is intentionally small and comparable. Its equality decides whether
dependents of that file need to be invalidated, as specified by ADR 0003. It does not require a
filesystem root, an import traversal, or the resolved origin of a name forwarded through another
module.

`Project` is a query result over the daemon's active source set. It includes every active source's
logical path and interface, folded in deterministic path order. It makes the whole active input
set observable for project summaries and project-level invalidation. It is not a reachability
analysis from one entry module: an active file that no current entry imports still belongs to the
project rollup.

`ModuleGraph(entry, project_root)` is a different boundary. It resolves explicit filesystem
imports, walks only the entry-rooted reachable modules, records dependency order, and resolves
visible names to their canonical exporting module and slot. This information is needed by
execution and native ABI wiring, but it is not a replacement for the per-file interface query or
the all-active-source project rollup.

The distinction matters for clients as well as for the implementation. A `Check` request currently
has a path and project root, but its summary is a per-file result. A future graph-aware check may
expose optional resolved exports when graph context is available. A fully graph-resolved project
summary would need an explicit entry/root and protocol contract; it must not be inferred by
silently changing the meaning of the existing `Project` request.

## Decision

### `Interface(file)` remains the per-file early-cutoff value

The interface contains the declared runtime exports in canonical order and the normalized
linkage identity required to compare equivalent declarations. It remains independent of graph
reachability and of the spelling of import specifiers in consumers. A graph may use this value as
an input, but graph-resolved export origins are not folded into the ordinary interface value.

Changing the interface to include resolved origins, dependency paths, or project-root context
would change the invalidation boundary described by ADR 0003 and requires an explicit amendment,
not an incidental field addition.

### `Project` remains the deterministic all-active-source rollup

The project query consumes every active source file and its interface, sorted by logical path
before folding. Its identity therefore changes when an active module is added, removed, renamed,
or changes its externally visible interface, even if that module is unreachable from a particular
entry. File interning order is not project identity.

The existing `Project` request and `ProjectSummary` retain this meaning. A future graph-resolved
project value may be added as a distinct query and response, but it must name the entry/root and
define whether it is an execution graph, a packaging graph, or another explicit projection.

### `ModuleGraph` owns resolved names and origins

The module graph owns canonical dependency targets, dependency-first order, and link-time export
resolution. A resolved export identifies its concrete local or namespace origin; an ambiguous
resolution remains distinguishable from a missing export. Consumers that construct import/export
ABI wiring use this resolved graph result rather than searching a provider's raw export list.

Graph resolution failures remain diagnostics or structured graph errors. When a graph context was
requested, the daemon must not silently fall back to a single-file interface or an incomplete
project rollup, because doing so can produce a successful-looking result with the wrong imports.

### Check may add optional graph information

The existing per-file `Check` result remains valid without graph construction. An additive future
field such as `resolved_exports: Option<...>` may be populated when the request explicitly asks
for graph context and graph resolution succeeds; it is `None` when no graph was built. The shape
of that field, its path/slot identity, and its wire-version policy are follow-up protocol work.
Its absence today is not permission to reinterpret `exports` as graph-resolved origins.

## Consequences

- ADR 0003 continues to govern `Interface(file)` equality and early cutoff.
- ADR 0011 continues to govern entry-rooted traversal, filesystem resolution, and linkage.
- `Project` summaries remain useful for the complete daemon source set and do not accidentally
  shrink when an entry imports fewer files.
- Native and interpreter graph consumers share one resolved-origin source of truth, while ordinary
  single-file queries retain their existing APIs.
- A graph-resolved project or a wire-visible resolved-export report requires a separate design:
  it must specify entry/root inputs, reachability, error reporting, and protocol compatibility.

## Acceptance checks

- A body-only edit can cut off at `Interface(file)` without recomputing `Project`.
- Project identity is unchanged by client interning order but changes for active-path or interface
  changes.
- A graph consumer resolves a forwarded/star export through the graph's canonical origin rather
  than a raw provider export table.
- Missing, ambiguous, cyclic, or out-of-root graph resolution is reported as a diagnostic or
  structured graph failure; it is never silently treated as a successful single-file result.
- Any future `resolved_exports` field is additive and explicitly distinguishes `None` (no graph
  context) from a graph that failed to resolve.
