# Combined native candidate gate

The production-equivalent clean `aee1648a`/`6e74e6ad` candidate CLI has SHA-256 `5c968ff32c38a8175754413088d5e62da0e39eefc2615750f1201124a99a806e`. Its completed routine gate **failed**; production remains unlanded. The aggregate, step envelopes, raw logs, result documents and binary receipt are retained here.

Routine Test262 passes its chunk floors: 97,694 pass, 4,440 fail, 384 unsupported and 4 skipped, across exactly 102,522 selected variants. There are 116 deferred extended variants; this is not a full Test262 run. Node (40), server (7 plus 2 skips), regress (58), and whole-program facts (31 targets) pass.

All twelve measured routine third-party bundles pass the unchanged performance policy (25% and at least 3 ms). The provider is selected from installed Full and FullGui manifests with both provider overrides unset; GUI dependencies no longer impose the earlier approximately 44 ms startup penalty. Postcss is deferred. This is not the final 23-microbenchmark/four-realworld comparison against Bun.

Crate tests report 2,452 passed, 4 failed and 10 ignored. Two failures concern test-counter ownership and the reviewed private-runtime ABI snapshot. The follow-up fix moves counters entirely into the C fixture, retaining lookup and retention assertions. The two extensions-off failures require separate diagnosis: subsequent serial execution passes, while parallel execution reproduces cross-fixture source paths even after space recovery.

Native parity reports 646 matched, 5 refused, 12 diverged and 4 invalid builds. Every one of the twelve divergent native output streams contains an explicit `No space left on device` error, decoded from the runner's exact captured hex in `native-enospc-decoded.json`. One invalid build explicitly reports disk exhaustion; three linker failures occur in the same exhausted interval. These are failed checks, not waived passes. Third-party correctness also fails with an empty log after exhaustion. Affected probes are being rerun after recovery.

The gate's cache reached over 20 GB while recent records were protected by its ordinary grace period; generated native output binaries occupied a further 25.47 GB logically. After the gate and its writers were terminal, only owned regenerable cache records and conformance binaries were reclaimed. No other worktree's outputs were removed. The removed generated binaries' file inventory is retained. Future validation must have adequate storage and preserve complete evidence.

This record does not prove the full Effect objective. The original SchemaBinary replay remains 192/193 with the same five-second zero-width-array deadline failure; full passing native and interpreter Effect inventories and the final quiet Bun comparison remain outstanding.
