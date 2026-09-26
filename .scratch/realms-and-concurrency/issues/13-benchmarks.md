# 13. Benchmarks against Node, Bun and Go

Status: ready-for-agent
Blocked by: 01 (then extended as each issue lands)

Under `bench/`, following `CLAUDE.md`'s rules (warm, interleaved, checksums compared):

- worker start time and memory: a kiln realm, a Node worker, a Bun worker, a goroutine;
- message throughput and latency, small and large payloads, copy and transfer;
- sharing a large read-only dataset across N workers;
- many-realm scaling: 10, 1,000 and 100,000 realms or goroutines doing a small periodic task;
- server scaling: thread mode against process mode against Node `cluster` against Go `net/http`.

## Acceptance

- One command reproduces the table, with machine details, and it is updated as each issue lands.

## Comments

### 2026-09-26: the suite and the first baseline

`bench/concurrency/run.sh` (`--quick` for one round) reproduces the table; `bench/concurrency/README.md`
explains every workload and what "not available" means. Full results, machine details and the
per-workload tables: `bench/concurrency/results-2026-09-26.md` (3 interleaved rounds at `1e46b859`,
7m47s, load average median 20.5 on 16 threads -- noisy, read to one significant digit).

| workload | node | bun | Go | kiln today |
| --- | --- | --- | --- | --- |
| start a worker, first message back (warm) | 43 ms | 19 ms | 0.8 µs | not available; nearest: `KILN_WORKERS=1` process spawn-to-exit 9.0 ms (node process 33 ms, bun 9.4 ms) |
| memory per added worker, 100 alive | 4.5 MB | 3.2 MB | 3.4 KB | not available; `KILN_WORKERS` process: 1.6 MB PSS per added process |
| small message ping-pong, across threads | 65 µs | 28 µs | 0.3 µs | not available; in-realm `MessageChannel` 148 µs (node 25, bun 9.3) |
| small messages one-way | 246k/s | 578k/s | 17.9M/s | not available; in-realm 8.4k/s (node 126k, bun 486k) |
| 1 MB object graph send + ack | 15.5 ms | 6.7 ms | 0.67 ms (deep copy) / 0.014 ms (ref) | in-realm 175 ms |
| 16 MB ArrayBuffer round trip, copy / transfer | 45 / 0.09 ms | 20 / 1.2 ms | 2.4 / 0.03 ms | in-realm 18 / 11 ms |
| 50 MB table, 16 workers: setup copy / SAB, query, RSS | 677 / 0.4 ms, 116-135 ms, 312-1,112 MB | 719 / 0.3 ms, 110-130 ms, 153-1,691 MB | 0 ms, 82 ms, 59 MB | not available |
| 100,000 tasks x 10 timer ticks: wall, peak RSS | 1.2 s, 418 MB | 0.57 s, 196 MB | 0.30 s, 208 MB | 2.9 s, 732 MB (async tasks, one realm) |
| map 10M, heavy callback: sequential / all cores | 991 / 349 ms (pool) | 640 / 407 ms (pool) | 723 / 104 ms | 952 / 1,144 ms (`parallelMap` falls back to sequential) |
| map 10M, light callback: sequential / all cores | 120 ms / -- | 62 ms / -- | 48 / 17 ms | 56 ms / build failed (see below) |
| hello-world HTTP, all cores: rps, p99, tree PSS | 88k (`cluster`), 12.5 ms, 554 MB | 177k (`reusePort`), 3.6 ms, 107 MB | 153k, 5.7 ms, 18 MB | 281k (`KILN_WORKERS=auto`), 3.7 ms, 172 MB |

Caveats:

- Noise: 91 cells spread by more than 25% across rounds; a quick run at load 5-12 gave the same
  ordering with smaller times. `oha` shares the cores with the servers.
- `parallelMap`'s typed pool does not compile at `1e46b859`: `call_method.rs`'s pool guard names
  `kiln_inheriting_new_target` (now `kiln_c99_inheriting_new_target`). With that renamed in a
  scratch build (not committed), the light map runs in ~19 ms vs ~119 ms at `KILN_PARALLELISM=1`.
  The heavy callback (a loop) is not accepted by the pool at all.
- A second emitter bug: `{ code: i * 3 }` inside a loop emits an undeclared `d<n>` temporary;
  worked around in `js/common.mjs` with `(i * 3) | 0`.
- kiln's in-realm `MessageChannel` is 10-60x slower than node/bun's, and transfer costs about as
  much as copy.
- Not measurable yet (marked "not available" in the table): kiln workers, cross-realm messages,
  sharing a dataset across workers; many-realm scaling is measured as many async tasks in one
  realm. These rows fill in as 05-10 land.

