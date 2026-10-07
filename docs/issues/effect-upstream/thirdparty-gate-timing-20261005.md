# Third-party timing gate replay, 2026-10-05

## Finding

The `dayjs` and `marked` performance regressions reported by frozen gate
`run.dFJrjxL9` did not reproduce in a quiet replay. The gate reported dayjs
11.62 ms against 7.61 ms (+52.7%) and marked 14.77 ms against 11.45 ms
(+29.0%). With no overlapping builds or test runs, their medians were 9.16 ms
and 12.77 ms. Those are residual increases of 1.55 ms and 1.32 ms, below the
third-party policy's 3.0 ms minimum absolute delta; neither qualifies as a
regression under the unchanged 25% / 3.0 ms policy.

This supports contention as the cause of the gate's two regression marks. It
does not establish a universal speedup or rule out a smaller performance
change. The gate log shows the performance measurement began after a 1,942.46
second release-barrier wait, with correctness work running beforehand.

## Frozen inputs and policy

- Frozen integration source: `4b5e6c9cc68661f19599ec02ed665e9a47379270`.
- CLI: `734d4a745f4beea64662d94fb87dcb82fd897600ed584056bff1da30941ba70a`.
- Profile: `native-release`; timezone: `UTC`.
- Harness policy copied from `scripts/thirdparty.sh`: 3 warmups, then 21
  timed process executions per bundle, serially; each run's exit status,
  stdout, and stderr had to match Node.
- Regression thresholds remained 25% and 3.0 ms. No baseline was updated.
- Dayjs source SHA-256:
  `2a33388f7530caf4578f5b5fe329175b401396aac2bae43921d41d73001e0c6c`.
- Marked source SHA-256:
  `e39b413c126685ca22512df10ad2ecf9450ccbbbe615edb4ec8a36f81904ca61`.
- Rebuilt dayjs executable SHA-256:
  `c9f00dc1cddb5ef9626a8949a9cb67aeac3aa15ec1903c568aae39bef9db3243`.
- Rebuilt marked executable SHA-256:
  `d6bb6168556e17443899af30b03dd36636b2e4e6409497946b963acb9cb14ae0`.
- Both workloads exited 0 and matched Node output. Dayjs stdout SHA-256:
  `8f7519d6cbabeb0afc895c87339ac3975f177957ffdf583bb84ff85c4301edf0`;
  marked stdout SHA-256:
  `df33f3abe627854e2f1f8ee410eda035deb5e707a0279ae4ee55769697e93a6a`.
  Both stderr streams were empty.

The native number-provider archive used for the earlier old/new-provider
comparison was byte-identical (SHA-256
`61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`). This
rules out changed provider archive contents as the source of the timing delta.

## Raw quiet samples

Samples are milliseconds, in execution order. The median and minimum are
reported above; individual values are retained here to make the replay
auditable.

```text
dayjs: 11.1101, 9.0477, 9.0842, 10.2673, 8.9941, 9.2680, 9.3325,
       9.4622, 9.1289, 8.8999, 9.1556, 9.4989, 9.1359, 9.7818,
       9.6421, 9.0776, 9.2097, 9.7757, 9.0700, 8.8372, 8.8073
marked: 12.5721, 13.3499, 12.8749, 12.7306, 12.4856, 12.4524,
        12.6573, 12.7901, 12.5838, 12.8372, 13.1277, 12.8939,
        12.6272, 12.7680, 12.8106, 13.2017, 13.0285, 12.8391,
        12.7262, 12.5382, 12.5908
```

The complete machine-readable records, including every sample, executable
hash, output hash, source hash, and CLI hash, are preserved at
`.scratch/thirdparty-quiet-20261005/quiet-timing.json` in the frozen
integration worktree. The two rebuilt executables and their build logs are in
the same directory. The gate's original log is
`.scratch/gate-push-milestone/evidence/run.dFJrjxL9/01-thirdparty_baseline.log`.
