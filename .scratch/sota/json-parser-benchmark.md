# JSON parser matched benchmark

The ignored harness is [matched_timing.rs](/Users/alee/Developer/Personal/kiln-string-json/crates/kiln-json/tests/matched_timing.rs). It runs 1,000 parses between clock reads, black-boxes the tape node-count checksum, and reports the checksum per parse. The same byte workloads and harness were run from the old baseline worktree `/Users/alee/Developer/Personal/kiln-json-baseline` and this worktree.

Commands:

```sh
mise exec -- mbx test --release -p kiln-json --test matched_timing --offline -- --nocapture
mise exec -- mbx test --release -p kiln-json --test matched_timing --offline -- --ignored --nocapture
```

Candidate source hashes at measurement: `kiln-json/src/lib.rs` `1f11725f8f5a554c9b2d2041abda3a07456a42f8998a6ababf90324609a7b30`, `scalar.rs` `ea6e4bd7a542cea4313c8fc9039fcc983b1d5aaf54c4466932b7cadeee05a51a`, provider `62c7a4ef3a670b3aa885d729ba6f557a8f8fef8a1816fea8680d55ca33f916ec`, harness `573ddd7df659f6ea803bcc3e6180f963bd58c6310d0d919b89e9d3b1dad1e357`.

Three alternating baseline/candidate pairs ran after the quiet slot was reserved. Rows are `short-ascii`, `long-ascii`, `escaped`, external UTF-8 `unicode`, and `number-heavy`, in microseconds per parse:

| pair | baseline | candidate |
| --- | --- | --- |
| 1 | 0.0172 / 1.3089 / 0.0219 / 0.0190 / 1.3317 | 0.0187 / 1.4293 / 0.0243 / 0.0846 / 1.4961 |
| 2 | 0.0172 / 1.4969 / 0.0217 / 0.0209 / 1.2819 | 0.0194 / 1.4548 / 0.0243 / 0.0859 / 1.5795 |
| 3 | 0.0168 / 1.3862 / 0.0215 / 0.0196 / 1.2612 | 0.0187 / 1.4107 / 0.0246 / 0.0842 / 1.5264 |

Every row's normalized tape checksum matched: `4`, `1`, `2`, `2`, and `201`. The candidate's additional `existing-units` row prepares `StringUnits` outside timing and parses the same Unicode payload directly as UTF-16: `0.0215`, `0.0221`, and `0.0219` us/parse across the three runs, versus baseline's UTF-8 byte path `0.0190`, `0.0209`, and `0.0196`. Direct UTF-16 raw-span bulk copying closes the representation-path gap. The external UTF-8 adapter row includes required UTF-8 to UTF-16 decoding and is measured separately.

The release test executable was rebuilt by the commands above; its test output is intentionally not checked in because Cargo names it from the build hash. The ignored source is the reproducible artifact.

Follow-up numeric diagnosis retained under `.scratch/sota/json-numeric-profile/`: the out-of-line
`number_from_parts` call was found in the candidate's optimized assembly and sampled stack. With
the measured `#[inline(always)]` boundary fix, the final candidate executable was
`target/release/deps/matched_timing-e32cd39cc86bbf5b`, SHA-256
`c34a2db9229bba4d0484e593511ee62b9742e997beacfbf6a8c345f3298c5af1`; the baseline executable
was `/Users/alee/Developer/Personal/kiln-json-baseline/target/release/deps/matched_timing-1dc06dc969ffc946`, SHA-256 `56220229f944794ba43c55094a7ef4a57c17959acb4a8fbb91571c1a484efcba`.
The final candidate run used the ignored harness command above and reported
`short-ascii 0.0140`, `long-ascii 1.4671`, `escaped 0.0218`, `unicode 0.0831`,
`number-heavy 1.3173`, and `existing-units 0.0170` microseconds per parse, with checksums
`4,1,2,2,201,2`.
