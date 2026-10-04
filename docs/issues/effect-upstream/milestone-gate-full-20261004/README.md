# Frozen compiler milestone full gate

The complete gate at source `fda0e625` (CLI SHA-256 `1b5a2a350cdfc20ee5f3b88b9159902e2e7c0833a39ac8f3b84b14710046e548`) finished in 3,872 seconds. It is **failed**, with two failing lanes: native parity and Test262 infrastructure. This compiler milestone has not been pushed.

- Workspace: 110 suites, 2,615 passed, zero failed, nine ignored.
- Native parity: 642 matched, seven refused, one diverged (`parallel-alloc-flatmap`). The two allocating parallel probes also refuse; diagnostic reduction is required.
- Third-party correctness: passed.
- Node: 40 passed; server: seven passed, two skipped; regressions: 57 passed.
- Facts baseline: passed.
- Full Test262: selected all 102,638 variants, but the generated-eval batch aborted under the 1 GiB cap. There is no accepted complete Test262 result. The later 4 GiB failed-batch rerun retained under `docs/issues/gate-test262-memory-floor/` completed all 14 variants; that is targeted infrastructure evidence, not a passing full suite.

The release-marker lifecycle worked: all 13 benchmark bundles, including PostCSS, completed preparation in 419.43 seconds, waited 3,372.31 seconds for correctness lanes to join, then measured serially in 5.01 seconds. Extra survey/diagnostic workers had been terminated before the timing phase. All 13 rows passed the existing 25% plus 3 ms regression rule without changing the baseline. Some percentage changes exceed 25% while remaining below the absolute 3 ms floor; the raw table is retained here. This is the repository's bundle baseline check, not the outstanding Bun direct-TS microbenchmark/real-world comparison.

The raw gate evidence remains in the owned `kiln-effect-milestone-gate-full` worktree at `target/gate-logs/run.2IeXJmv4/`. The aggregate and key summary logs are retained here with their frozen source/binary identities. The long crate lane ran single-worker compilation, with 299 mbx misses on the initial test-profile build; its 3,796 seconds should not be accepted as the desired routine gate latency.
