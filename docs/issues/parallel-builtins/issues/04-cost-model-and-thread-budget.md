# 04. Cost model and thread budget

Status: resolved

**Cost model.** Today `kiln_parallel_map` in the provider splits work into fixed-size chunks
(`CHUNK`) whenever the callback qualifies. Add a threshold on element count and estimated work per
element below which the call stays sequential, and size chunks from the measured cost of the first
chunk. Goal: no input size where `parallelMap` is measurably slower than `map`.

**Thread budget.** With `KILN_WORKERS=N`, every process builds a full pool today, so the machine
runs N × cores threads. Default the pool to `max(1, cores / N)`; `KILN_PARALLELISM` still
overrides it. The launcher passes N to each realm.

## Acceptance

- A benchmark sweeping input sizes from 1 to 10 million shows `parallelMap` never slower than
  `map` by more than measurement noise.
- `KILN_WORKERS=auto` on a many-core box runs at most one pool thread per core in total.

## Note, 2026-09-26

ADR 0070 replaces the separate pool with its scheduler (realms issue 09), which removes the
oversubscription for good. The thread budget here is the interim fix until then.

## Comments

2026-09-27, branch `work/parallel-cost-model` (commit `12583131`). Interim, as the note above
says: everything lives in `crates/native/kiln-native-provider-ffi/src/provider/parallel.rs`, in
one block marked for deletion when ADR 0070's scheduler takes the chunks. No emitter or C change.

**Cost model.** The fixed floor (16,384 elements) and fixed 1,024-element chunks are gone.
`kiln_parallel_map` now runs a growing prefix on the calling thread -- 128 elements with no clock
read at all, then batches growing 4x until one batch takes 2 us -- and takes the per-element cost
from that last batch (the earlier, smaller ones miss the page faults a fresh output array pays
past its first page, and read 1 ns for a body whose real cost there is 4). If the remaining work
is under 100 us (about twice the fork-join overhead measured here: 45-55 us on the 16-thread
Ryzen 7 5800H with other builds running; the old table recorded 60-90 us on the M-series laptop)
the call finishes inline. Otherwise the rest goes to the pool in chunks of ~25 us of work, with at
least four chunks per pool thread so stealing covers a descheduled worker. The pool itself is
started only when a call first decides to use it. `chunk_plan` and `pool_size` are pure functions
with unit tests; the pooled path is also exercised with a 4M-element call.

**Thread budget.** Default pool = `max(1, cores / KILN_WORKER_COUNT)` (the supervisor sets
`KILN_WORKER_COUNT` in every worker before its realm starts); `KILN_PARALLELISM` still overrides.
Measured by counting `kiln-parallel-*` threads in `/proc` across the worker processes of a
2M-element `parallelMap` on the 16-thread box: no `KILN_WORKERS` 16; `KILN_WORKERS=4` 16 in
total (4 x 4; was 64); `KILN_WORKERS=auto` 0 (16 realms, every call inline; was 256);
`KILN_WORKERS=4 KILN_PARALLELISM=3` 12.

**Sweep.** `map` against `parallelMap` in one native-release binary, us per call, min over rounds
of a >= 3 ms batch with the two interleaved round by round; light `x => x * 2 + 1`, heavy
`Math.sqrt(x * x + 1) * Math.pow(x % 7 + 1, 1.5) + Math.floor(x / 3)` (~23 ns). "old" is the same
source built against the provider before this change. The box was shared (load 5-26), so single
cells move by 10-30% between runs; ratios within a row are the reliable part.

| n | light map | light pMap | (old) | heavy map | heavy pMap | (old) |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 0.06 | 0.06 | 0.06 | 0.07 | 0.08 | 0.08 |
| 10 | 0.06 | 0.07 | 0.07 | 0.25 | 0.24 | 0.24 |
| 100 | 0.17 | 0.15 | 0.16 | 2.18 | 2.25 | 2.25 |
| 1,000 | 0.92 | 0.81 | 0.75 | 22.7 | 23.7 | 23.4 |
| 4,000 | 3.39 | 2.86 | 2.71 | 107.8 | 52.0 | 108.5 |
| 16,000 | 13.4 | 10.4 | 9.9 | 400 | 103 | 427 |
| 64,000 | 144 | 88 | 72 | 1,870 | 340 | 309 |
| 256,000 | 854 | 221 | 238 | 7,385 | 1,167 | 1,256 |
| 1M | 3,471 | 792 | 828 | 29,590 | 3,954 | 3,809 |
| 4M | 14,014 | 3,317 | 3,455 | 120,593 | 15,232 | 15,039 |
| 10M | 35,478 | 8,309 | 8,503 | 302,970 | 41,065 | 44,594 |

The old floor's two failures are gone: a heavy body between ~4k and 16k elements stayed
sequential (16,000: 427 us against 103 now), and a light body just past the floor went to the pool
for too little work (an earlier run, same box: 64,000 light, 74 us pooled against 58 us `map`).

**One gap the cost model cannot close.** Where a heavy call stays inline (below ~4k elements of
this body), `parallelMap` is 3-5% slower than `map` (1,000: 23.7 against 22.7; repeated runs
4.4-5.5%). It is the typed chunk worker's own code, not the pool or the probe: the pre-change binary
shows it (23.4 against 22.1) and so does `KILN_PARALLELISM=1` (25.2 against 23.8, and 700 against
560 at 16,000), where the probe never runs. The light body's worker is the other way round, 10-20%
faster than `map`. So the worker's lowering of `Math.pow`/`Math.floor` against `map`'s is the
place to look; it belongs with issue 14 (the workers' codegen), not here. Declining the call to
`map`'s lowering when the probe says "inline" would close it for this body but give back the light
body's win, and re-run the probed prefix.

