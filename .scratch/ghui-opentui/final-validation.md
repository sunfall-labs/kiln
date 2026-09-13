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
- Transcript: `.scratch/ghui-live/6c3dff62-live.log`

The transcript passed real HOME, repository navigation, PR count 50+, issues 15, PR 3 details, markdown
scrolling, Escape navigation, `q` terminal restoration, and exit 0. No GitHub mutations or
observed timeout occurred.

## Final release build

The app root for this artifact is `/Users/alee/Developer/Personal/ghui-process-capability`.

At final integration HEAD `4c32b75f`, the release CLI SHA-256 was
`a64abc98c4da896cb20369dd3b0b4e2a9aadff3b111db96f5084c56bbf7ec838`. The live-proven GHUI
artifact `app/.scratch/ghui-live/ghui-final` has SHA-256
`e020f2b3817746b6b5d25647e6d90a53c2276485020e1b93531a72b98e6f9472`. Logs: `.scratch/ghui-final-build-4c32b75f.log` (object/executable cache hits, fresh C emission 9,141 ms),
`.scratch/ghui-final-cli-build-4c32b75f.log`, and `.scratch/ghui-final-fmt-4c32b75f.log`.
The exact-HEAD regression log `.scratch/ghui-final-regress-4c32b75f.log` passed 14/14.

## Focused results

- Graph log: `.scratch/ghui-final-graph.log`
- Builtin log: `.scratch/ghui-final-builtins.log`
- Native focused GC-stress and real-FFI dlopen/close checks: passed.
- Before the final callable-name correction, native parity recorded 194 matches, 1 ShadowRealm
  refusal, and 1 divergence. After the correction, normal and GC-stress callable diffs are zero,
  with the direct interpreter probe passing; no full native rerun after the correction is claimed.
- Older full native run before the FFI additions: 205 passed, 0 failed, 2 ignored.
- Workspace follow-up: 73 targets, 1,382 passed; the subsequent facts-only check covered 31 targets cleanly.
- Backend: 131 unit tests and 36 emitted tests passed.
- Third-party: 17 interpreted matches, 17 native matches, 0 refused, 0 diverged, 5 absent; log
  `.scratch/ghui-final-thirdparty.log`.
- Performance: `.scratch/thirdparty-paired-final.log`, two warmups and five interleaved passes with
  equal outputs; median ms baseline/candidate: immer 4.592/4.315, ms 4.264/4.304, rxjs 21.532/21.308,
  uuid 4.894/4.506. This is launcher-inclusive short evidence, not a steady-state benchmark.
- r14/r15 regressions: 14 passed, including the r14 `source-file-type TypeError` contract and
  Node 25.9-verified r15 output; log
  `/Users/alee/Developer/Personal/kiln-ghui-host-capability-1572/.scratch/r14-r15-validation-final.log`.

## Scope and pending work

The loader retains closed-world literal provenance, importer-bound `createRequire` identity, lazy
TypeScript builtin dependencies, canonical graph values, realm-local failures, and early dynamic
import file-attribute validation. Computed or unsupported requests remain catchable runtime
failures.

The final performance comparison completed with two warmups and five interleaved passes; it is a
launcher-inclusive short comparison, not a steady-state benchmark. The full gate was run earlier;
subsequent failures were addressed with targeted checks, without a second full-gate run. Validation is
complete; landing/push is not recorded here.
