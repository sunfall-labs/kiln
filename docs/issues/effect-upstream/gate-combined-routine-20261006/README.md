# Combined recovery routine gate

Source commit: `9b6d7041725462c70784d6f1234ba815f04a0009`. The exact tracked-source digest and frozen CLI digest are in `aggregate.json`. The process is terminal, exit 1, after 3,560 seconds.

Correctness lanes passed: 2,730 crate tests across 110 suites, zero failures (11 ignored); native parity 659 matches, five named refusals, zero divergences; third-party native parity 17 matches, zero refusals/divergences; Node 40 passes, server seven passes/two skips, regress 58 passes; whole-program facts baseline passes. Routine Test262 has 97,692 passes of 102,522 selected variants, 116 explicitly deferred, and no regression in any of 122 chunks. This is routine coverage, not full Test262.

The overall gate **failed** only the third-party performance baseline. All 12 routine bundles regressed, with approximately 46 ms of added wall time on the smallest bundles. The build used the full-GUI provider. Provider/link/startup effects are being investigated; no performance baseline is updated or waived. These timings also overlap independently owned agent jobs, so they are not final quiet performance acceptance.

All gate evidence files are retained here verbatim, including the complete raw crate stream and parsed envelopes. This gate does not prove all upstream Effect cases pass: the Effect goal remains open, and newer Date, GC and Schema candidates are isolated from this source snapshot.
