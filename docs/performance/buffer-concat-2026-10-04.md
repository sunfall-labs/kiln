# Buffer concatenation, 2026-10-04

The native Buffer workload improved from 1,162 ms to 28 ms per warm iteration, a 41.5× speedup. Bun 1.4.2 took 6.125 ms, leaving native 4.57× slower. All nine process samples returned the same checksum, `78000000`.

| Runtime | Warm medians, ms | Median, ms |
| --- | --- | ---: |
| Native before | 1164, 1162, 1158 | 1162 |
| Native after | 28, 28, 27.75 | 28 |
| Bun | 6.25, 6.125, 6.125 | 6.125 |

The benchmark concatenates 1,000 buffers 100 times per iteration. Buffer.concat retains its TypeScript validation, truncation and zero-padding behavior, but copies compatible storage through the existing typed-array bulk-copy intrinsic. Unsupported storage retains the indexed fallback. The bootstrap handoff is deleted before user code runs. No new C built-in was added.

The existing harness calibrates batches and separates cold calls from warm samples; this table takes the median of three fresh-process warm medians. Module setup, compilation and process startup are excluded. Runtime order alternated across rounds. The integration gate had completed, agents held CPU work, and the full-suite runner and its current compilation were paused for 27.58 seconds and resumed in a finally block. No active test was paused. [Results and source/compiler/binary digests](buffer-concat-2026-10-04.json) retain the individual records.

The integrated routine gate passed all ten steps: 2,565 crate tests, 637 native matches with six named refusals and no divergences, all 17 present third-party bundles, and all 12 routine performance baselines. Routine Test262 had no regressions across 122 chunks; 116 extended variants were deferred. Focused Buffer probes also passed under allocation stress. [Gate reference](../issues/effect-upstream/buffer-gate-reference.json) retains the frozen source and verdict.
