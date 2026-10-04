# Direct-TypeScript benchmark milestone validation

Status: ready-for-agent

The six-file benchmark milestone improves native filesystem AOT execution, fresh child RSS attribution, startup provenance, and shared HTTP keepalive/checksum validation. Its production compiler and native runtime sources are identical to main `340a7d88`. Eight Python measurement tests and the Node keepalive fixture pass.

## Required result

Retain a passing performance check before calling the milestone fully validated. Do not change the committed timing baseline to conceal an unexplained slowdown.

## Evidence

The complete gate at `6f4ffa2d` failed only the thirdparty performance lane. All nine correctness lanes passed: 2,577 crate tests, native and thirdparty parity, the Node/server/regress corpora, facts, and routine Test262 (102,522 selected variants, no chunk regression; 116 variants deferred). The aggregate remains FAILED.

An isolated physical-core candidate replay still failed 11 of 12 timing rows. Replaying the same driver with the frozen main compiler also failed those 11 rows. This establishes that the six-file benchmark change did not introduce that difference; it does not prove the committed baseline is obsolete or establish acceptable current performance.

Briefly suspending the owned background workers reduced failures to two rows, dayjs and marked. The measurement was only partially quiescent: a separately launched root TxHashSet compiler used a cache-directory working directory outside the suspension census. No quiet final comparison is claimed. All suspended processes and original affinities were restored with zero restoration errors. The records name suspended process identities and timing intervals.

`reference.json` binds the complete gate aggregate and the three followup logs and affinity journals by digest. These are diagnostics, not accepted performance improvements. The next comparison must exclude all competing goal workers during measurement, after preparing artifacts, and retain exact CLI and workload identities.
