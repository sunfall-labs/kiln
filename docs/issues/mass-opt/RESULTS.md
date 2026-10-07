# Validation optimization measurements

Work starts at `e7591c17` in the owned `perf/mass-opt` worktree. The first integrated
native change is `90eea4ea`; copied binaries, full raw logs, and selected machine-readable
measurements are retained under this directory. Binary hashes are in the JSON records.

| Measurement | Before | After | Interpretation |
| --- | ---: | ---: | --- |
| Native crate execution, 8 test threads | 119.69 s | 29.68 s | Observed reduction only: native artifact cache warmth was not controlled; 219 passed, 0 failed, 2 ignored in both |
| Native crate command, including compilation | 148.83 s | 52.83 s | Build and native artifact cache conditions differ; not an attributable speedup |
| Warm native daemon build, median of four requests | 174.5 ms | 60.5 ms | 65.3% less time; see ../mass-bootstrap/measurements.json |
| Warm native corpus, 8 workers | 80.89 s | 82.17 s | No demonstrated gain; 245 matched, 1 refused, 0 diverged in both |
| Sparse array method Test262 case time, 1,290 variants | 3.656 s | 2.080 s | Same IDs and outcomes; see ../extended-sparse-array/evidence.md |

The initial baseline corpus ran in 82.45 s with a partially populated cache. The first
candidate corpus ran in 208.40 s while populating changed generated-C and provider identities.
It passed with the same counts. Neither is a controlled cold-cache comparison. The later warm
pair is the relevant corpus result. Native crate tests repeatedly compile in one process;
the corpus starts a fresh process per probe, so bootstrap code reuse benefits them differently.

A diagnostic corpus run wrapped each CLI invocation in GNU `time` and retained wall/user/system
times in `native-process-times.log`. Those are profiling data, not a benchmark comparison.

Rejected experiments: dev/check prelude consolidation produced the same actual prelude payloads
on the probes, offering insufficient gain for a policy change. String equality/inlining improved
a six-file Unicode sample by only about 1% across A/B and B/A pairs, with mixed results on a
non-whitespace sweep; it was reverted. No coverage, iteration count, baseline, or execution
budget has been reduced by this work.

ADR alignment: immutable bootstrap IR and RegExp compiled code are shared, while each realm
retains fresh mutable objects (0071). SHA-256 output and full-width cache identity are unchanged;
0010 records the measured reason for using the already pinned maintained implementation.
The prototype guard reads current realm state on every sparse-array iteration. No builtins
were added to the frozen C runtime, and no per-object property map was introduced.

Integrated commit `1c52189e` includes upstream `050c6c39` (provider ABI 43).
The full gate passed in 585 s: 2,074 crate tests passed (9 ignored), native parity
248 matched / 1 refused / 0 diverged, and all 102,638 Test262 variants selected
with zero deferred and no chunk regressions. Test262 completed in 160 s;
crate tests took 530 s including compilation and native parity took 450 s.
Evidence: `final-gate-evidence/run.MPbRaaOm/aggregate.json` and full adjacent logs.

A subsequent warm quick gate on the same tracked snapshot and binary passed in
183 s: crate tests 181 s, native parity 104 s, thirdparty 3 s, build 2 s.
Evidence: `warm-quick-before/run.WGVsVYsH/aggregate.json`.
This is the baseline for further test fixture artifact reuse. It is not a
comparison with the full gate, which includes additional coverage and cold builds.

Other controlled measurements: the non-whitespace RegExp sweep improved 4.3x
(`../regexp-code-cache-measurement.md`), the Proxy own-keys case reduced summed
case time 42.6% (`../ownkeys-linear/evidence.md`), and a one-source CLI fixture
reduced peak threads from 15 to 3 (`../native-single-item-pools-measurement.md`).
No CLI latency improvement is claimed from that thread reduction.

The resource watcher ran after the warm quick gate and identified disposable
records in this worktree's `.kiln/cache`; GC evicted 1,871 records and reclaimed
8.7 GiB. Shared mbx outputs, provider archives,
fixture caches and measurement evidence were preserved. Further measurements
must identify cache warmth explicitly.


## Final fixture and resource improvements

