# Corrected native milestone gate

The frozen candidate at `d8457099`, CLI SHA-256 `cdae5bcf3bd90f73412e0e0b3d35d0cefc8204fb388502476673d0e700ae6df2`, completed its routine gate in 1,625 seconds. The gate **failed only crate tests**: 2,738 passed, two failed and 12 ignored across 110 suites. Both actor fixtures scanned a shared temporary ancestor instead of their own project. A follow-up package-boundary fixture fix passes both actor and both rejected-entry tests; the production milestone still awaits a green gate.

Native parity completed every probe: 662 matched, five existing explicit refusals, zero divergences. Third-party correctness matched all 17 present bundles in each engine; five absent bundles remain explicit. Node 40, server seven plus two skips, regress 58, and facts 31 all pass.

Routine Test262 passed every chunk floor with 97,694 passes, 4,440 failures, 384 unsupported and four skipped across 102,522 selected variants; 116 extended variants were deferred. This is routine coverage, not full conformance.

All 12 routine third-party performance rows pass the unchanged policy; postcss is deferred. The timed execution window ran after correctness lanes finished, with separate owned compiler groups suspended for 4.6 seconds and then resumed. The signal receipt records all resumes. This is not the final microbenchmark/real-world comparison against Bun.

Complete gate envelopes, aggregate, compressed raw logs, result documents and archive hashes are retained. Production changes are unlanded. The original full passing Effect native/interpreter inventories and final Bun comparison remain unfinished.
