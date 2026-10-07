# 11. Benchmarks on realistic workloads

Status: resolved
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

## Comments

2026-09-27, branch `work/parallel-bench` (suite commit `e55bde95`). The Status field is left
unchanged: the suite is meant to grow as each tier lands.

**What exists.** `bench/parallel/run.sh` (and `--quick`) reproduces the table in about 7 minutes
(about 2 with `--quick`). It writes `bench/parallel/results-<date>.md` with the machine details,
the load average and `kiln build --explain-parallel` for every program. Raw records go to
`target/bench-parallel/`. See `bench/parallel/README.md`. The ten workloads, each over 1M
elements:

- typed `map` (light and heavy), `filter`, `reduce` (sum and a heavier fold), and `sort`;
- a number-array `sort((a, b) => a - b)`;
- three map -> filter -> reduce pipelines: one column (every stage tier 1), records held as
  typed-array columns, and records as objects. The last two have a tier-2 first stage.

The columns are node, bun, a node `worker_threads` pool (shared inputs, fused loops), Go
goroutines, kiln's sequential method, and kiln's parallel method at 1, 4 and 16 threads. Every run
is warm through `bench/harness.mjs`, with checksums compared at 12 significant digits. The CSV
text workload and the "enrich row objects" workload from this issue's list are rows in "Waiting on
tiers 2-4" (not measured yet), beside key-comparator sort and flatMap into arrays. This issue's
"filtering and sorting a large dataset" is the filter and sort rows plus the pipelines.

**First baseline** (Ryzen 7 5800H, 16 threads, load 7-21 with other agents' builds running, so
nearly every cell is `~`; read ratios, not milliseconds). Milliseconds per call, median of 3
rounds:

| workload | node | bun | node pool (16) | Go (16) | kiln seq | kiln par @1 | @4 | @16 |
|---|---|---|---|---|---|---|---|---|
| map light | 23.0 | 23.5 | 2.88 | 3.88 | 483 | 3.06 | 1.72 | 1.47 |
| map heavy | 66.0 | 56.0 | 8.00 | 8.86 | 557 | 33.5 | 10.5 | 7.00 |
| filter | 57.0 | 33.5 | 6.62 | 6.63 | 478 | 20.2 | 8.75 | 6.88 |
| reduce sum | 21.0 | 17.5 | 1.28 | 0.604 | 389 | 1.66 | 1.02 | 0.781 |
| reduce fold | 179 | 150 | 17.2 | 6.42 | 436 | 52.0 | 17.8 | 9.00 |
| sort Float64Array | 116 | 83.0 | 30.0 | 23.7 | 5,301 | 79.0 | 31.0 | 29.0 |
| sort array `a - b` | 1,264 | 333 | 30.0 | 41.3 | 275 | 52.0 | 35.0 | 30.0 |
| pipeline, one column | 109 | 51.0 | 1.69 | 1.23 | 1,208 | 13.0 | 8.75 | 7.12 |
| pipeline, typed records (partial) | 84.0 | 50.0 | 2.50 | 1.78 | 1,104 | 53.0 | 52.0 | 52.0 |
| pipeline, object records (partial) | 55.0 | 31.0 | 7.25 | 2.09 | 101 | 137 | 136 | 113 |

No divergences. All 16 sites decided as expected: 14 parallel, and the two record pipelines'
`parallelMap` sequential with `reads-property`.

**Findings for other issues.**

