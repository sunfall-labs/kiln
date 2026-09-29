# Concurrency benchmarks, 2026-09-29

Produced by `bench/concurrency/run.sh`; see `bench/concurrency/README.md` for what each row measures and what "not available" means. Medians of 3 interleaved round(s); a leading `~` marks a cell whose rounds spread by more than 25% of the median. Lower is better except where the unit is a rate (/s).

## Machine

- **kernel**: Linux 7.2.6-1-cachyos x86_64
- **cpu**: AMD Ryzen 7 5800H with Radeon Graphics, 16 hardware threads
- **memory**: 15.0 GiB
- **kiln**: a525e7c4 (dirty), `target/release/kiln`, native binaries `--profile native-release`, clang version 22.1.8
- **node**: v26.8.1
- **bun**: 1.4.2
- **go**: go1.27.1 linux/amd64
- **oha**: oha 1.16.0
- **started / finished**: 2026-09-29T05:46:23-06:00 / 2026-09-29T05:52:16-06:00
- **load average (1 min) seen before each job**: min 1.9, median 2.9, max 6.8 on 16 hardware threads

## 1. Worker start

| | node `worker_threads` | bun `Worker` | Go goroutine + channel | kiln `worker_threads` |
| --- | --- | --- | --- | --- |
| start one worker, first message back: warm median | 20.6 ms | 8.63 ms | 0.348 µs | 2.68 ms |
| the same, first (cold) start | 28.8 ms | 10.8 ms | ~9.80 µs | 3.21 ms |
| memory per added worker, 1 alive (RSS delta / N) | 11.9 MB | 15.0 MB | 84 KB | 2.77 MB |
| memory per added worker, 10 alive (RSS delta / N) | 7.36 MB | 1.88 MB | ~11.6 KB | 0.574 MB |
| memory per added worker, 100 alive (RSS delta / N) | 4.11 MB | 0.346 MB | 3.64 KB | 0.211 MB |

The public `spawnActor` path is measured separately because its ready handshake and first method call add work beyond `new Worker`. Each sample creates and stops a fresh actor realm; process startup is outside the clock.

| kiln actor API | time |
| --- | ---: |
| `spawnActor` to ready, first (cold) | ~4.31 ms |
| `spawnActor` to ready, warm median | 3.05 ms |
| first method call after ready, warm median | 0.296 ms |

Public actor capacity keeps every spawned realm alive while calling each one. The request window is 64; held RSS and thread count are sampled after all actors are ready. Every reply and stop is checked. The 10,000-actor row runs when the smaller counts project room for a reserve of at least 2 GiB or 20% of projected RSS, whichever is larger; a live memory guard stops it if the reserve is consumed.

| actors alive | spawn to all ready | one call each | stop all | held RSS | held threads |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | 8.87 ms | 1.79 ms | 0.452 ms | 28.4 MB | 18 |
| 100 | 53.7 ms | 13.6 ms | 6.26 ms | 82.6 MB | 18 |
| 1,000 | 556 ms | 191 ms | 61.8 ms | 616 MB | 18 |
| 10,000 | 7,403 ms | 9,056 ms | 2,627 ms | 5,943 MB | 18 |

Deployment comparison: one `KILN_WORKERS` process per worker (process mode, forked by the supervisor from a process that never built a realm). Timed from spawn to exit of the whole tree, each worker printing one line (the "message back"); memory is the summed PSS of the held-alive tree. The node and bun columns time the same one-line program as a whole process, for context.

| process start (spawn -> line -> exit), warm median | node | bun | kiln |
| --- | --- | --- | --- |
| 1 process | 23.1 ms | 4.43 ms | 4.48 ms (`KILN_WORKERS=1`, no supervisor) |

| `KILN_WORKERS=N` | wall, spawn to all N lines and exit | tree PSS held alive | tree RSS (sums shared text N times) | PSS per added worker |
| --- | --- | --- | --- | --- |
| N = 1 | 4.48 ms | 8.69 MB | 11.0 MB | -- |
| N = 2 | 6.70 ms | 10.5 MB | 24.3 MB | 1.78 MB |
| N = 10 | 8.82 ms | 22.0 MB | 103 MB | 1.48 MB |
| N = 100 | 37.0 ms | 153 MB | 1,000 MB | 1.46 MB |

## 2. Messages

Across threads (a `worker_threads` Worker; Go: a goroutine; kiln: a `worker_threads` Worker, a realm on a thread of the realm host, realms issue 06).

