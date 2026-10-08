# Unrestricted HttpClient interpreter recheck, 2026-10-08

The earlier full interpreter survey recorded eight `HttpClient` `ServeError` failures while localhost listener access was restricted. After localhost access was restored, this focused replay ran the same pinned upstream file and matched Node/Vitest: all eight unchanged cases passed with matching modes and outcomes. The exact case IDs and before/after provenance are listed in `recorded-cases.json` and `metadata.json`.

This is a one-file recheck, not a rerun of the 322-file interpreter survey; it does not change that survey's recorded totals or establish full-suite acceptance. The replay used the original 5-second per-test and 10-second before-each hook deadlines, the pinned Effect/CLI/runner/Node identities, one interpreter worker, and a 4 GiB kiln child cap. No dependencies were installed. Source, runner, harness, Node, and CLI inputs remained unchanged, and the supervisor confirmed process-group cleanup.

The report, raw interpreter log and supervisor records are preserved as verified zstd archives. Each decompressed SHA matches its source. No executable binary is included. `SHA256SUMS` verifies the archive files.
