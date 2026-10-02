# Server throughput, 2026-09-01 evening

`bench/server/run.sh`, three interleaved rounds of 5 s per concurrency, Node keep-alive client,
load average ~7 from other sessions. kiln is `bench/server/serve.mjs` compiled with
`--node --profile native-release` at perf/review bb8acd76, collector on with its 2 MiB floor.

| server | c=1 rps | c=16 rps | c=64 rps | RSS start -> end |
|---|---|---|---|---|
| kiln native | 16.8k-20.2k | 48.5k-70.6k | 39.1k-39.5k | 3.4 -> 42 MiB |
| node v25.9 | 31.9k-32.6k | 81.2k-86.9k | 77.1k-87.6k | 59 -> 98 MiB |
| bun 1.4 | 32.4k-33.9k | 80.9k-83.1k | 80.7k-84.1k | 23 -> 94 MiB |

Same day, before this branch (main 9c82b0bc / node/work ae0021aa, 3 s runs): kiln 953/163/108
and 9,287/9,171/8,058, both leaking 7-12 KiB per request.

What moved it: the collector (ADR 0041) made memory flat; the checkpoint fix (32744447) removed
four reactor turns per HTTP completion. What is left: the c=64 plateau, and per-request CPU in
`checkpoint_http`'s per-connection loop and the request/response object construction.

## Night, after ADR 0042 and the object-model work (perf/review 93912110)

`bench/server/run.sh`, port 18917, one 5 s run per cell per pass, three interleaved passes over all
four runtimes; median (min-max). Load average 9-10 from other sessions throughout, hence the ranges.

hello (drive.mjs against serve.mjs):

| server | c=1 | c=16 | c=64 | RSS end |
|---|---|---|---|---|
| kiln native | 25.5k (18-29) | 71.2k (61-80) | 67.3k (67-79) | 27 MiB |
| node v25.9 | 27.2k (24-30) | 78.2k (37-79) | 70.7k (50-74) | 83 MiB |
| bun 1.4 | 23.7k (22-29) | 64.8k (60-79) | 67.9k (60-78) | 67 MiB |
| deno 2.9.6 | 25.9k (18-28) | 80.8k (73-84) | 74.3k (69-82) | 108 MiB |

mixed (drive-mix.mjs against app.mjs: routing, query, JSON, POST body, 8 KiB body, headers, async):

| server | c=1 | c=16 | c=64 | RSS end |
|---|---|---|---|---|
| kiln native | 21.5k (17-23) | 68.6k (38-69) | 62.2k (43-65) | 35 MiB |
| node v25.9 | 6.9k (6-7) | 70.8k (42-71) | 41.9k (42-70) | 96 MiB |
| bun 1.4 | 6.9k (5-7) | 52.7k (42-69) | 58.2k (45-68) | 85 MiB |
| deno 2.9.6 | 4.3k (4-4) | 46.7k (43-51) | 59.9k (41-61) | 120 MiB |

Every cell served with zero errors and zero wrong answers. Caveat on the mixed c=1 column: the
`/async` route awaits `setTimeout(0)`, which the JavaScript runtimes clamp to a real millisecond
while kiln's realm clock is virtual and fires it at the next turn (ADR 0012/0031); with one
connection that alone explains their 4-7k. At 16 and 64 connections the other routes dominate.
`Date.now()` also answers 0 natively for the same reason; a native `--node` server should
probably take the host clock, which is a decision for ADR 0031's successor rather than a bench fix.
