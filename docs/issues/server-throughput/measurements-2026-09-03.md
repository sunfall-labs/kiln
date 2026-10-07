# Server throughput on Linux/x86_64, 2026-09-03

`bench/server/compare.sh` at 06ca4a2, `RUNTIMES="kiln node bun"`, four client processes with
sixteen connections each, five seconds, three interleaved passes, median rps across passes.
`rss` is the server's resident set after a run.

- Linux 6.1.176 x86_64, Intel Xeon Platinum 8259CL @ 2.50GHz, 16 cores
- kiln built with `kiln build --node --profile native-release`
- node v22.23.2, bun 1.4.0; deno is not installed on this machine, so its rows are absent
- load average 2.6 at the start, 4.5 at the end (this machine is shared)

## Hello world

| runtime | requests/s | p50 | p99 max | resident |
|---|---|---|---|---|
| kiln native | 72.3k | 0.73 ms | 2.66 ms | 8 MB |
| bun 1.4.0 | 65.5k | 0.90 ms | 3.03 ms | 66 MB |
| node 22.23.2 | 39.0k | 1.43 ms | 4.25 ms | 81 MB |

## Small service

| runtime | requests/s | p50 | p99 max | resident |
|---|---|---|---|---|
| bun 1.4.0 | 44.7k | 1.08 ms | 4.25 ms | 76 MB |
| kiln native | 42.9k | 1.26 ms | 4.12 ms | 9 MB |
| node 22.23.2 | 27.4k | 1.90 ms | 6.28 ms | 83 MB |

## What `KILN_WORKERS` buys

Separate measurement, same binaries. Four client processes is not enough load to show it: the
client saturates first, and a second realm then costs more than it earns.

| clients | workers=1 | workers=2 | workers=4 | workers=8 |
|---|---|---|---|---|
| 4 (hello) | 71.0k | -- | 64.5k | -- |
| 4 (mixed) | 43.6k | -- | 51.8k | -- |
| 10 (hello) | 47.1k | 84.4k | 107.8k | 103.1k |

The ten-client row is the one that answers "what do workers buy": 2.3x, with p50 falling from
3.23 ms to 1.24 ms. The four-client rows are kept because they are the shape the comparison above
uses, and they show the opposite for hello -- a reminder that a throughput number without its
client count means nothing.

## Two things that made earlier numbers wrong

**The profile.** `kiln build` defaults to `native-dev`. The first comparison run here used it by
mistake and reported 56.7k hello and 26.0k mixed -- understating the release binary by 25% and 68%.
A benchmark of the wrong artifact is not a slow benchmark, it is a different program.

**The port.** A kiln server with `KILN_WORKERS>1` forks, and killing the parent reparents the
children to init, where they keep the `SO_REUSEPORT` listener open and serve the *next* run. Twenty
orphans had accumulated before the readings stopped making physical sense (93k rps while the
measured process used 18% of one core). `compare.sh` now reaps every process holding the port and
refuses to report a run it could not clean up after.

## Against the 2026-09-02 table

The numbers in `measurements-2026-09-02.md` and the README are roughly twice these on both
workloads. That file does not record its machine, so the difference is not explained here rather
than explained away: what is certain is that these rows came from this Xeon, at this date, with the
provenance above.
