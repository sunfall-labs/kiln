# Direct TypeScript Effect suite

Goal: execute every pinned Effect test directly in kiln, prioritizing native, without
bundling sources. Bun is the primary direct-TypeScript reference. The official
Node/Vitest runner checks runner fidelity separately.

## Corpus and measurement

`conformance/effect-upstream/corpus.json` pins Effect 4.0.0 at
`67ba4e46a11ccda0b6761578bfd22c04ae00167d`, including all 322 test files.
The official Node/Vitest run registers 11,019 cases: 10,974 pass and 45 skip.
Evidence: `.scratch/effect-oracle/all.json` in the owned effect-complete worktree.
The [Porffor page](https://porffor-effect.porch.page/) uses Effect's own
`effect@4.0.0` test files and links back to that upstream tag. Its October 2 run
lists 322 files and 3,454/8,592 passing cases (40.2%), using Porffor `72d048d7`
and Node 26.3.0. The published metadata says `bundled: false`. The page counts
only cases passing under its Node runner, excludes unavailable packages/files
that fail under Node, and polyfills TextEncoder/TextDecoder. Its denominator
and adapter therefore differ from our complete official Vitest inventory;
the percentages are not matched coverage. `porffor-reference.json` retains
the retrieved metadata and evidence digests.

The complete Bun 1.4.2 direct-source reference registers the same 11,019 cases:
10,936 pass, 28 fail and 55 skip across all 322 files, with no incomplete files.
Ten failures name unimplemented loader/reset capabilities; the remaining failures
retain genuine runtime, formatting, assertion or child-process differences from
the official Node run. `bun-reference.json` records the case IDs, pin, adapter
digests and full report digest. These are correctness counts, not performance
measurements. Runtime-specific skip modes are preserved and compared explicitly.

`Formatter > format > function` remains a genuine direct-source failure. Its assertion expects
tab indentation and a trailing semicolon inside an arrow function's `toString()` result. The
test source contains ten-space indentation and no semicolon; Node 26.8.1 with
`--experimental-strip-types` preserves that source text, while Bun 1.4.2 reformats it to two
spaces plus a semicolon. Kiln's direct TypeScript path also preserves the source text. The
official Node/Vitest case passes because Vite transforms the source before execution, so that
runner does not establish the direct-source `Function.prototype.toString()` expectation. Do not
change source fidelity or rewrite this assertion to make this case appear compatible; it needs
an upstream/runtime contract decision. The pinned Bun failure remains recorded in
`bun-reference.json`, and the native failure remains in
`native-diagnostic-reference.json`.

The latest complete selection on native compiler `8cb04e4f` recorded 10,785 passes,
142 failures and 45 skips. Two files stopped early: 47 registered cases were not
recorded, so this is an incomplete diagnostic run, not a complete native score.
The same run reproduced Bun's complete reference exactly. The earlier compiler
`e445e822` recorded 9,379 passes, 796 failures and 799 missing results.
`native-diagnostic-reference.json` retains compiler and report digests, source
and adapter digests, per-file completeness and failure IDs. The midway compiler
predates subsequent fixes to generated modules, dynamic classes, UTF-16 string
replacement, text encoding and WebCrypto. Focused results from those fixes must
not be added to this older run. A fresh complete run on one frozen compiler is
required for an updated comparison.

A separate frozen run on `e61fe6fb` passes all 46 TestSchema cases after sharing
strict graph comparison with `node:util` and fixing native own-property
enumerability. Its three-file selection still records two native RegExp codec
failures and one native property-based diff/patch failure; Bun instead fails
the File/FormData codec cases and the property-based case. The exact IDs and
report digest are retained in `native-focused-reference.json`.

Later focused runs on frozen compilers pass all 115 selected compression and
SchemaAST cases, and all 206 selected Crypto and SchemaBinary cases, under both
native and Bun. Generated-schema tests pass 14 of 15 under both runtimes; native
still misses the linked child runner's completion output, while Bun's child fails
in its internal stream initialization. Matching outcome counts do not resolve
those different failures. `native-focused-reference.json` retains the separate
compiler identities, source/adapter/report digests and failing case evidence.
These subsets are not an updated whole-suite score.

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
