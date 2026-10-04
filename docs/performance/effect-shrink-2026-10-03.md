# Effect shrink benchmark, 2026-10-03

This records the isolated O3 comparison of the baseline and final native collector on the same
Effect shrink workload. The workload source was
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

Both AOT programs were built with `--profile native-release` from the same source. The baseline
build took 274s (emit 91.661s, C compile 178.381s, link 0.179s) and built its generated C, app
object, launcher object and prelude object. The final build took 110s (emit 89.714s, C compile
17.752s, link 0.236s); generated C, app object, and launcher object were cache hits, while the
prelude object was rebuilt. Thus the wall-time difference is cache-asymmetric and is not evidence
of faster final compilation. Both executables are 107,182,360 bytes. The release CLI hashes were
baseline `f0f52fee30cce988a8f75f5ce8364728a12882429950ebc0366d43dfcdd53788`, final
`cd6b6dc9f8b6d632aec009a794defdd5d956a9fd1528cbeec72cd0fc977307fd`.

## Hot-path sampling attempt

The final AOT file is stripped, and this environment denies `ptrace` attach (`Operation not
permitted`). Interactive GDB interrupts while running under the debugger produced address-only
stacks, including a libc wait and native runtime dispatch frames. GDB materially perturbed execution
(one workload took about 24s under instrumentation versus about 2.3s normally), so those samples
cannot establish representative hot-path proportions. No hotspot attribution is claimed from
them. A useful next profile needs a symbolized/unstripped AOT artifact or an in-process sampling
profiler; repeated entry-breakpoint counts would not answer the CPU-cost question.
