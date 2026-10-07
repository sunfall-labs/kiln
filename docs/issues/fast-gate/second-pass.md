# Second build-loop optimization pass

Measured 2026-09-26 in `~/Developer/kiln-fast-gate`, then applied to the main
working tree. Tracked source snapshot:
`27718bf25c281ee4106b6e0aa6fb54fc7f8ad6e997c2216caf49fc4041a2be82`.

## Results

| Work | Previous final gate | This final gate |
| --- | ---: | ---: |
| Full gate | 203.03 s | 192.71 s |
| Workspace tests, including compilation | 194 s | 125 s |
| Backend C unit suite | 108.73 s | 16.32 s |
| Native parity | 117 s | 41 s |
| Test262 baseline | 181 s | 186 s |

These are complete runs in the same owned worktree, with the same 32-CPU gate budget,
Clang 18 configuration and populated caches. Compiler cache populations and host load
can differ. The whole-gate reduction is about 5%; the large component savings do not
add together because the stages overlap. Test262 is now the critical path.

An alternating three-pair isolated probe using the real Test262 Unicode string builder
returned identical `181 2162507` checksums. Median wall time fell from 1.2073 s to
1.1274 s (6.6%). The String.repeat subtree completed in 143 ms with all 32 outcomes
unchanged. The fixture suite also completed in 9.08 s in isolation with warm objects;
its first cache-populating run was 11.12 s using Clang. Those isolated figures are not
controlled comparisons against the old GCC-based helper.

## Changes retained

- C unit fixtures use the existing artifact cache and honor `CC`. Each run preprocesses
  source, keys objects by compiler identity/flags/preprocessed bytes, relinks against
  current host libraries and executes. No execution results are cached.
- Test-only backend and IR modules no longer invalidate the production emitter cache.
  Production source changes still do. A test exercises the real build fingerprint.
- Numeric array assignments avoid decimal/UTF-16 allocation and reparsing, sharing the
  existing indexed-write implementation and retaining descriptor checks and key boundaries.
- Repeating an empty string no longer loops over the repeat count. Receiver/count
  coercion and argument validation still happen through the existing dispatch path.

A proposed change to keep indexed appends marked dense was withdrawn: full Test262
exposed a pre-existing dense-array path that mishandles length changes during copyWithin
argument coercion. The final patch preserves the previous density policy. The resulting
source was rebuilt and the complete gate rerun.

## Validation

- 2,043 crate tests passed, zero failed, nine ignored.
- Native: 243 matched, one existing refusal, zero diverged.
- Thirdparty: 17 interpreted and 17 native bundles matched; five not built as before.
- Node/server/regress and performance/facts checks passed.
- All 102,638 Test262 case outcomes across all 122 chunks are identical to the previous
  final gate: 96,266 pass, 5,986 fail, 382 unsupported, four skipped.
- The gate still exits nonzero for the same seven existing Test262 baseline regressions
  already reproduced on unchanged main. No baselines were lowered.
- Cache invalidation/execution tests pass with both GCC and Clang. Interpreter tests pass.
  Clippy on both modified crates succeeds with existing warnings. Workspace formatting
  check reports an existing difference in `crates/host/kiln-runtime/src/http_engine.rs`;
  changed code was formatted, and `git diff --check` passes.

Evidence:
- [Gate aggregate](~/Developer/kiln-fast-gate/target/gate-logs/run.VjHQCC3k/aggregate.json)
- [Full timed log](~/Developer/kiln-fast-gate/target/moonshot-gate-final.log)
- [Every-case comparison](~/Developer/kiln-fast-gate/target/moonshot-outcome-comparison.json)
- [Alternating probe measurements](~/Developer/kiln-fast-gate/target/unicode-controlled-final.json)

## Next substantial opportunity: distribute cases instead of whole chunks

Not implemented or benchmarked in this pass. The final RegExp chunk took 178.72 s;
TypedArray took 60.55 s and staging/sm 46.28 s. Each process receives seven workers,
and the scheduler cannot redistribute that capacity while another chunk remains busy.

A concrete next design is explicit deterministic case sharding in the conformance
request, scheduled as bounded batches across one shared CPU/memory allowance. Preserve
original corpus paths, fresh realms, all strict/sloppy variants and module fixtures.
Aggregate the batches back into the existing per-chunk baselines. Require a discovered
case manifest and verify the union has every expected ID exactly once; a missing batch,
crash, duplicate ID or zero-case shard must fail closed. Keep durable raw logs and
per-case results. Start long batches first using prior timing only as a scheduling hint.

Validate sharded versus unsharded outcomes on small fixtures (including failed tests,
modules and harness includes), memory-cap failure propagation and the complete pinned
corpus before claiming a speedup. A sub-two-minute gate is a target for that work, not
an established result.
