# Native test-cache override and Sharding replay receipts

This archive preserves two focused checks performed on 2026-10-07. It is not a
whole-gate result and makes no whole-gate speedup claim.

## Test-only native artifact-cache override

The exact test command and environment are in `cache-override/job.json`; the
supervisor receipt is `cache-override/status.json`, and the complete output is
`cache-override/test.log`. It selected the existing native cyclic-module
execution fixture exactly once:

`tests::module_graph::native_graph_cycles_instantiate_live_export_cells_before_evaluation`

The configured `KILN_NATIVE_TEST_CACHE_ROOT` was the immutable artifact cache
from an inactive owned worktree. The test passed **1/1**; the Rust test itself
reported **3.23 seconds**. The complete command, including Cargo compilation,
took 71.61 seconds. The test executed normally; only content-addressed build
artifacts are shared. This single fixture result does not measure or establish
a gate-wide time reduction.

## Original Sharding stage and quiet replay

`sharding/stage/` retains the stage job, terminal status, build log and binary
receipt. The stage used the pinned `cluster/Sharding` entry at source pin
`67ba4e46a11ccda0b6761578bfd22c04ae00167d`, compiler source commit
`c06c78329eb9c30f2c8ab44007b5054bb4a1dc2b`, CLI SHA-256
`af80860eda702b4d6d6742ca15e5b23013fe65649e759b0078aba1ffdef73d27`, and
provider manifest SHA-256
`4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`.
Stage wall time was **618.762 seconds** (the compiler's inner build record
reports 616.315 seconds). The staged executable's SHA-256 is recorded in
`sharding/stage/binary-receipt.json`; the executable itself is intentionally
not archived.

`sharding/replay/` preserves the prebuilt replay provenance, full case report,
native output and process snapshots. The unchanged original 66 cases completed
in **48.117 seconds** according to the runner report: **61 passed, 5 failed**,
with exact Node/Vitest mapping for all 66 and no mode or mapping errors. The
same five expected-pass cases remained failures; this archive records no
correctness improvement for them. The supervisor wall time was 48.924 seconds,
which includes process-wrapper overhead. Original per-case deadlines and
assertions were unchanged.

`sharding/failed-watcher/` is a separate diagnostic attempt, not a second case
result. Its wait supervisor failed because the watched systemd stage invocation
changed between observation and its assertion (`stage invocation changed`).
That failure occurred in the watcher before it could launch the replay; it does
not replace or invalidate the separately recorded direct prebuilt replay.

`SHA256SUMS` covers every archived receipt and raw log. Large CLI, native
executable, caches and provider archive are not included; their identities are
preserved in the receipts.
