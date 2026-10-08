# Fresh native Hash and SchemaBinary acceptance

The production CLI at source base `624968060241897edfc7d38b40f18aae80a3d2bc` passed all 218 original direct-TypeScript cases: Hash 25/25 and encoding/SchemaBinary 193/193. Every case ID, mode and outcome matched the pinned Node/Vitest reference. Original case and hook deadlines were retained. The formerly failing Hash law and SchemaBinary zero-width array cases passed.

The measured checkout was `554f5c465d7f042dd429c32479c10dffa78143c0`; its difference from the CLI source base only updates two emitted-C test snapshots. CLI SHA-256: `c1ee6aa03d420d95bb5c52df29bfe8a770ff4816e802442b996e13ff1da085be`. Upstream Effect commit: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.

The supervisor verified unchanged source, CLI, provider, upstream and harness inputs before and after execution, and confirmed the child process group exited. Hash build/execution took 961.900/8.302 seconds; SchemaBinary took 389.233/8.882 seconds. These are acceptance costs, not a comparative performance claim.

This run selected two files and deferred 320. It does not establish completion of the 322-file suite. Fresh full native and interpreter runs, unresolved Cluster/Sharding deadlines, all 23 microbenchmarks and four direct-TS real-world comparisons against Bun, and a successful final gate remain outstanding. The private native cache candidate was not involved in this result.

`status.json` contains the supervisor verdict and exact inventory validation. `run/report.json` and per-file reports retain outcomes; raw logs, the prepared job and supervisor are included. Native binaries and caches are excluded. `manifest.json` records the SHA-256 of each retained evidence file. Absolute paths in receipts identify the original execution environment.
