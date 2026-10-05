# Combined allocator and module-generation integration gate

`mise run gate-full` passed all ten steps in 1,307 seconds in the isolated
allocator milestone worktree. The eight-CPU budget assigned two crate-test
workers, two native workers, three Test262 workers with a 4 GiB cap, and one
third-party preparation worker. Benchmark execution waited at the gate barrier
until correctness work finished.

- Crate tests: 110 suites, 2,635 passed, zero failed, nine ignored.
- Native parity: 645 matched, five named refusals, zero divergences; 650 probes.
- Full Test262: 102,638 selected, zero deferred, verified partition; 97,683
  passed, 4,565 failed, 386 unsupported, four skipped. No regression across
  122 chunk baselines. These baseline failures remain explicit.
- Third-party correctness: all 17 present bundles matched under both engines;
  five absent bundles were reported. Native had zero refusals or divergences.
- All 13 present unchanged performance baselines passed, including PostCSS.
- Node: 40 passed. Server: seven passed, two skipped. Regression corpus: 57
  passed. Facts: all 31 targets passed.

The aggregate, every top-level envelope/result document and full raw stream are
retained here. `reference.json` records their hashes and the source/CLI identities;
`candidate.json` records the owned frozen CLI. Original per-chunk and per-batch
Test262 evidence remains in the original evidence directories named by reports.
The corrected full-GUI provider and unchanged headless provider were installed
separately for capability-based discovery; `provider-installation.json` identifies
the corrected files. No global provider override narrowed GUI coverage.

Two normal collector sweeps bounded only the gate worktree's own kiln cache
during correctness lanes. They preserved grace protection and compile-lock files;
their logs are retained in `../resource-coordination-20261004/http-fixed-gate/`.
Automatic shared-target mbx pruning remained disabled.

This is a passing compiler integration milestone. It does not prove completion of
the upstream 322-file Effect corpus, a complete corrected allocation-stress run,
or the final 22-microbenchmark and four direct-TypeScript comparison against Bun.
