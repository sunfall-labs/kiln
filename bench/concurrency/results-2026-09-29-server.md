# Concurrency benchmarks, 2026-09-29

Produced by `bench/concurrency/run.sh`; see `bench/concurrency/README.md` for what each row measures and what "not available" means. Medians of 3 interleaved round(s); a leading `~` marks a cell whose rounds spread by more than 25% of the median. Lower is better except where the unit is a rate (/s).

## Machine

- **kernel**: Linux 7.2.6-1-cachyos x86_64
- **cpu**: AMD Ryzen 7 5800H with Radeon Graphics, 16 hardware threads
- **memory**: 15.0 GiB
- **kiln**: 6696a3a1, `target/release/kiln`, native binaries `--profile native-release`, clang version 22.1.8
- **node**: v26.8.1
- **bun**: 1.4.2
- **go**: go1.27.1 linux/amd64
- **oha**: oha 1.16.0
- **started / finished**: 2026-09-29T12:14:22-06:00 / 2026-09-29T12:17:01-06:00
- **load average (1 min) seen before each job**: min 1.2, median 11.3, max 17.5 on 16 hardware threads

## 6. Hello-world HTTP server at all cores

`oha -c 128 -z 5s` after a 1 s warm-up, on the same machine as the server (the load generator competes for the same cores). Memory: summed PSS (and RSS) of the server's process tree after the load.

| server | rps | p50 | p99 | tree PSS | tree RSS | processes | errors |
| --- | --- | --- | --- | --- | --- | --- | --- |
| kiln `KILN_WORKERS=auto` (supervisor + one realm process per core) | 381k/s | 0.269 ms | 1.86 ms | 157 MB | 355 MB | 17 | 0 |
| kiln `KILN_WORKERS_MODE=thread` (one process, one realm per core) | 381k/s | 0.276 ms | 1.69 ms | 147 MB | 149 MB | 1 | 0 |
| node `cluster` (primary + one worker per core) | 202k/s | 0.346 ms | 6.80 ms | 556 MB | 1,390 MB | 17 | 0 |
| bun `Bun.serve` + `reusePort`, one process per core | 376k/s | 0.287 ms | 1.82 ms | 119 MB | 508 MB | 17 | 0 |
| Go `net/http` | 266k/s | 0.274 ms | 2.43 ms | 17.0 MB | 18.8 MB | 1 | 0 |
| kiln `KILN_WORKERS=1` (one realm, for scale) | 111k/s | 1.10 ms | 1.80 ms | 24.5 MB | 26.8 MB | 1 | 0 |
| node, one process (for scale) | 39,557/s | 3.18 ms | 3.52 ms | 85.5 MB | 89.1 MB | 1 | 0 |

This is a high-concurrency, same-machine HTTP measurement. It does not remeasure the 16-connection case where thread mode previously trailed process mode, or the value of process mode's crash isolation; process remains the default (issue 07).

## Divergences, failures and skips

None: every comparable answer matched the reference runtime (Go where it runs the case, else node).

Noisy cells (rounds spread > 25%): 0.
