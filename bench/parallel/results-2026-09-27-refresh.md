# Parallel built-ins benchmarks, 2026-09-27

Produced by `bench/parallel/run.sh`; `bench/parallel/README.md` says what each row and column measures. 1,000,000 elements. Each cell is milliseconds per call, warm (the median of calibrated batches inside one process, 1000 ms budget per run), then the median of 3 interleaved round(s); `~` marks a cell whose rounds spread by more than 25% of the median. Lower is better. The last two columns are kiln's all-core parallel method against kiln's own sequential method and against node's.

## Machine

- **cpu**: AMD Ryzen 7 5800H with Radeon Graphics, 16 hardware threads
- **memory**: 15.0 GiB
- **kernel**: Linux 7.2.6-1-cachyos x86_64
- **kiln**: 1cc3572d (rebased as 704bf0ff; `bench/parallel` is identical), `target/release/kiln`, programs built `--node --profile native-release`; clang version 22.1.8
- **node**: v26.8.1
- **bun**: 1.4.2
- **go**: go1.27.1 linux/amd64
- **started / finished**: 2026-09-27T17:48:11-06:00 / 2026-09-27T17:58:12-06:00
- **load average (1 min) seen before each job**: min 2.1, median 4.3, max 9.6 on 16 hardware threads. The machine is shared: other agents' builds run alongside, so single cells move by tens of percent between runs; ratios within a row are the reliable part.

## Results (ms per call)

| workload | node `map`/… | bun `map`/… | node `worker_threads` pool (16) | Go goroutines (16) | kiln `map`/… | kiln `parallel*` @1 | kiln `parallel*` @4 | kiln `parallel*` @16 | kiln @16 vs kiln seq | kiln @16 vs node |
|---|---|---|---|---|---|---|---|---|---|---|
| `map-light`: map `x * 1.5 + 2`, Float64Array | 11.0 | 11.9 | ~0.891 | ~1.32 | 1.88 | 0.664 | 0.688 | 0.750 | 2.5x | 14.7x |
| `map-heavy`: map `sqrt(x*x+1)*sin(x)+cos(x/2)`, Float64Array | 42.5 | 33.5 | 3.75 | 3.02 | 20.0 | 19.0 | 5.00 | 2.38 | 8.4x | 17.9x |
| `filter`: filter `sin(x) * x > cut`, Float64Array | 30.0 | 18.8 | 2.62 | 1.76 | 21.0 | 11.0 | 4.19 | 2.59 | 8.1x | 11.6x |
| `reduce-sum`: reduce `a + x`, Float64Array | 11.0 | 10.4 | 0.266 | 0.130 | 2.94 | 1.25 | 0.406 | 0.383 | 7.7x | 28.7x |
| `reduce-fold`: reduce, 8-round integer mix per element | 134 | 125 | 10.1 | 1.90 | 37.0 | 35.5 | 10.6 | 4.81 | 7.7x | 27.8x |
| `sort-typed`: sort Float64Array, default order (copy + sort) | 89.0 | 20.5 | 16.5 | 10.3 | 22.8 | 42.5 | 14.5 | 10.2 | 2.2x | 8.7x |
| `sort-array`: sort number array, `(a, b) => a - b` (copy + sort) | 390 | 189 | 16.5 | 23.0 | 185 | 20.5 | 19.2 | 8.50 | 21.8x | 45.9x |
| `pipeline-column`: pipeline: amounts -> cents (map) -> > 500.00 (filter) -> total (reduce) | 54.0 | 29.0 | 0.367 | 0.258 | 27.0 | 5.38 | 3.88 | 3.88 | 7.0x | 13.9x |
| `sort-records`: sort `{id, price}` records, `(a, b) => a.price - b.price` (copy + sort) | 476 | 306 | 66.0 | 32.3 | 486 | 227 | 90.0 | 49.5 | 9.8x | 9.6x |
| `pipeline-records`: pipeline: score records held as typed-array columns -> filter -> total | 40.0 | 26.5 | 0.523 | 0.361 | 36.5 | 23.2 | 8.00 | 5.06 | 7.2x | 7.9x |
| `pipeline-objects`: pipeline: score `{price, qty, rating}` objects -> filter -> total | 23.2 | 9.62 | 2.56 | 0.335 | 23.2 | 22.2 | 11.5 | 12.0 | 1.9x | 1.9x |
| `enrich-records`: tier 3: `records.map(r => ({ ...r, total: r.price * r.qty }))` | 114 | 21.0 | 461 | 5.52 | 156 | 154 | 48.0 | 37.0 | 4.2x | 3.1x |
| `flatmap-records`: tier 3: `records.flatMap(r => r.qty > 4 ? [r.price, r.qty] : [])` | 33.5 | 24.5 | 115 | 4.08 | 256 | 261 | 44.0 | 45.0 | 5.7x | 0.7x |
| `format-csv`: tier 3: `records.map(r => `${r.id},${r.qty},${r.price}`)` | 199 | 164 | 118 | 27.9 | 420 | 408 | 123 | 60.0 | 7.0x | 3.3x |
| `parse-csv`: tier 4: `lines.map(l => l.trim().split(",").map(Number))` | 403 | 214 | 444 | 22.5 | 658 | 656 | 142 | 99.0 | 6.6x | 4.1x |

