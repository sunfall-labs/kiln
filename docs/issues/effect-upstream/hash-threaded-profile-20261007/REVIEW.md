# Threaded VM Hash diagnostic

The frozen interpreter CLI d7d45c42 executed the 25 original direct-TypeScript Hash cases with unchanged harness and deadlines. The capture retained all exact case IDs and modes and mapped all 25 to the official Node/Vitest oracle. Under sampling it returned 21 passes and four timeouts, rather than the previous unsampled 24/25; this is diagnostic evidence, not a stable acceptance result or performance improvement.

Sampling quality passed: 3 per-thread events, 2 sampled TIDs, 3,228 user samples, 2,753 in the verified CLI image, no lost/unmapped/overrun samples. Counter/RUSAGE ratio is 1.0063 and period/counter is 0.9838. The process group completed and was reaped; all frozen input hashes remained unchanged.

Exact ELF STT_FUNC ranges attribute exclusive leaves. Thirty-eight CLI samples have overlapping symbol ranges and are explicitly classified as ambiguous; no nearest-symbol fallback or inclusive stack attribution is used. run_bounded_inner leads with 627 of 2,753 CLI samples, followed by string equality and object key lookup. The persistent/in-place HashMap case interval contains 1,384 CLI samples and spans approximately 35.5 seconds between completion markers. Synchronous work can delay a timeout callback; the 5-second deadline was not extended. This interval includes dispatch overhead and sampling effects and is not a benchmark duration.

Perf and supervisor clocks have a fixed offset on this host. The measured offset spread is 10.7 ms; interval analysis excludes 31.1 ms at each boundary. Full capture and exact attribution sources are retained for review. The diagnostic guides interpreter property/dispatch investigation while native remains the priority.
