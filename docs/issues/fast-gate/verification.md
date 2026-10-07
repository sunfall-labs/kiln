# Faster build and validation loop

**Latest:** [deferred-case optimization integration](../slow-case-opt/README.md) records the
subagent optimizations, ADR review, latest-main comparison and passing complete gate. The
[third optimization pass](third-pass.md) records the routine/extended split. Earlier evidence is history.

Measured on 2026-09-26 in the owned `kiln-fast-gate` worktree, based on main
`d2c0e686ca904d5070ea83d83f2cbe35e461bf7a`. The changes were then applied to the main
working tree. Its tracked content hash matches the final gate's source snapshot:
`5f28067b57c1b78cfbf947dc319a3de0dfae07eb25b8435132227a2c86b04b83`.

## Measurements

The final complete gate took **203.03 seconds (3:23)** on this 32-logical-CPU,
approximately 32 GiB machine, with populated build/artifact caches. Cold dependency,
provider and toolchain builds can add time.

The first measured complete run during development took 379.32 seconds (6:19).
That was an intermediate implementation, not an unchanged-main benchmark; these
measurements should not be presented as a controlled before/after comparison to main.

| Work | Earlier measurement | Final measurement |
| --- | ---: | ---: |
| Test262 baseline, all 122 chunks | 297 s | 181 s |
| Workspace tests, including compilation | 342 s | 194 s |
| Artifact-cache suite execution | about 45 s alongside native compilers | 0.79 s before them |
| Warm native build C emission | 206 ms | 79 ms |
| Warm native build, whole CLI command | 1.02 s | 0.91 s |

The native build probe was `conformance/native/message-port-transfer.mjs`, using
`kiln --one-shot --node build ... --explain`. The complete-run comparisons include
scheduling and cache-warmth differences; the retained logs are the evidence.

## Final behavior

- The complete artifact-cache suite runs before concurrent compiler filesystem work.
  Its original stress repetitions, writers, sweepers and assertions remain intact.
- Remaining workspace tests, native parity and Test262 overlap within a shared budget.
  Cargo test compilation and test threads are bounded. Performance baselines run after
  the correctness jobs finish.
- On larger budgets, Test262 runs two chunks concurrently, dividing both its workers
  and enforced memory allowance. Every chunk runs. All 122 chunks have identical case
  outcomes between the first and final complete runs.
- Thirdparty probes capture stdout, stderr and status from one execution per engine,
  and reuse that execution's Node oracle result for native comparison.
- Bootstrap C emission is cached using complete compiled IR, policy and emitter/header
  identity. Runtime execution and test verdicts are never cached by these changes.
- The test profile optimizes IR, graph and C emission at level 1 while preserving debug
  assertions and overflow checks. Ordinary dev builds retain their existing settings.
- Gate stages, Test262 chunks and individual conformance cases retain elapsed times.

## Validation and existing failures

- 2,039 crate tests passed, zero failed, nine ignored.
- Native parity: 243 matched, one existing refusal, zero diverged.
- Thirdparty: 17 interpreted and 17 native bundles matched, zero diverged; five bundles
  were not built, as reported by the existing corpus.
- Node, server and regress corpora passed their runnable cases. Performance and facts
  baselines passed.
- Focused cache invalidation, bootstrap-cache, runner stream/status, resource-budget,
  parallel-chunk memory/failure propagation, result-envelope and script-hygiene checks
  passed.

**The gate still exits nonzero for seven existing Test262 baseline regressions.**
ArrayBuffer, Atomics, DataView, TypedArray, TypedArrayConstructors, Uint8Array and
staging/sm were rerun using a freshly built, unchanged `d2c0e686` binary. Every case
outcome in those seven chunks matched the changed binary. The baseline was not lowered.
Final totals: 96,266 passed, 5,986 failed, 382 unsupported and four skipped, across
102,638 cases. The baseline intentionally tracks the engine's existing failures.

## Reproduction and evidence

Validation used the machine's existing cached Clang 18 installation. Its default GCC
rejects the repository's existing Clang-only native-release flags; this toolchain
configuration predates the changes.

```sh
CC=~/.cache/kiln/clang-18/usr/bin/clang-18 \
LD_LIBRARY_PATH=~/.cache/kiln/clang-18/usr/lib/x86_64-linux-gnu:~/.cache/kiln/clang-18/usr/lib/llvm-18/lib \
mise run gate
```

- [Final aggregate](~/Developer/kiln-fast-gate/target/gate-logs/run.Amk5ojEO/aggregate.json)
- [Complete timed output](~/Developer/kiln-fast-gate/target/verified-gate.log)
- [Final Test262 case documents](~/Developer/kiln-fast-gate/target/test262-baseline-logs/run.pQlPaQmX)
- [Unchanged-main regression comparison](~/Developer/kiln-fast-gate/target/main-regression-check/report.json)

`KILN_GATE_JOBS` can lower the shared budget, and `KILN_GATE_SERIAL=1` runs the checks
serially for diagnosis. Explicit Test262 memory limits remain respected.
