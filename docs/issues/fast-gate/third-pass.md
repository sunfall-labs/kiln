# Test262 batches and an explicit extended section

Implemented 2026-09-26 in the owned `kiln-fast-gate` worktree, then applied to the
main working tree. Both have tracked source snapshot
`94d28f0d88b7c489a1b44329a622c93ed13cf868d4110831fb08363ea10903b5`.

## Commands and coverage

- `mise run gate`: routine gate, including 101,634 Test262 variants.
- `mise run gate-full`: complete gate, including all 102,638 variants.
- `mise run test262-extended`: only the 502 deferred files / 1,004 variants.
- `mise run test262-full`: complete Test262 coverage.
- `mise run test262-update`: deliberate full-coverage baseline update.

[Classification and policy](~/Developer/kiln/conformance/test262-extended.md)
and the adjacent JSON manifest record the split. The original full baseline was not
lowered. Inventory fingerprints cover all 122 chunks. Every batch must return exactly
its expected variant IDs, and the union must cover the selected section exactly once.
Reports carry selected/deferred/discovered counts. Baseline updates require full coverage.

The scheduler distributes bounded file batches across three processes with five workers
and a 5,327 MiB memory cap each on this machine's gate budget. Long estimated batches run
first; estimates never select tests. Original paths, module fixtures, harnesses and fresh
realms are preserved. Result execution is not cached. Explicit batches do not overwrite
the interactive failure history.

## Measured results

Previous Test262 baseline: **186 seconds**. Final full Test262: **141 seconds**, with
all **102,638 individual outcomes identical** to the previous unsharded implementation.
Final routine Test262: **33 seconds**, with all **101,634 outcomes identical** to the
corresponding subset of that full run. An earlier routine run took 28 seconds.

The earlier cache-warm routine gate took 114.36 seconds (1:54), with 2,045 crate tests
passing and nine ignored. That run preceded the two reliability fixes below, and one
Test262 template-identity case failed intermittently. It is timing evidence, not the
final correctness result. The final full gate took 148.02 seconds, but its workspace
suite aborted on a pre-existing randomized numeric mismatch, so that whole-gate time
is not a successful full-workspace performance measurement.

Times are observations on this 32-logical-CPU machine with populated native caches;
compiler cache state and concurrent host work vary. The final routine run overlapped
a narrow unchanged-main reproduction that rebuilt dependencies in another worktree.

The **final routine gate took 191.49 seconds (3:11)**, including **75 seconds rebuilding
Rust test artifacts**. All **2,045 crate tests passed**, zero failed and nine ignored.
Native parity, thirdparty, Node/server/regress and performance/facts checks passed. Its
only failing step was the seven existing Test262 baseline regressions. Routine totals
were 95,316 passed, 5,934 failed, 380 unsupported and four skipped. The passing randomized
run does not invalidate the separate deterministic native-bug reproduction.

## Are the deferred tests inherently slow?

- **448 files (89%)** are exhaustive Unicode/RegExp work. These have large inputs,
  but a representative real Unicode-builder probe returned the same result in Node
  and Kiln in median 56 ms versus 1.11 s (three runs each). Those are whole-process
  triage times, including startup, not steady-state benchmarks or proof about all 448.
- **Six files** hit the eight-million-step execution limit: three TypedArray, two Map,
  one Set. The Map/Set cases expect an overridden insertion method to throw promptly;
  consuming their infinite iterators is a correctness issue.
- **48 files** have mixed workloads and need individual profiling. Their cost has
  not been established as inherent.

No defensible exact inherent-versus-avoidable count is available from timing alone.
The split is a cost policy and an optimization backlog, not a declaration that 502
files are irreducibly slow. Keep extended runs for affected features and periodic or
release validation, and promote optimized cases back into routine coverage.

## Reliability fixes and remaining failures

A new real-eval regression reproduced template-object identity reuse before the fix.
The cache used a program address after the program was freed; later eval allocations
could reuse that address. A weak owner now reserves allocation identity for the cache
entry's lifetime without retaining the program's code. The test passes after the fix.

The watcher test now publishes its fixture edits with write-then-rename. Its previous
truncate/write sequence could expose an actual empty-file intermediate edit during
concurrent load. Assertions and production watcher behavior are unchanged.

The full and routine Test262 reports still fail the same seven previously reproduced
baseline regressions: ArrayBuffer, Atomics, DataView, TypedArray, TypedArrayConstructors,
Uint8Array and staging/sm. Baselines and execution limits were not weakened.

The randomized native test also found a remainder-by-zero mismatch. A deterministic
three-element reproduction fails on unchanged main `d2c0e686`. It remains enabled in
the routine gate; [the separate issue and reproducer](issues/01-native-remainder-parity.md)
record the evidence and acceptance criteria.

## Evidence

- [Full gate aggregate](~/Developer/kiln-fast-gate/target/gate-logs/run.0yLYlGBs/aggregate.json)
- [Full every-case comparison](~/Developer/kiln-fast-gate/target/sharded-full-final-comparison.json)
- [Routine gate aggregate](~/Developer/kiln-fast-gate/target/gate-logs/run.4hKeIKoJ/aggregate.json)
- [Routine every-case comparison](~/Developer/kiln-fast-gate/target/sharded-routine-final-comparison.json)
- [Full timed log](~/Developer/kiln-fast-gate/target/sharded-full-final.log)
- [Routine timed log](~/Developer/kiln-fast-gate/target/sharded-routine-final.log)
- [Before template fix](~/Developer/kiln-fast-gate/target/template-identity-before.log)
- [After template fix](~/Developer/kiln-fast-gate/target/template-identity-after.log)
- [Unicode comparison](~/Developer/kiln-fast-gate/target/slow-case-node-comparison.json)

The protocol/CLI checks, explicit-batch integration test, 12 Python corpus/scheduler
checks and seven result-envelope checks passed. A pinned 728-case fixture containing
modules, tagged templates and copyWithin had matching unsharded/full/partitioned outcomes.
Validation used the same cached Clang 18 configuration recorded in the previous pass.
