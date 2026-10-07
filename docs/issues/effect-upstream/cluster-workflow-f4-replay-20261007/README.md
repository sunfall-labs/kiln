# ClusterWorkflowEngine native replay on f4

This is the official unchanged upstream replay of `cluster/ClusterWorkflowEngine` against the frozen f4 native CLI. The runner retained the original 47 registered test cases and their 5,000 ms per-case timeout. It completed the full inventory: 46 passed and one failed with the original `TEST_TIMEOUT: 5000ms` outcome. No case was missing, skipped, unmapped, or assigned a different mode; Node/Vitest mapping covered all 47 cases. The failure was `ClusterWorkflowEngine > a parent stays suspended past the activity interrupt retry budget`.

## Provenance

- Frozen production source commit: `f4a22dbbb9bd3df1a2322637d94629232229b5e6` (staging receipt).
- Frozen CLI: `~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/date-native-milestone/.scratch/combined-native-tier-gate/kiln-35fc543aea252fd0b1e8160b6f2871f5e58326f970905cb6836319e5746eddf2`; SHA-256 `35fc543aea252fd0b1e8160b6f2871f5e58326f970905cb6836319e5746eddf2`.
- Provider manifest SHA-256 `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`; its archive SHA-256 is `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`.
- Entry source SHA-256 `02f24e608ccbb680d2a7ecea488ed8c7eda102dbe5d7a2bf79a11a1ac64e0da0`.
- Result native executable SHA-256 `99a9839ba167d0b252d8836b573cce5405ec6ea87e867851e1c92d8cdadcabb8`, matching the staged AOT executable.
- One native worker; 4 GiB child address-space cap; unchanged runner and test deadlines. The replay ran 2026-10-07 06:00:04–06:01:01 UTC (57.245 seconds wall).

The native build log reports 348,737,891 generated-C bytes as `built`, 16.871 seconds in emission, while the application object, prelude object, runtime object, launcher object, and executable were cache hits. This receipt does not treat the build as a generated-C cache hit or make a performance claim. A parent status check found unrelated checker-build PIDs at replay completion; their start times are unavailable, so possible endpoint overlap is recorded as unquantified. My prelaunch process check showed no active native replay/build besides persistent daemon servers.

## Evidence

`evidence/native.json` contains the complete inventory and all 47 case outcomes. `evidence/report.json` retains aggregate counts and the Node/Vitest comparison. Raw runner, native runtime, native build, and staging logs and the exact job/status records are included. SHA-256 checksums:

- `native.json`: `fbfdfd6dadb78e08d955d86d4e88f5a50cc4a96e7875fee306f0178d34d5f99e`
- `report.json`: `f84e1c856eb24d105cfa343f83137eed9365dd36fef55459465d32c4d70049ed`
- `native.log`: `ac13cba1acb51eb0b4cebbed0545dbe451f6002a108eed3e4cd4d7157dfa06ad`
- `native-build.log`: `a07e538e5cd194cf1f87023374d0f9fa888b5b67331a0d768532952736691c56`
- `replay.log`: `93a238b256c001b083b801ebd22dcaf5de375b55020ba807a383de009d4677b2`
- `replay-status.json`: `afde51f474dbe77a4afed6e9a8fab5fbd057616c82bf1f88157c1b3a1f02e3c1`
- `replay-job.json`: `4f808bfb83a3da78b181a8ec270fa8c09d4280fcacddd745336d84d91f936feb`
- `staging-job.json`: `4cc6cc29774cc367b9f2fd7a81d87820c1319926767788ea8bd8b3feb25011a2`
- `staging-status.json`: `191b2399f88db08c25c57a2ef587f1ae56eca95c1a41ed6dd4cf4b998747ef6c`
- `staging-build.log`: `6b59af5522351ec00c73470d7b17a98105e4e3d866caadb9f29f7f070aa70a97`
