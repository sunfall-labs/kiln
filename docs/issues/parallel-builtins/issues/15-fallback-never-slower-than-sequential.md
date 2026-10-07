# 15. A parallel method's fallback is never slower than its sequential method

Status: resolved
Priority: high (breaks the PRD's "never slower than `map`")

Found by the benchmarks (issue 11): when `parallelFilter` or `parallelReduce` decide at run time
not to use the pool (e.g. over an array produced by `map` over objects), what runs instead is far
slower than the sequential method: `parallelReduce` 460 ms against `reduce` 14 ms at 1M elements,
`parallelFilter` 37 ms against 7.8 ms. The same values pushed in a loop run fast in parallel. The
fallback must be the sequential method's own fast path, not a generic call through the TypeScript
definition. Repro in issue 11's Comments.

## Acceptance

- For every method in the family and every array kind, `parallelX` with the pool declined is within
  noise of `X` (a probe per method comparing both at 1M elements, both engines).
- The object-records pipeline row of `bench/parallel` no longer slower than the sequential pipeline.

## Comments

2026-09-27, branch `work/sequential-fast-paths`.

**Why the fallback was slow.** Two causes, both structural.

1. A family site lowered only its pooled path; when the pool declined, the site made the generic
   method call into `kiln-builtins/src/parallel.ts`. That is `filter.call(...)` through the C
   built-in (a generic invocation per element: 37 ms against `filter`'s inline loop at 7.8 ms),
   and for `parallelReduce` the TypeScript tree itself, compiled generic-only at `-O0` (a
   `callback.call` and a generic element read per element: 330-460 ms against 7-14 ms).
2. The repro's array came from `map` over objects, and `map`'s inline loop created its result
   with the *receiver's* `is_numeric` flag. Records are not numbers, so a result made only of
   numbers was flagged non-numeric and every pooled lowering declined it at run time.

**Fix.** `emit_call_method` now maps a family site to its sequential method (`parallelMap` ->
`map`, `parallelFilter` -> `filter`, `parallelSort` -> `sort`) and emits that method's own inline
loops at the site, after the pooled lowering and behind `kiln_c99_kiln_extensions_on() &&
kiln_c99_builtin_store_epoch == 0`. So "pool declined" is exactly what `X` runs at that site,
typed callback entries included (`direct.rs` now references the family's callbacks at the
sequential arities, not only when parallel-safe). `parallelReduce` has no sequential method: its
tree (leaves of 256 from `initial`, pairwise levels, odd one carried) is written out once more in
`emit_graph/ops/callback_loops.rs::reduce_tree`, over arrays (holes skipped as `k in O` does) and
Number-kind views, typed when both callbacks' typed entries answer doubles, else by closure calls
with the levels in a collected array (objects answered by callbacks stay rooted; checked under
`KILN_GC_STRESS`). `parallel_map_intrinsic`, the old private copy of `map`'s loop, is gone:
`map`'s loop took over its thisArg handling (`map`/`filter` now lower with a thisArg too). `map`
and `filter` results are numeric until a non-number lands in them. `parallelFlatMap` still
falls back through the built-in, exactly like `flatMap` at the same site (neither has a loop).

**Probes.** `conformance/native/parallel-fallback.js` (map, filter, sort) and
`parallel-fallback-reduce.js` (the tree, including object accumulators and empty inputs), each
declined at compile time (reassigned capture) and at run time (an array holding non-numbers), over
arrays and a `Float64Array`, kept under the release profile's specialization limits so they
exercise the site loops; interpreter == native, and both == node with the family shimmed.
Timings: `bench/parallel/fallback/run.sh [kiln]` (one small program per method; 1M elements, best
of 7, ms, load 7-15):

| kind | method | `X` before | `parallelX` before | `X` after | `parallelX` after |
|---|---|---|---|---|---|
| numeric | map | 7.25 | 10.7 | 7.21 | 7.35 |
| mixed | map | 10.6 | 10.7 | 10.5 | 11.2 |
| typed | map | 249 | 12.3 | 8.66 | 8.85 |
| numeric | filter | 6.85 | 35.6 | 6.79 | 6.56 |
| mixed | filter | 9.25 | 35.5 | 9.64 | 9.05 |
| typed | filter | 287 | 295 | 12.2 | 12.8 |
| numeric | reduce | 7.54 | 331 | 7.60 | 8.29 |
| mixed | reduce | 7.60 | 350 | 7.37 | 7.88 |
| typed | reduce | 228 | 318 | 8.54 | 9.36 |
| numeric | sort `a - b` | 169 | 456 | 170 | 170 |
| mixed | sort `a - b` | 170 | 473 | 171 | 169 |
| typed | sort `a - b` | 1,967 | 1,927 | 613 | 609 |

`parallelReduce`'s fallback runs 5-10% behind `reduce` (two passes: leaves, then levels). The
interpreter always runs the TypeScript, where `parallelX` is `X` by construction; its clock is
virtual, so it is checked for answers (native.sh), not timed.

**The pipeline row** (`bench/parallel/run.sh --quick`, same machine, noisy): object records
before seq 71.0 / par@16 108 (0.7x); after seq 41.5 / par@1 32.0 / par@16 39.5 (1.1x) -- the
filter and reduce stages now pool. Acceptance holds.
