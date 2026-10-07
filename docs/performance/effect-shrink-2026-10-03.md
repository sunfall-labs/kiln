# Effect shrink benchmark, 2026-10-03

This records the isolated production `native-release` comparison of the baseline and final
collector on the same Effect shrink workload. It measures the production policy; it is not an
all-O3 comparison. `native-release` keeps the prelude at O3, but large generic source modules are
compiled at O0 while specialized modules use O2. The workload source was
`~/.cache/kiln/effect-gc-minor-diagnostic/files/shrink-bench/shrink-benchmark.ts`
(SHA-256 `a61378506dfea63f850798038d15af133e38d01d2076255d8bfd764371df5868`). Both native artifacts
and Bun reported `SHRINK Falsified:1001:0` on every invocation.

| Runtime | Artifact SHA-256 | Nine measured samples (ms) | Median |
| --- | --- | --- | ---: |
| Baseline native, `56ebd92e` + DCE fix | `277cbe7e91154eca5bef94a551f8048e0fa04fb079204d241b58c1bbf2ad91bb` | 4107.274, 4113.827, 4128.075, 4117.697, 4144.309, 4130.575, 4142.223, 4135.159, 4147.699 | 4130.575 |
| Final native, `0b3a2df4` | `21781cd5a76639964975782aa591735f6770e6214cc2ed6c98fb2de6541d09b5` | 2262.873, 2268.506, 2246.733, 2279.755, 2298.724, 2261.760, 2267.221, 2273.942, 2247.975 | 2267.221 |
| Bun | Bun 1.4.2, source above | 84.640, 70.296, 68.790, 80.407, 71.809, 70.228, 78.856, 70.040, 68.839 | 70.296 |

The final native median is 45.1% lower than baseline (1.822x faster). It remains 32.3x slower
than Bun on this workload. Each process warmed once and then emitted three measured samples; the
nine invocations were alternated across engines. Raw output is retained in
`.scratch/paired-runtime/` in the measurement worktree.

The source worktrees were isolated: baseline tree `4439a70b1cf7d6f7bcc4033e969a7822711d399f`
at `56ebd92e97efba4812bb5ffcc585643e6379ca3b`, final tree
`56dc29de7999bfac635422384f2bb5723926d91a` at `0b3a2df4aba36ca95dc2d8d4544892308c61189a`.
Both include DCE fix `daa420b1d25c39a6d54a5d96b2a8a0a77949b5`. The source tree delta isolates the
collector change.

The `native-profile` mode cannot substitute for this release policy: it changes code generation
and compact emission as well as adding `-pg`, and instruments all generated modules at O3. A
symbolized profile build was started to test gprof attribution, but stopped after about 19 minutes
when this policy difference became clear. Its partial objects were not used for performance
conclusions.

Both AOT programs were built with `--profile native-release` from the same source. The baseline
build took 274s (emit 91.661s, C compile 178.381s, link 0.179s) and built its generated C, app
object, launcher object and prelude object. The final build took 110s (emit 89.714s, C compile
17.752s, link 0.236s); generated C, app object, and launcher object were cache hits, while the
prelude object was rebuilt. Thus the wall-time difference is cache-asymmetric and is not evidence
of faster final compilation. Both executables are 107,182,360 bytes. The release CLI hashes were
baseline `f0f52fee30cce988a8f75f5ce8364728a12882429950ebc0366d43dfcdd53788`, final
`cd6b6dc9f8b6d632aec009a794defdd5d956a9fd1528cbeec72cd0fc977307fd`.

## Hot-path samples

The shipped artifact is stripped and this environment denies `ptrace` attach. For symbolization, I
made a separate `native-release` link from the cached production generated-C and app objects, with
only final symbol stripping disabled. The unstripped executable SHA-256 is
`655369bd07b22252676696be867e5daa5861d2dade2d8ecc3efd7aaf7914a8df`; its `.text` SHA-256 is
`b6106943e13498f9c3479e78483af0e1b3a89376793eba51f4b1785e64ab441f`, identical to the measured
stripped release artifact's `.text`. The AOT build report showed generated C, app object, prelude,
runtime and launcher objects were all cache hits; only the executable link was rebuilt.

Five asynchronous GDB interrupts landed in these stacks:

1. `kiln_c99_gc_weak_ephemerons` → `kiln_c99_gc_collect` → `kiln_c99_gc_collect_mid_turn` →
   `kiln_c99_val_make_cell_owned` → generated `internal/arbitrary/model.ts` functions.
2. `kiln_c99_val_write_element` → generated workload code → closure invocation and construction.
3. `kiln_c99_val_make_cell_owned` → `kiln_c99_val_call` → generated arbitrary-model functions.
4. `kiln_c99_call_inner` → `kiln_c99_construct_with_new_target` → generated arbitrary-model code.
5. `kiln_c99_gc_drain` → `kiln_c99_gc_collect` → `kiln_c99_gc_collect_mid_turn` →
   `kiln_c99_val_make_cell_owned` → generated arbitrary-model functions.

Three of five snapshots were in collection or cell allocation, with the two other snapshots in
array element writes and construction/call dispatch. This points to collection and allocation as
the next area to inspect, but the sample is too small and intrusive to estimate their relative
costs: GDB paused execution, and measured in-program samples ranged from 3.1s to 17.3s compared
with about 2.3s without GDB. All completed runs retained `SHRINK Falsified:1001:0`. The finding is
qualitative; a lower-overhead sampling profiler is needed before choosing a specific optimization.
