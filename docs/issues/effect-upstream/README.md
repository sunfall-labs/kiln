# Direct TypeScript Effect suite

Goal: execute every pinned Effect test directly in kiln, prioritizing native, without
bundling sources. Bun is the primary direct-TypeScript reference. The official
Node/Vitest runner checks runner fidelity separately.

## Corpus and measurement

`conformance/effect-upstream/corpus.json` pins Effect 4.0.0 at
`67ba4e46a11ccda0b6761578bfd22c04ae00167d`, including all 322 test files.
The official Node/Vitest run registers 11,019 cases: 10,974 pass and 45 skip.
Evidence: `.scratch/effect-oracle/all.json` in the owned effect-complete worktree.
The Porffor page's published denominator and runner differ; its percentage is
not interchangeable with this inventory.

The complete Bun 1.4.2 direct-source reference registers the same 11,019 cases:
10,936 pass, 28 fail and 55 skip across all 322 files, with no incomplete files.
Ten failures name unimplemented loader/reset capabilities; the remaining failures
retain genuine runtime, formatting, assertion or child-process differences from
the official Node run. `bun-reference.json` records the case IDs, pin, adapter
digests and full report digest. These are correctness counts, not performance
measurements. Runtime-specific skip modes are preserved and compared explicitly.

`scripts/effect-upstream.py` prepares an owned, locked cache workspace and loads
TypeScript test sources directly. It pins adapter dependencies, imports upstream
`vitest.setup.ts`, retains per-file logs and case IDs, and records timeouts,
missing inventories and missing summaries as incomplete runs. Bun and kiln use
the same source graph, setup and adapter. No bundler participates. Native uses
`native-dev`; this correctness run is not a steady-state performance benchmark.

The adapter uses actual Vitest assertion/spy libraries, but does not yet implement
all Vitest loader capabilities. `HARNESS_UNSUPPORTED` is unresolved work, never a
passing test or an upstream skip. Official Vitest passes all runnable cases.

## Implementation direction

TypeScript erasure and enum lowering belong in the common frontend/Owned HIR.
Native DCE must erase expression-only TS wrappers before JS-oriented Oxc folding;
it must not introduce an independent runtime TS transform. Optional chains,
reference evaluation order and method receivers must retain their semantics.

Indirect native eval must dispatch to the existing dynamic evaluator, preserve
global versus lexical scope, and reject unsupported source catchably. Syntax
validation must precede source execution.

Vitest module reset requires distinct source and module-instance identities.
Fresh instances stay in the same realm (including `Symbol.for` identities), and
old closures retain their instance's imports and cells. Native immutable code can
be shared; mutable link state must belong to a rooted module instance. The realm
owns shared host services. A global active-generation switch would incorrectly
redirect old closures. Mocking needs an explicit test-loader capability and import
facades; ordinary ESM namespace objects retain their language semantics.

## Outstanding

- Complete native direct-source execution and compare every case against Bun.
- Match official runner setup, lifecycle, mocking and module resets.
- Implement actual GC-related capabilities where required; do not fake `gc`.
- Address divergences by semantic owner and retain focused native parity probes.
- Run the repository gate before each finished milestone is pushed to main.
