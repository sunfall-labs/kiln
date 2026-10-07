# Native resource-policy validation

Source `3810b913`, frozen compiler SHA-256 `f1c8f22d171e209a929842563e5dd35f2a2226d9c363486497665bcde9189de0`, pinned Effect `67ba4e46a11ccda0b6761578bfd22c04ae00167d`. Both resource options were enabled on the real upstream Boolean file. Native and Bun each discovered and passed all 43 cases, with identical inventories, modes and outcomes; all 43 native cases mapped to the official Node/Vitest oracle. The report explicitly defers the other 321 files.

The native executable was compressed by a single-thread zstd process and its decompressed bytes were verified against the executed binary digest before raw deletion. Root independently rechecked the published archive SHA-256 and the recorded binary digest. The archive remains in the owned run output. Cache GC swept the same first writable shared cache used by builds, `effect-generation-reset-validation/.kiln/cache`; its log reports 489.1 MiB across 478 records. Its ordinary grace protection and 2 GiB budget remain in force.

All 28 Python runner unit tests pass. Options default off; upstream tests, assertions, registered cases, skip modes and deadlines are unchanged. This is resource-control integration evidence, not a full upstream run or a benchmark timing result.
