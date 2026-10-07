# Matched interpreter collector cohort — 2026-10-07

This is a targeted diagnostic comparison, not whole-suite acceptance. Both runs completed the same seven original upstream files and all 562 cases, with exact official Node/Vitest mapping and zero inventory or mode errors. Both passed 537 and failed 25, all at the original five-second deadline. There were no changed case outcomes. The other 315 upstream files were deferred.

The control is the published compiler source (frozen CLI SHA-256 `b0ac1791e1ab0a26e51711dbaa8bd833aabbb8c016d6e7921d6244faa07639fb`). The candidate is the unlanded dense cycle-collector graph commit `625a9c5b37ef6147833491558a801be4e42594dd`, frozen CLI SHA-256 `4ecafc1650b2fc7df3b0c94e6ce1ee8afa7dd4879399ed7bdac8c490f43d4eb8`. It changes temporary graph indexing, not collection cadence, ownership policy, or timeout budgets. Its focused collector suite passed 29 tests; the repository gate has not yet run for this candidate, and this archive does not land it.

The candidate ran first, then the control, serially with one runtime worker and the same service CPU weight (10,000), workspace, harness, runner, original source pin, memory cap, cases, and mode policy. No team compiler or profiler ran during either timed workload. These are single trials, not repeatable isolated performance attribution; system-wide external activity was not controlled. Supervisor elapsed times were 410.09 seconds for the candidate and 445.86 seconds for the control. `matched-comparison.json` retains per-file times and exact case-outcome differences (none).

| File | Control pass/total | Candidate pass/total | Control seconds | Candidate seconds |
| --- | --- | --- | --- | --- |
| Cron | 43/43 | 43/43 | 7.13 | 7.23 |
| Hash | 24/25 | 24/25 | 43.76 | 43.46 |
| arbitrary/Arbitrary | 171/178 | 171/178 | 94.66 | 88.17 |
| cluster/ClusterWorkflowEngine | 40/47 | 40/47 | 105.39 | 88.75 |
| cluster/Sharding | 59/66 | 59/66 | 120.22 | 109.65 |
| encoding/SchemaBinary | 191/193 | 191/193 | 60.79 | 59.32 |
| schema/toDifferJsonPatch | 9/10 | 9/10 | 13.04 | 12.59 |

The terminal services and supervisor receipts both exited 1 because cases failed. Those failures are preserved, not reclassified as successful acceptance. Compressed archives retain the complete selected-file logs, envelopes, reports, job specifications, and terminal receipts. `manifest.json` records the uncompressed SHA-256 and verifies lossless compression. Executable snapshots are identified by hash but excluded from the archive. The comparison script asserts exact coverage, input identities, terminal receipts, and case inventories before producing its report.
