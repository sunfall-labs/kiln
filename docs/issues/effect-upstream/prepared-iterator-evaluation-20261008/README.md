# Prepared iterator call evaluation

Candidate `b5f04b05386e37d60bf125b015d21f3ed7038dd7` removes temporary argument arrays from captured iterator `next` calls, preserving the captured method and receiver. Frozen CLI SHA256: `49ea63bbe6534c8953d629e487e41ca9234da3b103773557da9f2080e809cd71`. This candidate is not adopted by this evidence commit.

| Check | Observed result |
| --- | --- |
| Interpreter Hash | Complete original 25-case inventory: 24 pass, 1 fail |
| Native Sharding | Complete original 66-case inventory: 62 pass, 4 fail |
| Native ClusterWorkflowEngine | Compilation timed out after 1200.08 seconds; no cases executed |

Hash and Sharding failures match the production CLI's exact failing IDs and original `TEST_TIMEOUT: 5000ms` errors. Their inventories map to the official Node/Vitest IDs and modes; outcomes diverge from its passing results. Per-case elapsed timings are unavailable. This run proves neither a performance gain nor complete native 113-case coverage. It is not the final whole-suite acceptance run.

The native runner returned with exit 1. The outer supervisor stopped report validation on Cluster's missing inventory; its original failed status is retained. It did not record native process-group absence, which remains unverified. The postflight receipt separately captures both native envelopes and the interpreter result.

The launch supervisor erroneously digested a nonexistent harness directory, yielding the empty digest. The original receipt is preserved. Supplemental during-run and postflight evidence compares the actual runner support source against the staged copy: all 16 copied files match (`bd52edd86f51529a33f99b4852a1571da740087c66136ac3ebc958c315bbe1e1`). This correction is supplemental evidence, not a claim that the original preflight checked the right directory.

Upstream pin, frozen CLI and Full provider identities are in the postflight receipt. Case and hook budgets were unchanged. The native compile limit is separate from case deadlines. The retained binary was archived and roundtrip-verified; binary and archive are excluded here. The file manifest authenticates the copied result envelopes and original receipts.