1. **The pooled filter and reduce decline on an array returned by `map` over objects, and the
   fallback is far slower than the sequential method.** `pipeline-objects` runs 0.9x of `map`. In
   isolation, 1M elements: `parallelReduce` 137 ms against `reduce` 2.3 ms, and `parallelFilter`
   37 ms against `filter` 7.8 ms. The times are the same at 1 and 16 threads, so the TypeScript
   sequential definition is what runs. The repro is below. The same values pushed in a loop run
   pooled (0.8 against 0.4 ms at 400k). This breaks the PRD's "never slower than `map`" (issue
   04/10 territory).

   ```js
   const recs = [];
   for (let i = 0; i < n; i++) recs.push({ p: (i % 1000) + 0.5 });
   const xs = recs.map((r) => r.p * 2);
   xs.parallelReduce((a, v) => a + v, (a, b) => a + b, 0);   // 460 ms at 1M
   xs.reduce((a, v) => a + v, 0);                          // 14 ms
   ```

2. **kiln's sequential typed-array methods are 10-45x slower than node's.** `map`, `filter` and
   `reduce` take 400-560 ms per 1M, the default `sort` 5.3 s, and `slice` ~200 ms (against
   `new Float64Array(xs)` 15 ms). The pooled worker on one thread (`@1`) is 5-300x faster than
   kiln's own `map`, so "vs kiln seq" overstates what the cores buy. `@1` against node is the fair
   single-core comparison, and the parallel method wins it on every numeric row except
   `sort-typed`. The sequential method matters because it is also the fallback for every site
   that does not qualify.
3. `@4` to `@16` scales poorly on the memory-bound rows (map-light 1.72 -> 1.47, sort 31 -> 29).
   Part of that is the loaded machine. The compute-bound rows scale (reduce fold 52 -> 17.8 ->
   9.0; map heavy 33.5 -> 10.5 -> 7.0).
4. The pool and Go lead the pipeline rows mostly because hand-written parallel code fuses the three
   passes into one loop. The methods make three passes and allocate two intermediate arrays. A
   future fusion of chained `parallel*` calls would be what closes that gap.

**When a tier lands:** move its rows out of `WAITING` in `bench/parallel/run.py` into `WORKLOADS`
(with a `js/` program, a `pool-worker.mjs` kernel and a `go/main.go` case). Drop the tier note from
the pipeline rows, and commit a new `results-<date>.md` beside the old one.

### 2026-09-27 -- full tier-1 through tier-4 refresh

`bench/parallel/results-2026-09-27-refresh.md` is a complete table of 15 workloads and eight
runtime/configuration columns, produced by `bench/parallel/run.sh --out
bench/parallel/results-2026-09-27-refresh.md`. The four allocating workloads now have Node
`worker_threads` and Go implementations alongside their Node, Bun and kiln columns. The runner
refreshes its release CLI before building the programs, so a stale binary cannot silently give a
number for an older compiler. Small-input checksum checks for those four rows agreed across Node,
the pool and Go before the full run.

Three interleaved warm rounds over one million elements ran in 10 minutes on the Ryzen 7 5800H;
median load was 4.3 on 16 hardware threads. Every cell's checksum matched Node's sequential
answer at twelve significant digits, and only two cells had more than 25% spread. All 21
`--explain-parallel` sites in the results file report `parallel` (including every stage of the
record pipelines and the tier-4 CSV parser). Representative medians in ms per call:

| workload | kiln sequential | kiln parallel at 16 | Node sequential | Node pool at 16 | Go at 16 |
| --- | ---: | ---: | ---: | ---: | ---: |
| enrich records | 156 | 37.0 | 114 | 461 | 5.52 |
| flatMap records | 256 | 45.0 | 33.5 | 115 | 4.08 |
| format CSV | 420 | 60.0 | 199 | 118 | 27.9 |
| parse CSV | 658 | 99.0 | 403 | 444 | 22.5 |

The parallel path is 4.2-7.0x faster than kiln's sequential method on these rows. The
`flatMap` row remains slower than Node's sequential method, and Go is faster on all four; the
table records those gaps rather than treating the parallel speedup as a complete performance
claim. The generated results and raw records under `target/bench-parallel/` preserve the full
evidence. `bash -n` and `python3 -m py_compile` checked the runner; the full run checked every
workload and checksum. No correctness gate was rerun for benchmark-only code.
