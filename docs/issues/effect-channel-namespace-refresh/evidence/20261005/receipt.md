# Channel repeat scaled-operation diagnostic

Diagnostic only; this is not an acceptance result and does not alter or replace the pinned Channel test.

- Frozen CLI: `kiln-b8be79d5c64e824c3f0225024cf7d729f78d4ae68120cc45476cc57ebe616644`
- CLI SHA-256: `b8be79d5c64e824c3f0225024cf7d729f78d4ae68120cc45476cc57ebe616644`
- Invocation: `KILN_JOBS=1 RAYON_NUM_THREADS=1 <frozen-cli> --one-shot run <files/ChannelDiag/entry.ts> --node --clock wall`
- Pinned `Channel.test.ts` SHA-256: `d3207e341113ccda55b1b0f23c0c98db9f7d3219aaac558c59c1231949d8ff6e` (unchanged)
- Diagnostic `probe.test.ts` SHA-256: `df66da8606137c476b447049b279be09fe07a1a0163f41f9557e6c5874321005`
- Pinned `Channel.ts` SHA-256: `68681b99724f8058697a4342e4a19204e41016a8c37a8ad79d4aca711c05f6ed`
- Pinned `Schedule.ts` SHA-256: `dd8a086619963ac4e2cd007cc3ddfb6ce60e263c3429c59ad95a628b264b7db1`
- Pinned `TestClock.ts` SHA-256: `b35d4584199f9f5aaec54bfccf9092a45388b72b617858cdbfe95170a1ac30e3`

The probe copied `countOps`'s custom scheduler (`shouldYield` increments an integer and always returns `false`) and ran `Stream.fromChannel(Channel.repeat/forever(Channel.succeed([1])))`, `Stream.take(n)`, and `Stream.runDrain` for `n = 125, 250, 500`. It intentionally reports the metrics by throwing a diagnostic error after all runs; therefore the one test's failed status is expected and not an acceptance failure.

| path | 125 | 250 | 500 |
| --- | ---: | ---: | ---: |
| `forever` | 2,261 | 4,511 | 9,011 |
| `repeat` | 5,091 | 10,175 | 20,341 |

Both paths show approximately 2x operation totals when `n` doubles through 500. `repeat` uses about 40.7 counted operations per output and `forever` about 18.0. This supports linear work with materially higher fixed per-output work in the scheduled path; it does not establish elapsed-time scaling or identify a specific hot instruction. The original `1,000`/`2,000` case, 5s deadline, and all pinned files were left unchanged.

## Frozen-CLI sampled profile (diagnostic only)

Captured with `samply 0.13.1`, 500 Hz / 2 ms, save-only, pinned to CPU 0. The frozen release CLI contains debug info and the profile's symbol sidecar matches its build ID. The task's CaptureHost worker has 3,183 samples (~6.4 sampled CPU seconds); the separate command/source-sync thread has 236 samples and is excluded from VM hot-path percentages.

| Sampled frame | Leaf samples | Inclusive worker samples |
| --- | ---: | ---: |
| `StringUnits::eq<str>` under `ObjectData::refresh_namespace` | 974 (30.6%) | 1,815 (57.0%) for `refresh_namespace` |
| `StringUnits::eq<&str>` under `ObjectData::refresh_namespace` | 517 (16.2%) | — |
| generic `StringUnits::eq` under `ObjectData::refresh_namespace` | 37 (1.2%) | — |
| `ObjectData::refresh_namespace` itself | 224 (7.0%) | 1,815 (57.0%) |
| `Vm::prepare_property_inner_key` | — | 1,921 (60.4%) |
| `Vm::run_node_event_loop_inner` | — | 2,175 (68.3%) |

The principal sampled work is namespace refresh, not a clock wait. In the frozen source, `read_property_inner_key` calls `refresh_namespace` before every read of a live-binding namespace. `ObjectData::refresh_namespace` loops over `namespace_cells` and for each entry scans `properties` with `iter_mut().find(slot.key == key)`, so the lookup portion is quadratic in the number of namespace exports per read. The samples are consistent with repeated namespace key scans during Channel execution; they do not establish which exact namespace dominates or authorize changing live-binding semantics. A follow-up should identify whether an indexed slot mapping or change-driven refresh can preserve bindings and reentrancy while avoiding this scan.

Profile artifacts:

- `profile-presymbolicated.json.gz`, SHA-256 `de8d9d058a130a469902523e593f15708bbaf4c7a8c6184677eeebf985940503`
- `profile-presymbolicated.json.syms.json`, SHA-256 `bcecb5a10deab1d51864a7060ff9ca7144dd2be1942aefc67433bdfe6d03309e`
- `profile.json.gz`, SHA-256 `96856e9f3f7e2257e89c7347fea48ef444605f34c29689c45286eb7a7649a98e`

For the original `1,000 + 2,000` work, the sampled per-output rates project to about 54,000 operations for `forever` (18 × 3,000) and 121,800 for `repeat` (40.6 × 3,000). These are projections from the scaled diagnostic, not measured original-case totals.
