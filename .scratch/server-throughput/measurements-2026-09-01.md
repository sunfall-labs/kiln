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