Four CLI canonical HTTP run/build tests now link their fixture-local artifact cache to
`target/kiln-cli-native-semantic-cache`. The remaining fixtures keep private caches;
all program executions and output assertions still run. Cache setup falls back to the
private cache if symlinking is unavailable. No global environment is mutated by tests.

On the same worktree, Clang environment, four test threads and `KILN_JOBS=4` /
`RAYON_NUM_THREADS=4`, `mbx test -p kiln-cli --locked --jobs=4 one_shot_ --
--test-threads=4` selected the same 14 unit tests and one matching integration test:

| Fixture configuration | Unit execution | Result |
| --- | ---: | --- |
| Before, original private caches | 20.71 s | 14 passed |
| After, first shared-cache population | 18.62 s | 14 passed |
| After, populated artifact cache | 11.07 s | 14 passed |

The repeated edit-loop run reduced this group by 46.5%; the first population saved
10.1%. These are one sample per condition, not a statistical confidence interval.
The integration test passed in 0.02 s in every run. Raw logs are
`cli-fixture-before.log`, `cli-fixture-after-first.log`, and
`cli-fixture-after-warm.log`. The new cache occupied 172 MiB. Baseline commit:
`14d7c3b0`; candidate: `95e65b7c`. Both runs reused the same provider/toolchain artifacts.

Private source-read and native-emission pools previously ignored explicit worker limits
and could start eight/four workers even when the caller requested one. `78487236`
now clamps each pool to the positive caller limits as well as CPU count and its existing
cap. Pure sizing tests cover competing limits, invalid/zero values and CPU bounds;
no test changes process-global environment variables. This is a resource-limit fix,
not a claim that all independent pools/processes share one global allowance.


## Routine gate before landing integration

At `5cca720b`, `KILN_GATE_JOBS=16 mise run gate` passed all ten steps in 149 s
(150 s wrapper), with the Test262 memory cap at 8 GiB. Workspace tests:
2,078 passed / 0 failed / 9 ignored across 88 suites. Native parity:
248 matched / 1 refused / 0 diverged in 87 s. Routine Test262 selected 101,640
variants and explicitly deferred 998; 96,131 passed, with no regressions in 122
chunks, in 60 s. Thirdparty, Node/server/regress corpora and performance/facts
baselines passed. Evidence: `final-routine-evidence/run.KJh3HJYU/aggregate.json`.

The watcher observed at least 27 GiB available RAM, no swap use and continued
progress during these bounded checks. No concurrent agent build was allowed.
This establishes a routine loop under three minutes for this snapshot; it does
not claim the cold full gate or every future change is equally fast.

Before landing, main advanced to `83ee707d` with built-in consolidation and shared
native realm changes. Merge `f693cd91` integrates those changes. The updated ADR
0071 slot-publication amendment remains authoritative; this optimization does not
alter its shared native slot storage/publication. A separate integration gate is
required for this combined tree; prior measurements above retain their own commits.


The first merged-tree gate (`cbaae10f`) was capped at 300 s and stopped with
exit 124; **it did not pass**. Routine Test262 completed in 62 s with 96,141
passes and no chunk regressions. Workspace validation stopped at
`facts_baseline_check_passes`: upstream `512c8781` added a checksum callback to
`bench/harness.mjs` without updating its function inventory. The failure named
14 benchmark targets with the harness's function count changing from 16 to 17.
Full evidence: `merged-routine-evidence/run.e7gslvx1/`; all child processes ended
at the timeout, confirmed by the resource watcher.

This cold run rebuilt Rust test artifacts, the new provider archive and native
objects after the upstream runtime merge. It does not meet the desired cold-run
latency. Three simultaneous provider Cargo requests were observed, but only one
expensive provider rustc; Cargo's lock was already preventing duplicate compilation
there. Native C object misses have no analogous production coalescing: each
caller compiles before the cache's race-safe publication. That remaining repeated
work is being addressed without changing cache identity or test coverage.


`6f4d15d8` deliberately refreshes that facts inventory. Review compared the JSON
structurally: only the harness module's `functions` entries changed in the 14
benchmark records; all workload modules and all 17 bundle records are identical.
The focused facts check passes across all 31 targets.