2 cell(s) marked `~` spread by more than 25% across rounds.

## What ran in parallel (`kiln build --explain-parallel`)

```
bench/parallel/js/map-light.mjs:8:14  parallelMap  parallel
bench/parallel/js/map-heavy.mjs:8:14  parallelMap  parallel
bench/parallel/js/filter.mjs:9:14  parallelFilter  parallel (tier 1: reads captured values)
bench/parallel/js/reduce-sum.mjs:8:14  parallelReduce  parallel
bench/parallel/js/reduce-fold.mjs:8:14  parallelReduce  parallel
bench/parallel/js/sort-typed.mjs:12:32  parallelSort  parallel (typed arrays: the default numeric order)
bench/parallel/js/sort-array.mjs:8:22  parallelSort  parallel
bench/parallel/js/pipeline-column.mjs:13:6  parallelMap  parallel (tier 1: reads captured values)
bench/parallel/js/pipeline-column.mjs:14:6  parallelFilter  parallel (tier 1: reads captured values)
bench/parallel/js/pipeline-column.mjs:15:6  parallelReduce  parallel
bench/parallel/js/sort-records.mjs:10:27  parallelSort  parallel (tier 2: reads the heap)
bench/parallel/js/pipeline-records.mjs:14:6  parallelMap  parallel (tier 3: builds values in worker arenas)
bench/parallel/js/pipeline-records.mjs:15:6  parallelFilter  parallel (tier 1: reads captured values)
bench/parallel/js/pipeline-records.mjs:16:6  parallelReduce  parallel
bench/parallel/js/pipeline-objects.mjs:15:6  parallelMap  parallel (tier 3: builds values in worker arenas)
bench/parallel/js/pipeline-objects.mjs:16:6  parallelFilter  parallel (tier 1: reads captured values)
bench/parallel/js/pipeline-objects.mjs:17:6  parallelReduce  parallel
bench/parallel/js/enrich-records.mjs:13:19  parallelMap  parallel (tier 3: builds values in worker arenas)
bench/parallel/js/flatmap-records.mjs:13:19  parallelFlatMap  parallel (tier 3: builds values in worker arenas)
bench/parallel/js/format-csv.mjs:13:19  parallelMap  parallel (tier 3: builds values in worker arenas)
bench/parallel/js/parse-csv.mjs:13:17  parallelMap  parallel (tier 4: pure calls)
```

## Divergences

None: every cell's checksum matched node's sequential method at twelve significant digits.
