# Dense-key milestone gate evidence (2026-10-07)

This archive preserves the successful routine gate for source commit `edff3fc105558bcf74bac098d47dc4c889a8efb5`. The frozen CLI SHA-256 is `c61bb82a8afdd67f718bb3c3a2be7fe92b44b2c37d82b323c01b6ac82eabb948`; the frozen receipt is `gate/frozen-cli.json`. The gate aggregate and per-step envelopes/raw logs are under `gate/run.y7NQwiKM/`, with the job, terminal status, and full gate log beside them.

The gate completed successfully in 3,389.74 seconds. It reports 2,750 crate tests passed, 0 failed, 12 ignored across 110 suites; routine Test262 97,694 passed, 4,440 failed, 384 unsupported, 4 skipped out of 102,522 selected cases, with 116 deferred and 0 chunk regressions across 122 chunks; native parity 664 matched, 5 refused, 0 diverged; 12/12 timed third-party baselines passed; third-party correctness 17 matched, 0 refused, 0 diverged (5 bundles absent/unbuilt); Node conformance 40/40; server 7 passed and 2 skipped; regress 58/58; and whole-program facts 31/31. The source and CLI digests are in the gate aggregate and frozen CLI receipt.

This is the routine gate's scope. It is not a replay of all 322 Effect files. The earlier seven-file / 562-case Effect diagnostic remains separately archived in `dense-collector-matched-cohort-20261007`; it is not upgraded to full-suite evidence by this gate.

The Test262 artifact `test262-baseline-logs-run.BvsnPfO2.tar.zst` contains all 1,258 per-chunk, batch, and inventory files from the gate's result directory. `test262-baseline-SHA256SUMS` lists every file hash before archiving. The archive was tested with zstd and its extracted file hashes were compared against that manifest. `archive-roundtrip.json` records the verified entry count, bytes, and archive digest. `SHA256SUMS` lists the other archived evidence files and the Test262 archive.

The optional cache-cleanup receipt is under `cache-cleanup/`. It records an ordinary GC sweep limited to the inactive owned `native-loop-coverage` cache, reclaiming 26.5 GiB reported payload and leaving the standard 2 GiB cache budget. The receipt explicitly notes that standalone subprocess status and precise filesystem-byte attribution were not separately captured.

To verify the checksums from this directory, run `sha256sum -c SHA256SUMS`. To verify the compressed Test262 archive, run `zstd -t test262-baseline-logs-run.BvsnPfO2.tar.zst`.
