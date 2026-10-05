# Lazy captured-cell Sharding comparison

This records a selected-case comparison of the lazy-cell candidate `bc3e1913` against its exact base `4b5e6c9c`. The candidate did not improve the native Sharding async-reset timeout, so its optimization remains unintegrated. This is not a full Sharding or Cluster replay.

Both binaries ran the same private counter-instrumented Sharding overlay, provider archive, native-release profile, and original 5,000 ms test deadline. Six fresh-process executions alternated baseline/candidate, three each. Every execution hit the original timeout. Baseline median wall time was 5.137 s and candidate median was 5.206 s; user CPU was about 5.0 s for both. The counts also overlap: storage reads were 25–27 for baseline and 25–26 for candidate, while fired timer/yield counts were 159–165 and 160–162. This evidence shows no useful change for this timeout; it does not establish equivalence across the full corpus.

The exact source and binary identities, input hashes, build summaries, commands, per-run stdout/stderr, and timing envelopes are retained in `raw/`. The baseline CLI was `734d4a745fbeea64662d94fb87dcb82fd897600ed584056bff1da30941ba70a`, based on `4b5e6c9c`; the candidate CLI SHA-256 was `e99058cc0bc9772de904e8ab6e314623de0efbfec2ad5e20029c2c1a81425416`, source `bc3e1913`. The same ABI-82 provider archive SHA-256 was `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`.

Build durations are retained for provenance only: the baseline build was cache-cold and candidate build cache-hot, so their build-time difference is not a performance comparison. The baseline command initially streamed output without a separate raw capture; `baseline-build.log` contains the complete final build report and records that limitation.
