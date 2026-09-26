# 11. Benchmarks on realistic workloads

Status: ready-for-agent
Blocked by: 04 (then extended at each tier)

Workloads under `bench/` through the existing harness (`bench/harness.mjs`), warm, following
`CLAUDE.md`'s rules:

- numeric: vector maths over a `Float64Array` (today's tier);
- data transformation: map a million row objects to enriched rows (tier 3);
- text: parse and normalise a large CSV's lines (tier 4);
- filtering and sorting a large dataset (the family).

Each is measured as `map` under Node, `map` under Bun, `map` under kiln, `parallelMap` under kiln
at 1, 4 and all cores, and, for context, the same job split across Node `worker_threads`.
Checksums are compared; a differing answer is excluded and named.

## Acceptance

- One command reproduces the table; results and machine details are kept beside it.
