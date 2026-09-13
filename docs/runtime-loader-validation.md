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

- Log: `~/Developer/Personal/kiln-ghui-host-capability-1572/.scratch/r14-r15-validation-final.log`
- r15 output was checked against Node 25.9.
- The r14 `source-file-type` case preserves its `TypeError` contract.

The conformance crate check passed 68 tests with no failures. Runtime-loader changes were reviewed
as a focused validation; this record does not claim a new full-gate run.

## GHUI live acceptance

The latest clean compiler revision was `6c3dff62`, with compiler SHA-256
`fd7ecb9dd263e3a7a4bff7c3f4a2b65b5c374c77a685ebecf31db9aac6aa3d6c`. The GHUI binary SHA-256 was
`e020f2b3817746b6b5d25647e6d90a53c2276485020e1b93531a72b98e6f9472`, size 116,275,880 bytes, at
`app/.scratch/ghui-live/ghui-6c3dff62`. Build log: `.scratch/ghui-6c3dff62-build.log` (emit
9,629 ms, compile 38,354 ms, link 11,796 ms).

The live transcript `.scratch/ghui-live/ghui-6c3dff62-live.log` passed real HOME, repository
navigation, PR 50 and issues 15, PR 3 detail view with markdown scrolling, Escape navigation,
`q` terminal restoration, and exit 0. No GitHub mutations or observed timeout occurred.

## Additional evidence and limits

The graph and builtin focused logs are `.scratch/ghui-final-graph.log` and
`.scratch/ghui-final-builtins.log`. Native focused GC-stress and real-FFI dlopen/close checks
passed; the full native run before the FFI additions recorded 205 passed, 0 failed, and 2
ignored. Workspace follow-up recorded 1,382 passed across 73 targets, then the facts-only check
completed 31 targets cleanly. Backend checks recorded 131 unit and 36 emitted tests passing.

Third-party validation recorded 17 interpreted matches and 17 native matches, with 0 refused, 0
diverged, and 5 absent in `.scratch/ghui-final-thirdparty.log`.

Native parity, performance comparison, final rebuild, and push validation remain pending. These
records do not claim `mise run gate` is clean.
