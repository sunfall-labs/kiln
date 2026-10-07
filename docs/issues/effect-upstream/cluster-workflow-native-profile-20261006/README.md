# ClusterWorkflowEngine native timeout profile (2026-10-06)

This receipt records a diagnostic profile of the original native ClusterWorkflowEngine run. It is not a clean acceptance replay: sampling overhead caused a second timeout elsewhere in the 47-case file. The attached profile was aligned to the reported parent-suspension retry-budget timeout.

## Identity and setup

- Source commit: `f53e22c6d47ef0daad71fa915d4ec8c73d2d6bae` (from the original replay job receipt).
- Frozen CLI SHA-256: `cdae5bcf3bd90f73412e0e0b3d35d0cefc8204fb388502476673d0e700ae6df2`.
- Profiled native executable SHA-256: `3125bfcd12d2e58c4eef7e12fc4f733a0ed885b14594b83b64af648b4166466b`.
- Native number-provider manifest: `provider-4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`; manifest SHA-256 is recorded in the run receipt source location. Provider archive SHA-256: `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`.
- The profiler attached after the case `resumes a fresh entity from a legacy envelope with queued replay true` passed and detached as the parent-suspension timeout output arrived. The full raw run log is retained.
- `samply` ran at 100 Hz for 7 seconds. The saved profile contains 878 samples over 8.70 seconds with 8,694,728 microseconds of `threadCPUDelta` weight. These are sampled CPU weights, not elapsed-time measurements.

The symbol-only relink used the same frozen CLI and provider, one job, and cached generated C and all application/prelude/runtime/launcher objects. Its `.text` and `.rodata` sections match the profiled executable byte-for-byte (length and SHA-256); see `evidence/section-identity-profiled.json`. This validates address-to-symbol mapping without claiming whole-file identity.

## Weighted exclusive attribution

The analyzer assigns each sample to its closest recognized runtime-operation ancestor, avoiding double-counting nested frames. The weights below sum to the sampled CPU total; sampling and inline/unwind limits still apply.

| Group | CPU weight | Share |
| --- | ---: | ---: |
| Property lookup/hash | 2,636,857 µs | 30.3% |
| Method/call dispatch | 2,419,218 µs | 27.8% |
| GC tracing/lookup | 1,392,969 µs | 16.0% |
| Generated bodies | 1,131,373 µs | 13.0% |
| Boundary conversion | 554,326 µs | 6.4% |
| Allocation | 509,958 µs | 5.9% |
| Unclassified | 50,027 µs | 0.6% |

Largest observed leaf weights include `kiln_val_to_boundary` (0.379 s), `kiln_c99_gc_lookup_nursery` (0.355 s), `kiln_c99_invoke_method_dispatch` (0.321 s), `kiln_c99_key_hash` (0.304 s), `kiln_c99_get_own_kind` (0.261 s), `kiln_c99_interned_key_with_hash_in` (0.250 s), `kiln_c99_get_key` (0.240 s), `kiln_c99_gc_finalize` (0.235 s), and `kiln_c99_lookup_method` (0.213 s). These figures identify investigation targets, not isolated costs or proof that key hashing is repeated on cache hits.

## Retained files

`evidence/` contains the compressed raw profile, complete native log, run and relink receipts, section-identity check, analyzer, attribution output, and symbol-relink log. The large executable is intentionally not committed; its immutable compressed original and frozen source workspace remain at the paths in `run.json` and `relink.json` on the originating machine.
