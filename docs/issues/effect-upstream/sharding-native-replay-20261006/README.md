# Sharding native replay

This is the unchanged pinned upstream `cluster/Sharding` native replay on the frozen 5c CLI. The official harness recorded all 66 cases with no skips or inventory gaps: 61 passed and five failed with the original per-case `TEST_TIMEOUT: 5000ms`.

The five timeout IDs were:

- `Sharding claim release regressions > keeps the claim of an active request replayed after an entity defect`
- `Sharding claim release regressions > keeps the claim of an active uninterruptible request after restarting`
- `Sharding claim release regressions > keeps the claim of an active stream after an acknowledged chunk`
- `Sharding > redelivers a request reset while an asynchronous storage read is pending`
- `Sharding > holds durable messages while entity layers are still building`

The result matches the previous 61/66 native Sharding baseline; no case IDs or deadlines changed. The report maps all 66 cases to the official Node/Vitest reference, with five outcome mismatches and no mapping or inventory errors.

Provenance:

- Frozen CLI: `kiln-5c968ff32c38a8175754413088d5e62da0e39eefc2615750f1201124a99a806e`, SHA256 `5c968ff32c38a8175754413088d5e62da0e39eefc2615750f1201124a99a806e`.
- Source baseline: root-frozen `5c` binary; the staging build used the exact same CLI and Sharding entry.
- Native profile: `native-release`; process memory cap 4096 MiB; one build/runtime worker.
- Provider manifest SHA256: `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`.
- Provider archive SHA256: `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`.
- Harness SHA256: `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`.
- Runner SHA256: `28211c0c0120db8bcfd0c9380eeb458c4ccbca0a00a0bf4cbb5026ba50af5e61`.
- Full parsed report: `full-replay/report.json` (SHA256 `961a36487bb748533cecfb13849c088491a6924153f0c1c81fcb4af557778553`).
- Raw case log: `full-replay/cluster/Sharding/native.log` (SHA256 `424c3980f7e20c9a621d64a9ce9c13e18947885b4ecb9070b3d9f74a5ea8d4cb`).
- Per-file result: `full-replay/cluster/Sharding/native.json` (SHA256 `3f53994956e9658f541066597b5cddffa6937381aaf75fe23575befa5dffb426`).
- Compressed executable: `full-replay/cluster/Sharding/native.bin.zst` (SHA256 `9a9f37d0a734a27f9315b46d943c5f662483cefbaa273f2a229656a96a32ff42`).
- Staging native binary: `Sharding.native` (SHA256 `082875197e0c4a50e857f56ae349dfdca5347e0263b36856e6e749e91ce22a36`); build log: `stage-build.log`.

The official runner compressed the executable after execution and retained the complete logs and report in `full-replay/`.
