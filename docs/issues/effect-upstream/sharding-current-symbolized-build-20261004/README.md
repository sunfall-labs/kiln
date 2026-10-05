Current candidate Sharding build preparation

Build-only evidence: no cases were executed and no correctness or performance verdict is implied. The unmodified pinned 66-case Sharding entry was compiled at native-release with local symbols retained for an attributable profile. Source, CLI and provider hashes are in receipt.json. Binary SHA-256: 1daaeb90e8c07f7846d913329f00d4e36fe12cd1f2327d3430feab49afbdbe8f. ELF Build ID: 4187d30bdf113afd9391abf0f9049d4faf54cfd6. Build completed exit 0 in 769.158 seconds. Native binary remains in the owned worktree under .scratch/sharding-current-symbolized-prebuild/native.bin. Case deadlines are unchanged; quiet replay and profile remain pending.

Quiet replay completed: 66 registered and recorded, 60 passed, six failed with original TEST_TIMEOUT 5000, zero skipped, exit 1, no outer timeout, 54.384 seconds. All 66 map to the official Node/Vitest oracle. No owned builds or other test/benchmark workloads ran concurrently. An earlier replay briefly overlapped push hooks; it is retained in .scratch but is not the quiet evidence.

A separately labeled 500 Hz samply diagnostic profile ran the same binary with original test deadlines and no stress environment. It also records 60 passes and six failures. Retained ELF Build ID matches receipt. Native leaf summary is sampled CPU attribution across the whole suite, not proof of timeout cause; inclusive stacks and failing-case windows require further analysis. Profile run cwd was root owned worktree; quiet verdict run cwd was the original suite workspace. Neither run claims full 322-file conformance.

# Sharding native profile attribution

This is source-only analysis of the quiet, unmodified 66-case Sharding run and its separate sampled run. It adds no runtime changes and does not assign profile samples to individual test cases.

The quiet run registered 66 cases: 60 passed, six failed at the unchanged 5,000 ms case deadline, and none skipped. It completed in 54.384 s. The failures were:

- Three claim-release regressions: active request after entity defect; active uninterruptible request after restart; active stream after acknowledged chunk.
- `redelivers a request reset while an asynchronous storage read is pending`.
- `holds durable messages while entity layers are still building`.
- `defects durable messages when no entity ever registers`.

The native binary was SHA-256 `1daaeb90e8c07f7846d913329f00d4e36fe12cd1f2327d3430feab49afbdbe8f`, ELF Build ID `4187d30bdf113afd9391abf0f9049d4faf54cfd6`. It came from CLI `16958abcd29f328d23bb789fc188f0e55a065e349b0b77736ce0354ba1820ceb`, pinned test source `0040998907b12af77ac53b533b0ddf19ca9f7bac552502d524d256a72696fed0`, provider manifest `b03c48a204de3f911b424ce62b7d96a88ef595de0f99217cba8c34caf6c5dd6e`, and provider archive `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`. The sidecar's native image Code ID is the matching ELF Build ID.

Run `analyze-profile.py PROFILE.json.gz PROFILE.syms.json --source-root PINNED_PROJECT_ROOT` to regenerate `profile-analysis.json`. The script decodes the sidecar's address-to-symbol map, walks sampled stacks leaf-to-root, calculates inclusive sample counts and caller relationships, and verifies module path hashes using the pair-hash algorithm in `crates/native/kiln-backend-c/src/identity.rs`. For these inputs, the profile contains 23,870 samples whose leaf frame belongs to `native.bin`, over monotonic sample timestamps 840884621.143 through 840938836.210 ms.

The largest inclusive entries are `kiln_c99_invoke_closure_body` (84.5%), `kiln_c99_call_inner` (79.3%), `internal/effect.ts` generated function 50 (54.3%), `ku_call_fast` (49.6%), and `internal/effect.ts` generated function 48 (49.5%). The runtime timer/drive path (`kiln_c99_run_timer`, `drive_one_ready_turn`, `run_one_timer`) is each about 24.7–24.9% inclusive. These percentages overlap because an inclusive sample is counted in every ancestor frame.

The exact pair hash `9f9747a19882905c65626196c02048b1` resolves to `source/packages/effect/src/internal/effect.ts`. Generated-C comments identify functions 50 and 48 with source ranges `14775..16556` and `13616..14772`, respectively. Function 50's span crosses several top-level declarations and does not establish a trustworthy source method name, so it remains identified by generated function number and source span only. Most samples in function 50 have function 48 as the outward caller. No more specific source attribution is claimed.

