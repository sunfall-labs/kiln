# Null-prototype descriptors and generator stress

The interpreter now respects explicit null-ending prototype chains in HasProperty, including descriptor objects. Own-property hits retain their fast path. Source commit `2e24614a309841a0c856e045be8ac560cd60e211` passed the full interpreter library: 305 passed, zero failed, one ignored. Final integration includes this change as `d80918e8`.

Four representative native probes matched under GC stress: the original r59 async-generator request queue, queued-return protocol, closed reentry, and generator-result own properties. Each reports one matched, zero refused, zero diverged and zero stress probes skipped. Supervisors retained exact source/probe/CLI/provider pins, bounded memory and disk use, and observed reaped leaders and absent process groups. These are representative probes, not the full stress corpus or full Effect acceptance.

The frozen CLI SHA is `537ac9c412bf80bb76fe248060c8d4cb934c6b95bc99ca9b9dd3ee1d55ef9517`. Its build began with the null-prototype edits uncommitted. The retained source mapping reconstructs the exact working diff from committed 945ebdac to 2e24614a, matching both build receipt digests. This proves source-content equivalence; it is not a clean-at-build receipt. Native results are targeted source-content validation.

The earlier 240-second r59 supervisor timeout is retained alongside the successful fresh 1,200-second retry (94.45 seconds). The timeout produced no semantic verdict and is not counted as a test failure or pass. Case deadlines in upstream Effect were not changed.

All original raw streams and receipts are retained under `raw/`; SHA256SUMS covers every artifact except itself. Full native/interpreter Effect replays, final gate, benchmarks and publication remain outstanding.
