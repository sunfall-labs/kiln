# Runtime follow-up checks (2026-10-07)

The combined clean source `642243f98ba01b321dfe25a2589735e6f5a0aa4e` built successfully in 272.06 seconds. Frozen CLI SHA-256: `0320d657acbe69fba302a30d509f6540b4b58e9187b9da9cda9d8b6dc8ce474a`. Complete build log and supervisor provenance are preserved.

The new Array.fill differential probe matched under kiln, Node v26.8.1 and Bun 1.4.2, all exit 0 with identical stdout SHA-256 `6bdfae5d469967ec9571ce82a7f6ca7a9d1080c30fb3bfd4e67a724eb27fe9e1`. The repository runner reports 1 matched, 0 refused, 0 diverged. This covers ordinary dense holes, inherited setters and permissions, and prototype changes during argument coercion; it is one probe, not full conformance.

Actual upstream Sharding `kiln check --node --profile native-release --explain-parallel` completed in 39.74 seconds and emitted the real native module graph without compiling C. The Effect internal module payload is **identical to the older pinned emission** (`40abce548e10301f3b35c1160673b475767e63009749b712f60d0fbbe0ee00fc`). Function 120, the computed evaluate method established in the separate source proof, still has O0/optnone attributes. Thus the computed-key ranking unit tests do not prove that its intended upstream optimization is selected. Investigation continues; no native performance improvement or fresh Effect acceptance is claimed here.

`actual-module-proof.json` records the exact new artifact identity and complete declaration attributes. The compressed generated C was round-trip checked against its recorded SHA-256. This archive preserves the failed optimization check rather than silently treating fixture coverage as upstream coverage.