The pair hash `d26041571cf61f0bad70365407312980` resolves to `source/packages/effect/src/cluster/Sharding.ts`. Generated-C function 77 has source range `44860..48905` and 916 bytecode instructions. That range is the `sendOutgoing` closure, including local/remote routing, durable-message handling, retry scheduling, and shutdown abandonment. It is 38.3% inclusive but only 3.7% leaf, so much of its inclusive time is in callees. Its shape is a plausible follow-up target for the timeout group, especially tests involving retries or unavailable entities; this whole-suite profile does not prove that these samples occurred during the six timeout windows.

## Result-marker profile: per-case windows

A second profile reused the identical native binary and retained stdout arrival timestamps as pairs of Unix and monotonic nanoseconds. The native console host flushes each newline (`crates/native/kiln-native-runtime/src/lib.rs`), so these timestamps follow the case-result records promptly. samply sample times are monotonic milliseconds; the marker clock pair's Unix-minus-monotonic offset changed by only 3,797 ns across the run. The first inventory marker was 173.121 ms after the sampled process thread registered, and the last sample was 3.292 ms after the final summary marker. The analyzer correlates these clocks directly.

The analyzer reports each interval from the preceding inventory/result marker to the current case result. These windows include harness hooks, cleanup and setup around a case, so they are broader than the test callback itself. The six failing windows are:

| Case | Marker interval | Native samples | `sendOutgoing` fn77 inclusive | `internal/effect` fn50 inclusive | timer runner inclusive |
| --- | ---: | ---: | ---: | ---: | ---: |
| Active request replay after entity defect | 5.057 s | 2,151 | 42.5% | 53.0% | 24.9% |
| Active uninterruptible request after restart | 5.116 s | 1,711 | 39.9% | 56.3% | 28.6% |
| Active stream after acknowledged chunk | 5.064 s | 2,244 | 41.4% | 54.0% | 26.2% |
| Reset during asynchronous storage read | 5.030 s | 2,350 | 42.3% | 53.4% | 25.4% |
| Durable messages while entity layers build | 5.040 s | 2,306 | 38.3% | 53.2% | 26.4% |
| Durable messages with no entity registration | 5.039 s | 2,330 | 40.6% | 51.3% | 24.6% |

Passing controls have comparable stack shares: the adjacent remote-reset storage-read pass has 41.3% in fn77, 53.1% in fn50 and 22.3% in timer runner; durable-message retry-on-restart has 32.3%, 55.6% and 26.6%; and the registration-missing local-send pass has 38.9%, 50.8% and 22.2%. Thus fn77 participates in all six failures, but its share is not unique to failing cases. The profile shows sustained Sharding/scheduler activity until each timeout; it does not identify a single pathological native primitive or distinguish excess retries from an expected state transition that never occurs.

The next discriminating probe should run each unchanged failing case independently alongside a nearby passing control and record test-tagged counts for `sendOutgoing` attempts, retry scheduling, storage reads/claims/resets, and timer/fiber turns. That would show whether the failures repeatedly retry or stop making the state transition the test awaits. No implementation change or benchmark conclusion follows from this sampled run: its receipt records that an unrelated Rust identity-test build overlapped the initial diagnostic seconds, and a separate full322 run was active.

The reproducible `analyze-profile.py` accepts `--markers-jsonl` and `--clock-receipt` to create `marker-profile-analysis.json`. For this profile, SHA-256 values are: gzipped profile `fc4b93ed1cd36a850028d3b74e169c219143f04555eabc47c6c2ce4df50119b0`; symbol sidecar `6204ec678f93b459c936338147656a3ce4995ef8fdf34456189f7b24749986a9`; marker arrivals `0ce7b362cb70818d9bfc210fdcb2d40e95e66637fe6363c7191bea95a8a4d04b`; receipt `9c3a3136b168d1e62db62a875554620ccda5125b2bc13ccb25ce1c2ca0f69fd9`. Binary SHA and Build ID match the earlier run above.

Source evidence: the quiet result and logs, `profile.json.gz`, symbol sidecar, and leaf summary are retained in the sibling scratch directory `sharding-current-symbolized-prebuild` under `kiln-effect-next-milestone`. Their SHA-256 values are: profile `1cc284094f40dcb2fa1c052e3cd08a483b4322ea5fcf4568e6e70f4c8d39eaac`; symbol sidecar `3b13c8bcd7b59d00284227fe23b66fe0b96c21e928076319da542d894275ef30`; leaf summary `d7fab1c2e5ffaf57ebaa96575a4e5edac874436d33e3a0ed2314c9a924267d8c`; quiet result `c5ffc869ee04604f99d955e639adb56c08cb66f8b40d3b91a9ff79431e71a451`.