A representative native-realms check of `method-inline-cache.js`,
`prototype-mutation-paths.js` and `regexps.ts` passed with 1, 2 and 4 realms per
process: 3 matched / 0 skipped / 0 diverged. It ran with one build worker and a
120 s cap; `merged-realms.log` retains the result. This is a named subset, not
a full native-realms or collector-stress run.


## Concurrent cold object compilation

Native object cache misses now take a bounded per-cache advisory lock, recheck the
full key, and compile only if the object is still missing. Locks are released on
error and process exit; warm and read-only hits bypass locking. The cache keeps
its existing verification and corruption checks. At most 256 lock files are used
per writable layer, outside the records swept by cache GC. Process mutexes hash
the canonical lock-file path so independent caches do not always serialize.

Controlled measurement: four concurrent CLI processes, each with one worker,
compile and execute the same two-module program against a fresh shared artifact
cache. All 16 executions across A/B/B/A returned exactly `42`. Both versions use
the same Clang wrapper, which records `-c` invocations and delegates unchanged to
Clang 18. Provider archives were already built; only native artifact caches start
empty. This measures concurrent build requests, not JavaScript execution speed.

| Run | C compilations | Wall time | Summed child CPU |
| --- | ---: | ---: | ---: |
| Before A | 21 | 8.294 s | not recorded |
| After B | 6 | 7.390 s | not recorded |
| After B | 6 | 7.479 s | 8.77 s |
| Before A | 24 | 11.498 s | 29.67 s |

The implementation removes 71–75% of compiler invocations in this example.
The CPU-instrumented reverse pair used 70.4% less child CPU. Wall-time gains are
smaller and variable (11–35% across the pairs); this is not a whole-gate speedup
claim. Every candidate source compiled once; controls compiled the prelude,
runtime and launcher four times each. Reproduction script and exact binary
hashes/results: `object-miss/measure.py`, `object-miss/{before,after,after2,before2}/result.json`.
Baseline production code is `cbaae10f`; candidate code is `8585cf71`.

Focused checks: all three native-object tests passed in 0.93 s, covering concurrent
misses, read-only hits and retry after compiler failure. The release CLI build
passed. One initial test-only byte-slice type error was corrected before any test
was claimed as passing; both logs are retained.

The combined routine gate at `69083262` reached its 300 s deadline before the
native crate and native parity completed. This is an incomplete validation, not
a passing gate (`landing-gate-evidence/run.Y1C0X6qF/`). Routine Test262 passed
in 96 s, with 96,141 passes and no regressions across 122 chunks. The completed
CLI suite took 92.41 s; three native HTTP fixtures exceeded 60 s during cold
compilation after the upstream merge. No test failure was reported before the
timeout. The resource watcher verified that termination left no orphaned children.

The extended Test262 complement then ran separately under an 8-worker / 8 GiB
budget and a 180 s deadline. All 998 selected variants completed: 950 passed,
46 failed and 2 unsupported, with no baseline regressions across 15 chunks.
The runner verified the partition against 102,638 discovered variants;
101,640 routine variants were explicitly deferred in this invocation.
`landing-extended.json` and `landing-extended.log` retain the complete result.

Resource cleanup reclaimed 8.7 GiB through this worktree's native artifact GC,
then 322.2 MiB from the four isolated object-miss measurement caches. Measurement
JSON, compiler logs, output and before/after binaries remain. Shared mbx targets,
provider archives and other worktrees' outputs were preserved. During these runs
the watcher reported about 29 GiB available RAM, 766 GiB free disk and 76 MiB
swap in use, without an OOM event. Cold gate latency remains unresolved.

The isolated native crate then completed in 86.60 s: 227 unit tests and four
integration tests passed, with two ignored unit tests. This follows partial cold
gate runs and is not a controlled before/after timing. Read-only review confirmed
that native fixtures already share the persistent test cache and that prelude
variants depend on demand/profile, not temporary fixture paths. The CLI's fifth
output-only native HTTP fixture now uses the existing shared fixture artifact
cache too; its fresh project and output/exit assertions remain unchanged.

## Completed routine validation and the host I/O bottleneck

