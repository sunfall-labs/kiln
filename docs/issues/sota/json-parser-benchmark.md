# JSON parser matched benchmark

The final matched run used the same 1,000-parse batch harness for the old baseline and the
final `41af` candidate. Each row black-boxes the tape checksum; all checksums agree. The raw
logs and the median source are retained in `docs/issues/sota/json-fused-final/`.

Commands:

```sh
mise exec -- mbx test --release -p kiln-json --test matched_timing --offline -- --nocapture
mise exec -- mbx test --release -p kiln-json --test matched_timing --offline -- --ignored --nocapture
```

The first command is the baseline executable at
`~/Developer/Personal/kiln-json-baseline/target/release/deps/matched_timing-1dc06dc969ffc946`
(SHA-256 `56220229f944794ba43c55094a7ef4a57c17959acb4a8fbb91571c1a484efcba`). The ignored
candidate command is the final executable at
`~/Developer/Personal/kiln-string-json/target/release/deps/matched_timing-4ef85e16328094f3`
(SHA-256 `da1ebe6b0a717dda9b08d432fc9c6c4418cb798eb86df93579edd0d15d92796a`).

Medians are microseconds per parse:

| workload | baseline | candidate | candidate vs baseline |
| --- | ---: | ---: | ---: |
| short ASCII | 0.0166 | 0.0172 | +3.6% |
| long ASCII | 1.4030 | 1.4277 | +1.8% |
| escaped | 0.0223 | 0.0273 | +22.4% |
| external UTF-8 Unicode | 0.0197 | 0.0433 | +119.8% |
| number-heavy | 1.3018 | 1.2671 | -2.7% |

There is no overall parser speedup. The number-heavy row improves modestly; short, long, escaped,
and external UTF-8 rows are slower, with the external row including the required UTF-8 adapter.
The candidate's separate existing-`StringUnits` row is `0.0224` us/parse. It is reported
separately because the baseline row prepares UTF-8 bytes, while this row prepares UTF-16 units
outside the timed parse. It does not establish a baseline-vs-candidate speedup.

The three raw baseline/candidate pairs are `1-baseline.log` through `3-candidate.log`; their
checksums are respectively `4`, `1`, `2`, `2`, and `201` for the five shared workloads. The
complete median record is `medians.json`, and the executable paths and hashes are in
`binaries.json`.
