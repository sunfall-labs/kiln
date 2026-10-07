# Consolidation performance record

This record is derived from the retained raw results in
`docs/issues/perf-consolidation/http-paired/` and `docs/issues/perf-consolidation/microtasks-paired.json`.
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
median of `0.5078125 ms`, while Node reported `0.4375 ms`. The complete raw records and ordering are in `docs/issues/perf-consolidation/microtasks-paired.json`; compact combined evidence is in `docs/issues/perf-consolidation/results.json`.

The HTTP runner starts a fresh server for each row, waits for readiness, then measures the five-second load; process startup is excluded from the rate, but there is no separate HTTP warmup phase. The microtasks harness explicitly warms before reporting its internal median. These results do not claim a general scaling result. The method-dispatch metadata change is
native-backend metadata generation; it does not unify interpreter dispatch semantics. Shape and
multi-worker scaling remain separate questions.

The binaries were measured before the final integration rebase. Subsequent upstream compiler/provider/linker changes, including `440a4336`, are unmeasured here.

Validation status: focused harness and corpus checks passed; the final full gate remains pending.

## Final validation (2026-09-14, individual stages complete)

The final gate's ten individual stages all completed successfully. Its aggregate deliberately refused to produce a green verdict because tracked documentation changed while the gate was running, so the records came from mixed code snapshots. Root will rerun the aggregate on the frozen tree.

- Crate tests: 1,809 passed, 7 ignored, 0 failed.
- Native parity: 199 matched, 1 expected `shadow-realm` refusal, 0 diverged.
- Third-party corpus: 22 interpreter matches and 22 native matches; 0 diverged and 0 native refusals.
- Third-party timing baseline: 15 bundles measured, 4 gains, no regressions. Final YAML was 4.89 ms versus 5.08 ms baseline (-3.7%); Zod was 7.86 ms versus 15.52 ms (-49.4%).
- Facts baseline: all 31 targets passed (14 benchmarks, 17 bundles).
- `conformance/node --node`: 36/36; `conformance/server --node`: 8/8; `conformance/regress --node`: 15/15.
- Test262: no regressions across 122 chunks; 96,776 / 102,638 passing, with five chunks gained.

The earlier focused YAML result (+47.2%, +2.40 ms) remains in the raw history; the final 15-bundle timing run above is the current measurement. The percentage increase was recorded plainly and remained below the harness's 3.0 ms absolute-delta threshold when it occurred.

The controlled HTTP pair in the retained performance report remains explicitly pre-rebase evidence: baseline `b9bbb589` versus candidate `100c7c7b`, before later upstream compiler/provider/linker changes. It is not a measurement of the final rebased tree. The later setEncoding object/string regression was fixed through the existing text path; incremental decoder behavior remains the existing limitation.