The next 16-worker gate also timed out at 300 s. It completed native parity
(262 matched / 1 refused / 0 diverged) in 271 s, and routine Test262 in 54 s;
workspace tests were incomplete. The serial diagnostic completed all 2,097
workspace tests in 83 s and routine Test262 in 55 s, but hit the total 300 s
deadline during native parity. Neither invocation is a passing gate.

The watcher found 5–6 blocked tasks and 16–19% I/O wait during the overlapping
native workloads, despite ample RAM and disk space. An isolated two-worker native
corpus passed in 90.68 s with identical 262/1/0 results. Its sampled blocked queue
was zero and I/O wait 1–4%. A small filesystem probe and the shape-key GC probe
were fast in isolation; skipping their assertions was not the remedy.

At `60e5bb54`, **the complete routine gate passed in 158 s** with
`KILN_GATE_JOBS=8`: two workspace workers, two native workers, four Test262
workers and a 4 GiB Test262 cap. All ten steps passed. Evidence is retained in
`bounded-gate-evidence/run.AiJDAzzZ/`; `bounded-gate-aggregate.json` is committed.

| Step | Result | Elapsed |
| --- | --- | ---: |
| Workspace | 2,097 passed, 0 failed, 9 ignored; 88 suites | 131 s |
| Native parity | 262 matched, 1 existing refusal, 0 diverged | 107 s |
| Routine Test262 | 101,640 selected; 96,141 passed; no chunk regressions | 74 s |
| Thirdparty | Passed, with absent bundles explicitly reported | 14 s |
| Node / server / regress | 36 passed / 6 passed and 2 skipped / 26 passed | 1 s combined |
| Thirdparty / facts baselines | Both passed | 26 s / 1 s |

This is a populated-cache run and a host scheduling result, not a controlled
cold-build speedup. The source snapshot is
`c62de85809c385d9f0c38d4248e0d18712c15fe2ec936e731a4245284d032a68`;
the CLI hash is `3e1d6b1bb1cfa865a5787bf2df7eeac321c59aaeb46a64bf6846eeed17653178`,
also used for the separately completed extended Test262 check above. Coverage
and the extended manifest were not changed. Higher concurrency is not a faster
default on this measured filesystem; eight gate workers are the local setting
to retain. Cold builds remain a separate, unresolved latency limit.

The next architectural candidate is bounded raw linker-input staging: current
link misses copy validated provider archives and object bytes into each private
workspace. A shared raw blob could be hard-linked into fresh workspaces, but
requires a quota, eviction coordination and integrity checks. Directly linking a
mutable provider package path would introduce a time-of-check/time-of-use bug.
No unbounded disk or memory cache was added for this proposal.

## Final upstream integration

After that completed gate, `ea4f5a00` landed upstream with typed-entry builtin
guard fallbacks. Merge `bddf7720` is clean. Review checked the latest ADR 0071:
the added guard answers are scalar/member identifiers and static code pointers,
kept per realm. Backend and prelude digests cover the changed production code;
our bootstrap cache, object locks and dedicated emitter identities remain valid.

The merged release CLI built successfully in 10.94 s. Both added Rust tests
passed (`a_speculation_asks_only_about_the_builtins_it_relies_on` and
`typed_entry_call_sites_ask_about_the_members_the_entry_inlines`), then all three
new native probes matched with `KILN_IC_CHECK=1`: 3 matched, 0 refused, 0 diverged.
Logs are `upstream-final-{build,tests,probes}.log`. This was focused integration
validation; the 158 s complete gate belongs to the preceding optimization
snapshot, not this merge. The upstream issue-20 record separately documents its
full native/backend/realm validation, so those full runs were not repeated.

The main checkout and this worktree now have an excluded `.mise.local.toml`
setting `KILN_GATE_JOBS=8`, verified through `mise env`. The repository's shared
default remains portable; this host retains the measured budget. Final watcher
checks found no orphaned build/test children, about 760 GiB free disk and
29.9 GiB available RAM, with 76 MiB swap in use. Reclaimed space attributable to
our cleanup was 8.7 GiB plus 322.2 MiB; unrelated filesystem changes are not
credited as cleanup gains.
