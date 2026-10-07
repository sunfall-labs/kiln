# Automatic-GC dc62 ClusterWorkflowEngine replay and profile

This receipt records an unchanged interpreter replay and a separate `samply` diagnostic replay of the pinned upstream `cluster/ClusterWorkflowEngine` file. The sampled run is diagnostic only; it does not replace the unsampled result or change per-case deadlines.

## Frozen inputs

- Source at release build start: `dc62a142e7d8397cd9e416d02ace7b7859dd3061`, clean worktree. A docs-only commit (`549118504e089449c504626c37cb9d4d006c4d76`) landed during the build; source diff was empty.
- Frozen CLI SHA-256: `bb0456a5a0c46f489a866caf8822f204be21247645f7e68ae6a46a6f03f09022` (`kiln-dc62a142`; binary intentionally not copied here).
- Build: durable unit `kiln-effect-autogc-dc62-cli-20261006`, `mbx build --release -p kiln-cli --offline --jobs 1`, exit 0, 158.639 seconds. The build receipt is `source.json`; the compiler log is omitted because the retained build log is available in the original scratch receipt and this evidence focuses on execution.
- Pinned Effect corpus commit: `67ba4e46a11ccda0b6761578bfd22c04ae00167`.
- Corrected harness SHA-256: `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`.
- Original per-case timeout remains 5 seconds; child memory cap 4096 MiB; one runtime worker; wall clock. The exact runner command and terminal receipt are in the raw logs/status embedded in `workflow-report.json` and `profile/run.log.gz`.

## Unsampled outcome

Durable unit `kiln-effect-autogc-dc62-cluster-workflow-20261006` exited 1 because the unchanged test file had 10 failures. All 47 cases were recorded: 37 passed, 10 timed out at the original 5-second limit, 0 skipped. The exact case IDs and one-to-one Node/Vitest mapping are in `workflow-report.json` and `workflow-run.log.gz`.

This matches the preceding 3ff interpreter run's 37/47 and ten timeout IDs; dc62 therefore showed no material improvement on this workload. This is a focused file result, not a full-corpus claim.

## Profile outcome

A separate `samply 0.13.1 record --save-only --rate 100` run wrapped the same corrected runner, frozen binary, input file, cap and case deadlines. Durable unit `kiln-effect-autogc-dc62-cluster-profile-20261006` completed with profiler exit 0; the test command itself recorded the same 37/47 result. The exact command, process IDs and elapsed time are in the profile status data copied into the compressed raw profile log.

The actual sampled CLI thread was PID 3419413 / TID 3419421, verified against `/proc/<pid>/exe` as the frozen `kiln-dc62a142` binary. It was registered for 153.879 seconds and had 10,299 samples at a requested 10 ms interval. Its recorded `threadCPUDelta` sums to 102.811 seconds. This is about 66.8% thread CPU over the registered span; it does not support calling the whole test fully CPU-bound. Symbolization covered every executable offset observed in samples (4,765 offsets).

Collector attribution should use unique samples containing collector frames, not inclusive frame counts that count nested frames repeatedly. 972/10,299 samples (9.44%) contain a `cycle_gc.rs` frame, representing 9.730 seconds (9.46%) of recorded thread CPU deltas. Collector self leaves are 425 samples (4.13%); prominent collector leaves include `collect_closure_cycles` (109 at line 530, 72 at line 556, 55 at line 508), `Owner::strong_count` (81 at line 166), and `object_children` (54 at line 1167). The earlier inclusive sum (1,427 frame hits) is reported only as a frame-hit count, not a percentage of runtime.

Other observed self leaves include `IntrinsicKey::eq` (252), `Value::clone` (242), `PropertyKeyRef::matches_classified_slot` (113), `Native::eq` (113), `kiln_string::Repr::clone` (112), and `ObjectData::lookup_key` (49). Inclusive stacks are dominated by interpreter execution/call frames (`run_bounded`, `call_value_inner`, `run_frames`, `invoke`, and event-loop turns). These samples point to broad interpreter call/property/clone work in addition to a measurable collector cost; they do not establish that collector work explains the additional timeouts.

Profile contents are in `profile/profile.json.gz`, raw profiler output in `profile/run.log.gz`, and the resolved sample summary in `profile/analysis.txt`. The profile was captured at 100 Hz but delivered 66.9 samples/second across the registered target thread, so treat it as useful attribution evidence rather than a cycle-accurate breakdown.
