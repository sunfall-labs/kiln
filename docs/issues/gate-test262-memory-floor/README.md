# Preserve single-case Test262 memory at small CPU budgets

The full gate at source `fda0e625`, with `KILN_GATE_JOBS=4`, assigned one Test262 worker and a 1,024 MiB process cap. Batch 0002 aborted while evaluating the generated block source in `staging/sm/regress/regress-610026.js`; its log reports a failed 167,772,120-byte allocation. `coredumpctl info 105559` identifies SIGABRT in Rust allocation failure, with Oxc SemanticBuilder under parse_eval in the corpus worker. The kernel journal reports no OOM kill for this interval.

Evidence is retained in `target/gate-logs/run.2IeXJmv4/03-test262_baseline.log.envelope.json` and `target/test262-baseline-logs/run.XTwnuRDz/batch-0002.log` in the owned frozen gate worktree. The envelope correctly reports infrastructure failure, not conformance counts or a passing gate.

A CPU allowance bounds concurrent cases; it cannot guarantee how much memory a single case needs. The gate now keeps the standalone runner's 4 GiB default at small CPU budgets, while preserving the half-physical-memory and 16 GiB ceilings. An explicit memory override remains authoritative. Test262 worker count is still limited by its assigned CPU workers and memory cap.

The five budget unit tests pass, including one-worker allowance, low-physical-memory ceiling, explicit overrides and aggregate failure handling. The complete gate with the new memory allowance remains required; this unit evidence does not establish complete Test262 coverage.

## Failed-batch rerun

Using the same frozen release binary with one worker and a 4,096 MiB cap, all 14 variants in batch 0002 completed in 28.43 seconds without an allocation abort or RSS-cap violation. Both `regress/regress-610026.js` variants passed. The batch contains four passes and ten pre-existing step-budget failures; a case failure is recorded as a verdict rather than losing the entire batch document. The receipt, full case results and raw log are retained in `evidence/`.

This rerun validates the memory allowance against the allocation-failing input. It does not replace the required fresh full gate or establish its chunk baseline verdict.
