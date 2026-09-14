# Consolidation performance record

This record is derived from the retained raw results in
`.scratch/perf-consolidation/http-paired/` and `.scratch/perf-consolidation/microtasks-paired.json`.
The HTTP pair used baseline `b9bbb589` and candidate `100c7c7b` on an Apple M5 Pro (`Mac17,8`,
18 logical CPUs), Node `v25.9.0`, native-release binaries, one server worker, four client
processes, sixteen connections per client, three interleaved five-second passes. Rates are sums
across client processes; each client reports its own percentiles, and the table uses the median of
those per-client aggregates rather than pooled request percentiles.

| workload | baseline req/s | candidate req/s | Node req/s |
| --- | ---: | ---: | ---: |
| large-upload | 1,973 | 21,029 | 22,530 |
| json-echo-64k | 5,007 | 12,266 | 19,026 |
| hello | 178,887 | 177,352 | 125,809 |
| mixed | 114,299 | 112,924 | 101,614 |

All 36 HTTP aggregates completed with zero errors and zero wrong responses. The raw metadata records
source refs, transformed source hashes, driver hashes, and native binary hashes.

The interleaved microtasks run used baseline `b9bbb589`, candidate `100c7c7b`, and Node `v25.9.0`.
All nine runs produced checksum `20000`; baseline and candidate both reported an internal warm
median of `0.5078125 ms`, while Node reported `0.4375 ms`. The complete raw records and ordering
are in `.scratch/perf-consolidation/microtasks-paired.json`.

These measurements compare warmed execution only. They do not compare launch time or cold JIT
startup, and they do not claim a general scaling result. The method-dispatch metadata change is
native-backend metadata generation; it does not unify interpreter dispatch semantics. Shape and
multi-worker scaling remain separate questions.
