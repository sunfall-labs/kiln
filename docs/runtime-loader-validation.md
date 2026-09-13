# Runtime loader validation record

This record documents the reviewed runtime loader and the latest GHUI acceptance evidence. It is
an evidence record, not a claim that the full repository gate or push validation is complete.

## Loader behavior

Literal Node loader requests retain closed-world provenance: owner module, captured importer path,
and literal specifier. Computed or unsupported requests remain catchable runtime failures. A
`createRequire` closure captures its importer, normalizes a file URL to its `href` before the host
boundary, and resolves CommonJS targets through canonical graph cache cells.

Demand-only TypeScript builtins use the canonical graph value path and recursively materialize
retained dependencies. CommonJS cycles expose partially initialized shared exports. Thrown values
and loader failures remain realm-local. Failure lookup is owner-scoped and keyed by captured
importer plus literal specifier.

Dynamic import `type:"file"` validation is performed before graph plan reuse and rejects a source
module with the expected `TypeError`; only `ModuleSource::Asset` is accepted.

## Exact validation

The focused r14/r15 regression run used the owned worktree and passed 14/14:

- Log: `/Users/alee/Developer/Personal/kiln-ghui-host-capability-1572/.scratch/r14-r15-validation-final.log`
- r15 output was checked against Node 25.9.
- The r14 `source-file-type` case preserves its `TypeError` contract.

The conformance crate check passed 68 tests with no failures. Runtime-loader changes were reviewed
as a focused validation; this record does not claim a new full-gate run.

## GHUI live acceptance

The live-session compiler revision was `6c3dff62`, with compiler SHA-256
`fd7ecb9dd263e3a7a4bff7c3f4a2b65b5c374c77a685ebecf31db9aac6aa3d6c`. The GHUI binary SHA-256 was
`e020f2b3817746b6b5d25647e6d90a53c2276485020e1b93531a72b98e6f9472`, size 116,275,880 bytes, at
`app/.scratch/ghui-live/ghui-6c3dff62`. Build log: `.scratch/ghui-6c3dff62-build.log` (emit
9,629 ms, compile 38,354 ms, link 11,796 ms).

The live transcript `.scratch/ghui-live/6c3dff62-live.log` passed real HOME, repository
navigation, PR count 50+ and issues 15, PR 3 detail view with markdown scrolling, Escape navigation,
`q` terminal restoration, and exit 0. No GitHub mutations or observed timeout occurred.

## Final release build

The app root for this artifact is `/Users/alee/Developer/Personal/ghui-process-capability`.

At final integration HEAD `4c32b75f`, the release CLI SHA-256 was
`a64abc98c4da896cb20369dd3b0b4e2a9aadff3b111db96f5084c56bbf7ec838`. The GHUI artifact at
`app/.scratch/ghui-live/ghui-final` has SHA-256
`e020f2b3817746b6b5d25647e6d90a53c2276485020e1b93531a72b98e6f9472`, matching the live-proven
artifact. Final build logs are `.scratch/ghui-final-build-4c32b75f.log` (object/executable cache hits, fresh C emission 9,141 ms),
`.scratch/ghui-final-cli-build-4c32b75f.log`, and `.scratch/ghui-final-fmt-4c32b75f.log`. The
regression log `.scratch/ghui-final-regress-4c32b75f.log` is the exact-HEAD `mbx run -p kiln-cli --
test conformance/regress --node` run and passed 14/14.

## Additional evidence and limits

The graph and builtin focused logs are `.scratch/ghui-final-graph.log` and
`.scratch/ghui-final-builtins.log`. Native focused GC-stress and real-FFI dlopen/close checks passed. Before the final callable-name
correction, the native parity run recorded 194 matches, 1 ShadowRealm refusal, and 1 divergence;
after the correction, normal and GC-stress callable diffs are zero, with the direct interpreter probe
passing. The older full native run before the FFI additions recorded 205 passed, 0 failed, and 2
ignored; no full native rerun is claimed after the final correction. Workspace follow-up recorded 1,382 passed across 73 targets, then the facts-only check
completed 31 targets cleanly. Backend checks recorded 131 unit and 36 emitted tests passing.

Third-party validation recorded 17 interpreted matches and 17 native matches, with 0 refused, 0
diverged, and 5 absent in `.scratch/ghui-final-thirdparty.log`.

The paired short performance comparison is recorded in `.scratch/thirdparty-paired-final.log`; two
warmups and five interleaved passes produced equal outputs. Median milliseconds (baseline 047 vs
candidate 4c) were immer 4.592/4.315, ms 4.264/4.304, rxjs 21.532/21.308, and uuid 4.894/4.506.
No meaningful regression was observed; this is launcher-inclusive evidence, not a steady-state benchmark.

The full gate was run earlier; subsequent failures were addressed with targeted checks, without a
second full-gate run. Validation is complete; landing/push is not recorded here.

## Loader path consolidation

Commit `e32d5c31` removes the conformance loader's ambient `BASE` state. Each capability now owns
an optional fixture base for importer-less eval/realm/agent requests; a request's explicit importer
always takes precedence. Two loader instances can therefore run interleaved without sharing a
resolver directory, while nested imports still resolve from their captured importer.

Production's immutable graph loader and conformance's fresh filesystem loader retain their
different adapters: both call `ModuleGraph::import_plan_for_request` for the shared
asset/import-attribute policy, while conformance preserves its `NotFound`/`Syntax`/`Unsupported`
classification and production preserves graph-owned deferred failures and cache identity.

Focused validation on the consolidation worktree:

- `mbx test -p kiln-module-graph --lib`: 148 passed; log `.scratch/consolidation-module-graph.log`.
- `mbx test -p kiln-conformance --lib`: 69 passed; log `.scratch/consolidation-conformance.log`.
- `mbx run -p kiln-cli --bin kiln -- test conformance/regress --node`: 14 passed; log
  `.scratch/consolidation-regress.log`.
- Pinned Test262 dynamic-import subtree, checkout `771005236e...c0172`, run through
  `scripts/test262.sh --out .scratch/consolidation-test262-script.json --path
  language/expressions/dynamic-import`: 1,550 passed, 57 failed, 293 unsupported across 1,900
  selected variants. Full log `.scratch/consolidation-test262-script.log`; this focused corpus
  result is recorded for diagnosis and is not a green Test262 claim.

The runner's selected-path contract was then checked against the same pinned checkout and memory
cap with both the consolidation binary and the previous integration binary. Both discovered 1,900
variants and produced identical per-case outcomes: 1,550 passed, 57 failed, and 293 unsupported.
Candidate log/report: `.scratch/test262-candidate-wrapper.log` and `.scratch/test262-candidate.json`;
comparison log/report: `.scratch/test262-baseline-wrapper.log` and `.scratch/test262-baseline.json`.
The report records `path: "language/expressions/dynamic-import"`; empty roots and paths outside
the physical `test/` tree are rejected before invoking Kiln.