| | node | bun | Go | kiln |
| --- | --- | --- | --- | --- |
| small object, ping-pong round trip | 18.5 µs | 18.8 µs | 0.291 µs | 41.5 µs |
| small object, one-way throughput | 593k/s | 881k/s | 19,432k/s | 328k/s |
| 1 MB object graph, send + ack (Go: deep copy) | 5.18 ms | 5.46 ms | 0.601 ms | 11.4 ms |
| 16 MB ArrayBuffer, round trip, copied | ~13.4 ms | 20.3 ms | 2.16 ms | 4.43 ms |
| 16 MB ArrayBuffer, round trip, transferred | 0.0371 ms | 0.531 ms | 0.0249 ms | 0.0819 ms |
| 1 KB ArrayBuffer, round trip, transferred | 0.0169 ms | 0.0178 ms | -- | 0.0615 ms |
| 100 MB ArrayBuffer, round trip, transferred | 1.62 ms | 0.61 ms | -- | 0.0609 ms |
| 1 MB object graph by reference (Go only) | -- | -- | 0.0137 ms | -- |

In one realm: the global `MessageChannel`, both ports on the main thread (structured clone plus an event-loop hop, no thread crossing). The one-way row sends 10,000 messages here (100,000 across threads).

| in-realm `MessageChannel` | node | bun | kiln |
| --- | --- | --- | --- |
| small object, ping-pong round trip | 9.23 µs | 8.65 µs | 13.8 µs |
| small object, one-way throughput | 563k/s | 539k/s | 218k/s |
| 1 MB object graph, send + ack | 4.72 ms | 5.09 ms | 5.08 ms |
| 16 MB ArrayBuffer, round trip, copied | 12.8 ms | 11.2 ms | 3.20 ms |
| 16 MB ArrayBuffer, round trip, transferred | 0.0268 ms | 0.483 ms | 0.0465 ms |

The public actor call uses the same 1 MB graph shape in three modes. `prepareTransfer` snapshots a prebuilt graph; `isolate(factory)` constructs a fresh graph in movable storage. Their preparation and construction times appear separately and in their respective totals. The actor computes the same checksum in every mode. Actor startup is outside these clocks.

| kiln actor graph call | time |
| --- | ---: |
| structured clone call | 13.1 ms |
| structured clone round trip | 19.0 ms |
| prepare private snapshot | 4.79 ms |
| prepared handoff and reply | 4.98 ms |
| prepared total | 9.78 ms |
| direct factory construction | 8.23 ms |
| direct handoff and reply | 18.0 ms |
| direct construction through reply | 26.3 ms |

## 3. A 50 MB read-only table shared by N workers

Each worker does 2,000,000 pseudo-random lookups. Setup: from the first send until every worker holds the table (workers already started). Query: all workers' lookups, median of 3. RSS: the whole process after setup. The SharedArrayBuffer row shares bytes across workers; this row measures shared bytes; the frozen-value API (issue 11) has its own semantics.

| N | runtime / mode | setup | query | RSS |
| --- | --- | --- | --- | --- |
| 4 | node: copy (structured clone per worker) | 186 ms | 41.1 ms | 361 MB |
| 4 | node: SharedArrayBuffer | 0.108 ms | 38.8 ms | 162 MB |
| 4 | bun: copy (structured clone per worker) | ~173 ms | ~47.9 ms | 450 MB |
| 4 | bun: SharedArrayBuffer | 0.119 ms | 45.3 ms | 103 MB |
| 4 | go: one slice, shared | 0 ms | 20.4 ms | 58.6 MB |
| 4 | kiln: copy (structured clone per worker) | 98.7 ms | 106 ms | 268 MB |
| 4 | kiln: SharedArrayBuffer | 0.166 ms | 95.4 ms | 68.4 MB |
| 16 | node: copy (structured clone per worker) | 648 ms | 114 ms | 1,112 MB |
| 16 | node: SharedArrayBuffer | ~0.326 ms | 95.5 ms | 312 MB |
| 16 | bun: copy (structured clone per worker) | 632 ms | 107 ms | 1,741 MB |
| 16 | bun: SharedArrayBuffer | 0.262 ms | 98.0 ms | 151 MB |
| 16 | go: one slice, shared | 0 ms | 65.0 ms | 59.1 MB |
| 16 | kiln: copy (structured clone per worker) | 394 ms | 233 ms | 877 MB |
| 16 | kiln: SharedArrayBuffer | 0.488 ms | 176 ms | 76.9 MB |

