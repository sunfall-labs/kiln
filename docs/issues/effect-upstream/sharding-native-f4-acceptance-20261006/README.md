# Effect Sharding native replay on f4

The unchanged pinned Effect `cluster/Sharding` file recorded **61 passes, 5 failures, and 0 skips** out of 66 cases. All 66 cases mapped to the official Node/Vitest 5.0.3 inventory; there were no inventory or mode mismatches. The five outcome mismatches are exactly the five failures below, each an original 5,000 ms timeout also seen in prior native Sharding replays:

- `Sharding claim release regressions > keeps the claim of an active request replayed after an entity defect`
- `Sharding claim release regressions > keeps the claim of an active uninterruptible request after restarting`
- `Sharding claim release regressions > keeps the claim of an active stream after an acknowledged chunk`
- `Sharding > redelivers a request reset while an asynchronous storage read is pending`
- `Sharding > holds durable messages while entity layers are still building`

This is one-file evidence, not a full 322-file corpus claim.

The run used frozen source commit `f4a22dbbb9bd3df1a2322637d94629232229b5e6`, CLI SHA-256 `35fc543aea252fd0b1e8160b6f2871f5e58326f970905cb6836319e5746eddf2`, the pinned direct-TypeScript workspace and unchanged `Sharding` entry, native-release profile, provider ABI 82, one runtime worker, a 4,096 MiB runtime cap, and a 2,400-second outer runner allowance. The upstream per-case timeout remained 5,000 ms. Native binary SHA-256 was `626f34173ea8d388dadeeb9f7cefb784e2ee1595dc54c6ec691d7750a5dfdec7`.

The AOT stage took 1,122.57 s wall time and emitted 346,675,763 bytes of C / compiled a 315,830,072-byte application object (38.409 s emission, 1,078.203 s compile, 0.345 s link). During the replay the generated C was emitted again, while the app, prelude, runtime, launcher and final executable object/cache layers were hits; compile/link time was zero. The native runtime emitted all 66 results in 67.167 s including runner setup. Timings are retained as build/run receipts, not comparative benchmarks.

`evidence.tar.zst` retains the complete reports, all case output, build logs, job specs/statuses, frozen CLI and provider manifests, and entry digest. Executable binaries and build caches are excluded. The report records the corpus pin, harness/oracle/mapping digests, provider identity, CLI digest, runner digest and exact case outcomes.
