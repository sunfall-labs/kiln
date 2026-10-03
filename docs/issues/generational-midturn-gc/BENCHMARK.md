# Mid-turn collector shrink benchmark

This records a matched, deterministic native measurement for the 1,000-element shrink workload used to diagnose allocation-triggered minor collection. The source is the temporary benchmark at the pinned Effect checkout, SHA-256 `a61378506dfea63f850798038d15af133e38d01d2076255d8bfd764371df5868`; Effect source revision `67ba4e46a11ccda0b6761578bfd22c04ae00167`. It constructs the same `Schema.Array(Schema.Int)` with min/max length 1,000, falsifies its first property evaluation, and asserts the result `Falsified:1001:0`.

## Matched native-dev measurements

All binaries use `native-dev` (`-O0`) and the same host/compiler (clang 22.1.8, x86_64 Linux). Each process does one unmeasured warmup and three timed runs. Three process invocations per engine were run serially in alternating order; the table includes all nine measured calls per engine. Every process exited 0 and every checksum was `Falsified:1001:0`.

| Runtime | Samples (ms) | Median (ms) | Range (ms) |
| --- | --- | ---: | ---: |
| Pre-generational kiln | 13335.624, 13340.498, 13219.541, 13255.271, 13230.811, 13157.374, 13146.523, 13162.429, 13083.632 | 13219.541 | 13083.632–13340.498 |
| Final GC kiln | 7216.814, 7231.017, 7215.823, 7263.912, 7365.383, 7262.702, 7309.579, 7313.111, 7295.982 | 7263.912 | 7215.823–7365.383 |
| Bun 1.4.2 | 81.708, 70.963, 69.856, 82.774, 69.059, 68.663, 86.524, 76.117, 72.321 | 72.321 | 68.663–86.524 |

On this `native-dev` measurement the final collector took 45.1% less time than the pre-generational binary (1.82x throughput). It remains roughly 100x slower than Bun. This is a diagnostic development-profile result, not a production `native-release` claim: the C toolchain maps `native-dev` to `-O0`, while `native-release` uses `-O3 -ffp-contract=off -fno-math-errno`. Release-profile measurements are pending separately.

Build time is excluded. The final native-dev executable came from CLI commit `9904979c` (CLI SHA-256 `86fc2622eb95b8760cf88007982b7d41c9acd4123b7aa79c197f559caaf2f906`); its one-shot build took 18.358s, including 16.735s emission, 0ms C compilation from cache, and 343ms link. The pre-generational CLI fingerprint is `8fa5201c98c06e33d4df04a1a12473be561e02b34ba15dc3e5b1d9de4dfcbf84` from the `42440934` source baseline. Executable SHA-256 values: pre-generational `9bb3cb648563e671ef3c4240dfa573785e3760bb1ecc05f783dce7d643d552b3`; final `57f0a271264401ac9d4cc50749087d6709fd3fd0afe270eef9a95c29903d2877`.

Raw process logs are retained in the owned worktree at `.scratch/shrink-bench/{pregen,final,bun}.{1,2,3}.log`; GDB evidence is `gdb-samples-final.log` in the same directory.

## Collector audit and focused tests

With the final `9904979c` CLI, `KILN_GC_MINOR_AUDIT=1` on pinned upstream `arbitrary/Arbitrary` reported `229 collections, 0 missing old-to-young edges`. The file had six 5-second upstream test timeouts in this diagnostic run; those were Date full-domain, Uint8Array, bounded integer/BigInt, cardinality, 10,000-node suspend, and 1,000 lazy child shrink. Pre-fastpath runs also timed out on the Date, cardinality, suspend, and lazy-shrink cases, with Uint8Array appearing in one run; timeout counts varied between runs, so they are not a claimed correctness improvement or regression. The edge-audit count is the relevant collector result.

After the active-range guard, `mise exec -- mbx test -p kiln-backend-c executable_minor_gc` passed all three focused C fixtures: tenured owner barrier insertion/reclamation, active young owner fastpath, and filled/open nursery cross-chunk promotion.

A GDB run sampled five calls to `kiln_c99_gc_collect_mid_turn` (hits 1, 25, 50, 75 and 100). Every sample entered through `gc_alloc_due`; callers were closure creation, owned cell creation, or object construction. This identifies allocation-triggered collection as the remaining broad cost but does not identify the hottest instruction inside the collector.