A separate kiln-only row holds the same ordinary object graph in public actors. The copy mode clones it into each actor; `share()` snapshots once and sends shared handles. Each mode runs in a fresh process. Dataset RSS is the held RSS minus RSS after actors have started, so actor startup is outside the setup and memory deltas.

| actors | mode | snapshot | load all | query all | dataset RSS delta | held RSS |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 4 | copy | 0.000541 ms | 83.8 ms | 7.32 ms | 46.1 MB | 72.9 MB |
| 4 | shared | 18.9 ms | 62.2 ms | 23.1 ms | 34.5 MB | 61.2 MB |
| 16 | copy | ~0.000601 ms | 318 ms | 22.2 ms | 175 MB | 210 MB |
| 16 | shared | 19.1 ms | 247 ms | 117 ms | 99.5 MB | 135 MB |

## 4. Many concurrent tasks

N tasks each wake 10 times on a 10 ms timer and add to a counter (floor: 100 ms). JavaScript: async functions in one realm, kiln included (one realm; the separate native-realm workload is section 7). Go: goroutines. Peak RSS is the process's maximum resident set.

| N | metric | node | bun | Go | kiln (async tasks, one realm) |
| --- | --- | --- | --- | --- | --- |
| 10 | wall | 103 ms | 101 ms | 103 ms | 108 ms |
| 10 | ticks/s | 973/s | 988/s | 974/s | 923/s |
| 10 | peak RSS | 51.6 MB | ~51.0 MB | ~51.0 MB | ~51.0 MB |
| 1,000 | wall | 110 ms | 103 ms | 111 ms | 109 ms |
| 1,000 | ticks/s | 91,115/s | 97,108/s | 89,858/s | 91,471/s |
| 1,000 | peak RSS | 57.2 MB | ~51.0 MB | ~51.0 MB | ~51.0 MB |
| 100,000 | wall | 1,057 ms | 480 ms | 284 ms | 774 ms |
| 100,000 | ticks/s | 946k/s | 2,082k/s | 3,524k/s | 1,292k/s |
| 100,000 | peak RSS | 420 MB | 194 MB | 225 MB | 293 MB |

These JavaScript tasks share one realm. Section 7 measures separate native realms and includes the launcher's result-file cost.

## 5. Data-parallel map

`(x) => { 16 rounds of y = (y * 31 + k) % 1000003 }` over 10,000,000 numbers; warm median of 3 (after one cold run). Peak RSS of the whole run.

| | node | bun | Go | kiln |
| --- | --- | --- | --- | --- |
| sequential `map` (Go: a loop) | 545 ms | 529 ms | 676 ms | 892 ms |
| all cores (node/bun: `worker_threads` pool of 16, copy in, transfer back; Go: goroutines; kiln: `parallelMap`) | 231 ms | 230 ms | 74.0 ms | 83.8 ms |
| kiln `parallelMap` with `KILN_PARALLELISM=1` | -- | -- | -- | 880 ms |
| peak RSS, sequential | 429 MB | 352 MB | 391 MB | 393 MB |

The same over a light callback, `(x) => (x * 3 + 1) % 1000003`: memory-bound, but the shape kiln's typed `parallelMap` pool accepts.

| light callback | node | bun | Go | kiln |
| --- | --- | --- | --- | --- |
| sequential `map` (Go: a loop) | 108 ms | 51.7 ms | 45.2 ms | 53.2 ms |
| all cores (Go: goroutines; kiln: `parallelMap`) | -- | -- | ~15.5 ms | 14.3 ms |

## 7. Separate native realms on the turn scheduler

Each realm wakes on a timer 10 times and prints `10`; every output and exit status is checked. Wall time includes process launch, realm setup, timer turns, and the native launcher's two result files per realm. Peak RSS is for the whole process. The 100,000 realm row runs only if projected RSS plus a reserve of at least 2 GiB or 20% fits available memory; a live guard checks the same reserve. This row has no Node/Bun equivalent: their one-realm async tasks appear in section 4, and spawning 100,000 worker isolates is a different resource budget.

| realms | wall | turns/s | peak RSS |
| ---: | ---: | ---: | ---: |
| 10 | ~15.0 ms | ~6,650/s | ~51.0 MB |
| 1,000 | 91.6 ms | 109k/s | 75.0 MB |
| 100,000 | 7,914 ms | 126k/s | 501 MB |

## Divergences, failures and skips

None: every comparable answer matched the reference runtime (Go where it runs the case, else node).

Noisy cells (rounds spread > 25%): 18.
