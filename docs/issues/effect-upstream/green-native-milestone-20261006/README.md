# Green native Effect milestone

The finished production candidate at `f53e22c6d47ef0daad71fa915d4ec8c73d2d6bae` passed all ten routine gate steps in 1,087 seconds. The frozen CLI SHA-256 is `cdae5bcf3bd90f73412e0e0b3d35d0cefc8204fb388502476673d0e700ae6df2`; the tested code snapshot is `bb5ab6e123724d9fc8d5dc15ccea54b7077990d23182cb39ff6e47eca9f02ae5`. Subsequent merges and this archive change documentation only. The aggregate, complete gate streams, result documents and archive hashes are retained.

This milestone makes Date scalar getters and UTC clock setters avoid unnecessary calendar decomposition, preserves captured numeric intrinsics and setter ToNumber semantics, specializes native property-key hashing by string width, reuses GC store lookup results, and reads sealed callable stores before fallback. Native link keys include provider linker arguments, so provider choice remains correct without paying unrelated GUI startup costs. A bounded generic loop-callee O1 plan is shared across cyclic module phases without changing dynamic dispatch. Test counters are owned by their C fixture; CLI extension/actor/rejected-entry fixtures declare project boundaries.

Verification: 2,740 workspace tests passed, zero failed, 12 ignored across 110 suites. Native parity: all 667 probes accounted for, 662 matched, five existing explicit refusals, zero divergences. Third-party correctness: 17 present bundles matched in each engine, five absent explicitly reported. Node 40, server seven plus two skips, regress 58 and facts 31 all pass.

Routine Test262 passed all 122 chunk floors: 97,694 pass, 4,440 fail, 384 unsupported and four skipped out of exactly 102,522 selected variants. The 116 extended variants were deferred. This is a green routine baseline check, not complete language conformance.

All 12 routine third-party performance rows pass the unchanged policy; postcss is deferred. The separate owned builds were terminal and the global compiler process inventory was empty when the benchmark was resumed from its existing barrier; serial warmup/measurement took 4.36 seconds. The hold/resume receipt is retained. This is not the final 23-microbenchmark/four-realworld Bun comparison.

Focused combined Date acceptance already retained on main includes original Cron 43/43, Date Test262 1,166/1,188 with the same 22 baseline failures, five ordinary/stressed native probes, and selected 1/2/16-realm controls.

The full Effect goal remains active and incomplete. Latest original native Sharding replay is 61/66 with the same five five-second deadlines. Original Hash remains 24/25, and the older SchemaBinary replay 192/193. A separate Schema compiler candidate and the staged current ClusterWorkflowEngine binary await quiet original-case acceptance. Full passing 322-file/11,019-case native and interpreter surveys and the final quiet Bun benchmark comparison remain required.
