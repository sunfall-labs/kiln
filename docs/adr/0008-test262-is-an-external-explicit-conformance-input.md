# ADR 0008: Test262 is an external, explicitly classified conformance input

## Status

Accepted.

## Context

Design §14 makes parser and interpreter Test262 execution part of the compiler-developer loop.
The suite is large, evolves independently, and describes more execution variants than “read a
`.js` file and run it”: ordinary tests normally have sloppy and strict script variants, while
`module`, `raw`, `async`, and negative phases change either the parse goal or the expected result.

Kiln's executable language is deliberately incomplete. Treating an unimplemented feature as a
passing conformance test would make progress numbers meaningless; treating it as an ordinary
failure would make a small supported subset impossible to measure honestly. Vendoring the full
suite would also put an unrelated high-churn corpus in the compiler repository and its everyday
source walk.

## Decision

`kiln test <path>` consumes either an external Test262 checkout or a small repository-owned smoke
corpus. Kiln does not vendor the upstream suite. Discovery excludes the harness and `_FIXTURE`
files and preserves the metadata needed to choose variants and ordered includes.

Each test result is one of four explicit outcomes: `Pass`, `Fail`, `Unsupported`, or `Skipped`.
Only `Pass` is conformance success. `Unsupported` means Kiln parsed the test contract but cannot
yet implement its language or harness requirements; `Skipped` means the requested runner mode is
inapplicable. Reports retain all four categories, and CI fails only when an executed expectation
is wrong.

Ordinary tests are parsed as scripts even though Test262 names them `.js`. Their default sloppy
and strict variants are distinct; strict mode prefixes the script source. `module` selects the
module parse goal. `raw` never receives the normal harness or strict transformation. `async` and
module resolution are never silently executed with ordinary-script semantics. Negative tests
must fail in the declared parse, resolution, or runtime phase and with the declared error type;
a failure in another phase is a test failure.

The changed-test selector orders previously failing tests before smoke tests and affected tests.
Its failure state lives under the Kiln project, not in an external Test262 checkout. A daemon
request executes the selected batch in one resident process so the everyday command does not pay
one compiler startup per test.

The full upstream lane is `scripts/test262.sh`. The three script entrypoints delegate checkout,
pin, physical subtree, empty-corpus, memory-cap, and result-document policy to the shared
`scripts/test262_corpus.py` module. Managed fetches stage and validate the exact pinned commit
`771005236e88a909635104e03ba12559688c0172`, then publish it to the durable per-machine cache;
an explicitly supplied checkout is validated in place and is never removed or replaced. The
runner invokes `kiln test <checkout>/test` through the normal resident daemon path. The
machine-readable report retains the schema-1 per-variant results and adds the pin, selected path,
discovered test-file count, executed verdict count, elapsed milliseconds, and
executed-tests-per-second throughput. The chunk baseline uses the same validator and execution
policy while retaining its separate per-chunk comparison and update rules.

## Consequences

- Conformance counts remain honest while interpreter, harness, and realm coverage grow.
- Parser goal is an owned HIR property rather than an inference from a filename extension.
- The upstream suite can be updated independently and CI can pin a revision outside this repo.
- Local smoke fixtures keep the runner and protocol testable without a network checkout.
- CI's pinned external lane records full-suite throughput separately from the fast smoke and
  repository-corpus gates; unsupported behavior remains visible in its report.
- Harness includes, fresh realms, async completion, module resolution, and exact error identity
  remain visible implementation work rather than behavior approximated by the initial runner.

## Explicit batch coverage and the extended section (2026-09-26)

`kiln test <root> --list --out inventory.json` discovers file IDs and their exact realm-variant
IDs without execution. `--cases batch.json` accepts a nonempty JSON array of those file IDs;
unknown or duplicate IDs fail. These protocol requests use the same conformance coordinator as
ordinary tests, preserve original paths and harness/module behavior, and do not update the
interactive changed-test failure state. They are explicit selection, not cached verdicts.

The pinned baseline runner builds bounded batches from the inventory and checks the exact union
of returned variant IDs. Its routine and extended selections are disjoint and cover the complete
inventory. The reviewed `conformance/test262-extended.json` list selects expensive files; timing
estimates affect scheduling only. Every report declares selected, deferred and discovered counts.
The full per-chunk baseline remains authoritative for complete runs; routine passing floors are
its counts less the extended section's recorded reference passes. Baseline regeneration requires
full coverage and updates both documents deliberately. Default gate/test262 tasks run routine
coverage; gate-full/test262-full run all cases and test262-extended runs only the expensive section.
