# Combined constructor parity

Frozen CLI SHA-256 `8ac442d8270d0f76ea7933acef5c9812b24f1d9a1f537fbd98f25e7e43515867`, production source `79a05698`.
Provider archive SHA-256 `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`.
One worker; ordinary parity: 2 matched, 0 refused, 0 diverged.
`oxc-using-context.mts` declares the native-release profile; `module-namespace-slot-refresh.mjs` used default dev.
GC-stressed constructor parity: 1 matched, 0 refused, 0 diverged, 0 skipped.
This is focused constructor/rooting verification, not a full native stress corpus or a full Effect suite.
The namespace ordinary result predates adding descriptor-before-Get coverage for the separately reproduced interpreter stale-descriptor bug.
