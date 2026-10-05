# Follow-up integration gate (failed)

The source remained frozen at `4f39b9dc` throughout this complete routine gate. The unchanged
aggregate, envelopes, result documents and raw logs are in the archive; its receipt records the
hash. The gate finished in 1139 seconds with a **failed** verdict.

Workspace tests: 2655 passed, zero failed, nine ignored. Routine Test262: 97617 passed out of
102522 selected variants, zero regressions across 122 chunks, 116 deferred variants. Thirdparty
correctness, the Node/server/regression corpora, and whole-program facts passed.

Native parity: 646 matched, five explicit refusals, one divergence. The actor-transfer problems
from the previous gate are resolved. `has-own-property-borrowed` exposed an interpreter fallback
that asks inherited membership for generator `hasOwnProperty`; the native answer agrees with
Node. Its source correction is tracked separately. Dayjs measured 10.74ms against the 7.61ms
baseline and crossed the unchanged timing thresholds; marked did not cross both thresholds.

This evidence proves neither a successful gate nor complete Effect acceptance. Subsequent
corrections and quiet timing measurements do not change this recorded verdict.
