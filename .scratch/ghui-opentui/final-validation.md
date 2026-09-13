# Final validation record

This is the reviewed GHUI native acceptance record. It does not claim that the full repository gate
or push validation is complete.

## Clean compiler and live acceptance

- Compiler revision: `6c3dff62`
- Compiler SHA-256: `fd7ecb9dd263e3a7a4bff7c3f4a2b65b5c374c77a685ebecf31db9aac6aa3d6c`
- Binary: `app/.scratch/ghui-live/ghui-6c3dff62`
- Binary SHA-256: `e020f2b3817746b6b5d25647e6d90a53c2276485020e1b93531a72b98e6f9472`
- Binary size: 116,275,880 bytes
- Build log: `.scratch/ghui-6c3dff62-build.log` (emit 9,629 ms, compile 38,354 ms, link 11,796 ms)
- Transcript: `.scratch/ghui-live/ghui-6c3dff62-live.log`

The transcript passed real HOME, repository navigation, PR 50, issues 15, PR 3 details, markdown
scrolling, Escape navigation, `q` terminal restoration, and exit 0. No GitHub mutations or
observed timeout occurred.

## Focused results

- Graph log: `.scratch/ghui-final-graph.log`
- Builtin log: `.scratch/ghui-final-builtins.log`
- Native focused GC-stress and real-FFI dlopen/close checks: passed.
- Full native run before the FFI additions: 205 passed, 0 failed, 2 ignored.
- Workspace follow-up: 73 targets, 1,382 passed; the subsequent facts-only check covered 31 targets cleanly.
- Backend: 131 unit tests and 36 emitted tests passed.
- Third-party: 17 interpreted matches, 17 native matches, 0 refused, 0 diverged, 5 absent; log
  `.scratch/ghui-final-thirdparty.log`.
- r14/r15 regressions: 14 passed, including the r14 `source-file-type TypeError` contract and
  Node 25.9-verified r15 output; log
  `/Users/alee/Developer/Personal/kiln-ghui-host-capability-1572/.scratch/r14-r15-validation-final.log`.

## Scope and pending work

The loader retains closed-world literal provenance, importer-bound `createRequire` identity, lazy
TypeScript builtin dependencies, canonical graph values, realm-local failures, and early dynamic
import file-attribute validation. Computed or unsupported requests remain catchable runtime
failures.

Native parity, performance comparison, final rebuild, and push validation remain pending. The
focused evidence above must not be read as a clean `mise run gate` result.
