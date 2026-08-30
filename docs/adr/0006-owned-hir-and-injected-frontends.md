# 0006. HIR is owned, and frontends are injected into the query engine

## Status

Accepted.

## Context

Design §14 item 4 is “Oxc → HIR.” ADR 0004 already requires Oxc to stay behind a
`frontend-oxc` crate and forbids Oxc types in signatures outside that crate. It also says
`kiln-db` has zero dependencies. Those rules were compatible while `Parsed` was a three-field
placeholder produced by a line scanner inside the database. A real arena-allocated parser makes
the missing ownership boundary unavoidable.

Putting Oxc directly in `kiln-db` would make every edit to the query engine rebuild Oxc's large
dependency graph and would make the supposedly replaceable query layer know which parser it was
driving. Retaining Oxc AST nodes in a memo would also retain arena lifetimes across revisions,
which is the wrong ownership model for a persistent compiler. Keeping all HIR types in
`kiln-db`, on the other hand, would turn the hottest crate into the type crate for every future
compiler pass and make it change continuously during language development.

## Decision

`kiln-hir` is a dependency-free foundational crate containing allocator-independent,
compiler-owned values. Oxc's arena and every Oxc type end at the `frontend-oxc` boundary. The
first slice owns spans, source kind, ordered top-level item classifications, requested modules,
ECMAScript export entries and parser diagnostics. Executable statement and expression HIR will
grow here as the interpreter is built.

`frontend-oxc` depends on Oxc and `kiln-hir`. Its public entry point is an ordinary function:

```text
parse(path: &str, source: &str) -> kiln_hir::Module
```

`kiln-db` depends only on compiler-owned value layers (`kiln-hir` and `kiln-ir`) plus the small
concrete `kiln-digest` identity primitive required by ADR 0010. It accepts the parse function
through the `Frontend` function pointer type when `MemoEngine` is constructed. The daemon and
timing harness inject `frontend_oxc::parse`; database unit tests inject a tiny fixture frontend.
This preserves three properties simultaneously:

1. Oxc is absent from the database's normal build and test dependency graph.
2. Production parsing is a memoized query, not a side channel.
3. Another frontend can be evaluated without changing the query engine.

The interface query projects runtime export names from the HIR. TypeScript type-only exports are
excluded. Explicit named and default exports participate immediately; star exports remain HIR
entries until module resolution can determine the names they contribute. Recoverable parser
diagnostics are cached with the parse result and cross protocol version 2 to the CLI.

This refines ADR 0004's “`kiln-db` has zero dependencies” rule to “`kiln-db` has no third-party
dependencies and may depend on the dependency-free compiler type layer.” The intent of the rule
was to keep heavy or volatile implementations off the central rebuild path; an owned type layer
serves that intent better than placing the types inside the database.

## Consequences

An edit to `kiln-hir` rebuilds `kiln-db` and every downstream compiler crate. HIR therefore stays
small, owned and deliberately boring; parser helpers and Oxc conveniences do not migrate into it.
This is the expected cost of changing a central compiler contract and is one of the rebuild cases
§13 requires us to measure.

The first HIR slice is module-header complete, not execution complete. Broad item kinds and spans
make the next body-lowering changes additive, but the interpreter cannot be built honestly until
statements and expressions have semantic HIR forms. That incompleteness is explicit in the
README rather than hidden behind an `Unsupported` path that pretends item 4 is finished.

Oxc upgrades are localized to `frontend-oxc`, although changes in the owned output can still be
semantic changes that alter interface hashes. The adapter tests therefore cover comments and
strings that resemble exports, default exports, type-only exports, imports, star re-exports,
TSX source selection and recoverable diagnostic spans. The daemon test covers the full
Oxc → HIR → memo → protocol path and reasserts early cutoff after a body-only TypeScript edit.
