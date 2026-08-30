# ADR 0007: Interpreter bytecode and HostOps are separate stable boundaries

## Status

Accepted.

## Context

The founding design makes interpreter-first execution part of the critical edit-to-run path and
also requires native backends to execute the same semantics. Letting either Oxc AST nodes or VM
implementation details become the shared representation would make parser upgrades, interpreter
changes, and future native lowering move together. It would also make host I/O an implicit set of
Rust calls instead of a contract that native artifacts can target.

## Decision

Kiln lowers owned HIR into a versioned, parser-independent stack bytecode in `kiln-ir`. The query
database memoizes that compile result as `ir(file)`. VM launches are deliberately not memoized:
the bytecode is pure compiler output, but executing it has observable HostOps effects which must
happen on every `run` request.

`kiln-interpreter` depends on bytecode and the compiler-independent `kiln-host-abi`; it does not
depend on HIR or Oxc. The ABI crate may use allocation-free runtime semantic cores, but no compiler
crate. Native operations cross `HostOps` using append-only numeric operation identifiers
and owned scalar values. `kiln-runtime` is one provider and the CLI currently uses its deterministic
capturing host.

The first bytecode version executes variables, scalar literals, arithmetic and comparisons,
assignments, blocks, conditionals, loops, short-circuit operators, and `console.log`. Valid syntax
outside that surface remains explicit `Unsupported` HIR and becomes a source-ranged compile
diagnostic. Parser diagnostics remain syntax-only, so `kiln check` does not confuse frontend
coverage with executable-backend coverage.

## Consequences

- Warm runs reuse parsing and bytecode while replaying program side effects.
- Parser, interpreter, and future native backend implementations can evolve independently.
- Host behavior has a testable ABI seam from the first executable slice.
- Bytecode versioning and HostOps versioning are separate because an internal instruction-set
  change does not necessarily break compiled native artifacts or runtime providers.
- Module linking, functions, objects, exceptions, async behavior, and native emission remain
  explicit later slices rather than being approximated incorrectly.
