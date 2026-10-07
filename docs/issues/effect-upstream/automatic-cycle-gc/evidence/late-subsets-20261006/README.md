# Automatic-cycle-GC interpreter subset replay

These are unchanged upstream interpreter cases run against the frozen automatic-cycle-GC CLI after the complete `Arbitrary` replay. All commands used one runtime worker, the pinned corrected runner/harness, a 4096 MiB child address-space cap, wall clock, and original per-case deadlines. No case inventory, assertion, or timeout was changed.

- Candidate source: `ea95686fe704f60676b45d4ce0dad6d1c59a56e5`
- Candidate CLI SHA-256: `3ff14d18c5e4f9f6fe8097f7289641629f55dd8f77fc60d6a2dfb5c85afe1021`
- Pinned corpus: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`
- Corrected harness SHA-256: `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`
- Candidate runner commit: `f1080bebd50cfbcbbe79109036548d91ae9313c0`; timeout policy remains the harness's original 5-second case budget.
- Main subset unit: `kiln-effect-autogc-late-subsets-20261006`, exit 0 after collecting results (the selected test process failures are recorded per file), 496.27 seconds; 17 files, 442/442 cases recorded, 420 pass, 22 fail, 0 skip.
- Additional file: `schema/toDifferJsonPatch`, unit `kiln-effect-autogc-todifferjsonpatch-20261006`, 10/10 recorded, 9 pass, 1 fail, 15.25 seconds.

The baseline is the prior full interpreter survey at source `efd9e68a8920d764ccc7df1f244d7591d74794d6`, CLI SHA-256 `148946a23bb17b60b6d1f5907364c8950c752992c09949d1eeb287b3bdec4d4b`, with the same corpus, source file digests, harness, original deadlines and 4096 MiB cap. Per-file candidate and baseline JSON/raw logs are retained below.

| File | Baseline | Candidate | Failure comparison |
| --- | --- | --- | --- |
| `Hash` | 24/25, 56.05s | 24/25, 53.03s | Same HashMap persistent/in-place edit timeout |
| `Cron` | 41/43, 15.24s | 42/43, 14.31s | Forward/reverse sequence timeout disappeared; impossible day/month `prev` timeout remains |
| `encoding/SchemaBinary` | 191/193, 81.46s | 191/193, 83.07s | Same two original timeout cases |
| `cluster/ClusterWorkflowEngine` | 40/47, 103.41s | 37/47, 153.86s | Same seven prior timeouts plus three new timeouts |
| `cluster/Sharding` | 59/66, 119.58s | 58/66, 130.14s | Same seven prior timeouts plus one new timeout |
| `schema/toDifferJsonPatch` | 9/10, 13.69s | 9/10, 15.25s | Same property-based roundtrip timeout |

The three additional ClusterWorkflowEngine timeouts are `resumes when children complete during activity cleanup` and the two `resumes a fresh entity from a legacy envelope with queued replay` cases (`false` and `true`). The additional Sharding timeout is `durable streams are resumed on restart`. These are potential throughput regressions and require profiling before attributing them to automatic GC.

Other selected cluster files completed without failed cases. The aggregate report records all selected IDs and mappings. This is a focused subset comparison, not a full corpus run.
