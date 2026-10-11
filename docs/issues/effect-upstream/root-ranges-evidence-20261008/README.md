# Native module-root range visitor evidence

This archive records validation of the root-range visitor change. The release CLI was built at `f0eefa7eedede3dfdc40a6e71815e28226b318cb` with a tracked two-file patch (31,518 bytes; SHA-256 `d50f9f2ef9d1687e4b769c382348fde10ab9bd391b04539553a225c870c37f85`), then that exact patch was committed as `e5915ac13cc20e8e61570400a98eace79d107b5f`. The build used Rust 1.97.1 and the locked, offline release command `mise exec rust@1.97.1 -- mbx build -p kiln-cli --release --offline --locked`. Source pre-build and post-build receipts are included. The focused executable C fixture is `tests::module_graph::emitted_root_ranges_match_legacy_visitation_order_and_failure_behavior` in `crates/native/kiln-native/src/tests/module_graph.rs`. It checks generated root ranges against the legacy visitation order and failure behavior using rendered metadata and an executable fixture.

The focused fixture passed 1/1 (287 filtered). The frozen CLI SHA-256 was `f38fbd75443f9b45d941b8df3306a33fb488226116dbe89e6a86f967b4d1cb9d`. Its provider was ABI 82, manifest `f31f0ceebf4a272400e9667d561d119e5f00d451ae3c072c4783375f6deecc9f`, archive `fe25b4720dd0741d661b60ceceec3ba490e4941cfd21570ec23b9aecb9c14720`.

The original pinned Effect `Hash` file ran all 25 expected cases and modes against Node’s mapped Vitest oracle: 24 passed and one retained its original 5-second timeout, `Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place edits`. The result is not a full Effect-suite acceptance. Generated C was 186,214,878 bytes; the measured compile included contention and is not a performance comparison.

Three targeted native probes passed in each of ordinary, GC-stress, and realms runs. The realms run exercised N=1, 2, and 16. These are targeted checks, not the full native corpus or release gate. The archive includes their raw summaries and machine-readable terminal receipts, the focused fixture log, CLI build log, Hash case report, and Hash native build log.

No speedup is claimed: there is no accepted matched performance measurement or full gate result in this evidence set.

## Reproducibility and limits

`evidence.tar.gz` contains the compact source-independent logs and receipts. `SHA256SUMS` verifies the outer README and archive; `payload/SHA256SUMS` (inside the archive) verifies each archived file. The Hash status records a reaped child, absent process group, and no outer timeout. Each native-probe receipt records unchanged inputs, a reaped child, absent process group, and no disk-guard trigger. The source commit and fixture location above identify the implementation under test. The archive excludes generated C, object files, binaries, and the full upstream checkout.
