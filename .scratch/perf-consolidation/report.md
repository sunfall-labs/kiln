# Consolidation performance record

This record is derived from the retained raw results in
`.scratch/perf-consolidation/http-paired/` and `.scratch/perf-consolidation/microtasks-paired.json`.
The HTTP pair used baseline `b9bbb589` and candidate `100c7c7b` on an Apple M5 Pro (`Mac17,8`,
18 logical CPUs), Node `v25.9.0`, native-release binaries, one server worker, four client
processes, sixteen connections per client, three interleaved five-second passes. Rates are sums across client processes; the table reports the median of the three pass-level sums.

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
median of `0.5078125 ms`, while Node reported `0.4375 ms`. The complete raw records and ordering are in `.scratch/perf-consolidation/microtasks-paired.json`; compact combined evidence is in `.scratch/perf-consolidation/results.json`.

The HTTP runner starts a fresh server for each row, waits for readiness, then measures the five-second load; process startup is excluded from the rate, but there is no separate HTTP warmup phase. The microtasks harness explicitly warms before reporting its internal median. These results do not claim a general scaling result. The method-dispatch metadata change is
native-backend metadata generation; it does not unify interpreter dispatch semantics. Shape and
multi-worker scaling remain separate questions.

The binaries were measured before the final integration rebase. Subsequent upstream compiler/provider/linker changes, including `440a4336`, are unmeasured here.

Validation status: focused harness and corpus checks passed; the final full gate remains pending.
