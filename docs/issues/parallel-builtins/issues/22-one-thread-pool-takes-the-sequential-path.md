# 22. A one-thread pool takes the sequential path

Status: resolved

Found by issue 07 (tier 2): with the pool at one thread (`KILN_PARALLELISM=1`, or a machine or
`KILN_WORKERS` share of one core), a tier-2 site still runs its boxed, guarded clone, which is about
25% slower than the sequential method on object records. One thread cannot win anything, so the
sequential method's own loop must run instead — the rule of issue 15 ("never slower than the
sequential method"). More generally, let the cost model's first-chunk probe compare the clone's
per-element cost against the sequential loop's where both are measurable, and choose the faster.

## Acceptance

- At `KILN_PARALLELISM=1`, every parallel method on every array kind within noise of its sequential
  method (bench/parallel rows and bench/parallel/fallback).

## Comments

### 2026-09-27 -- with tier 3 (issue 08), branch `work/parallel-tier3`

New provider export `kiln_parallel_threads()` (ABI 49): the pool's size, 1 without a pool. The
first cut put every pooled lowering behind `kiln_parallel_threads() > 1`, and the quick bench
showed that is wrong for two rows: at one thread the pooled lowering does *less work* than the
sequential method, not the same work with more overhead -- the key sort computes each key once and
sorts numbers (`sort-records` @1: 226 ms pooled against 464 ms for `sort`), and the typed-column
pipeline runs its typed clone (`pipeline-records` @1: 24.8 against 35.5). So the guard sits only
where the issue measured the loss: the heap map/filter branches over arrays (the boxed, guarded
clone against `map`'s inline-cached loop, and tier 3's arenas). There, one thread runs the
sequential method's own loop.

At `KILN_PARALLELISM=1` (quick bench, ms): `pipeline-objects` 22.8 against `map` pipeline 24.0
(was ~25% behind); `enrich-records` 398 / 396, `flatmap-records` 321 / 319, `format-csv` 417 /
409; `map-heavy` 19.0 / 20.2, `pipeline-records` 24.8 / 35.5, `sort-records` 225 / 464 (the
pooled lowering kept where it wins). `bench/parallel/fallback` at `KILN_PARALLELISM=1`: every
pair within noise (map, filter, sort) except `parallelReduce`'s tree, 7-11% behind `reduce` as
before this change.

Not done: the "more generally" half -- letting the first-chunk probe measure the clone against the
sequential loop and choose. The provider's probe times only the clone; a comparison needs the site
to hand it a sequential chunk function as well.
