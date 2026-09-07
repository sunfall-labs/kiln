# Issue 05: Conformance Verification & Multicore Benchmarking

Status: planned
Feature: rust-parallel-engine
Governing ADR: ADR 0058

Reconciled 2026-09-06 to ADR 0058 (revised): `parallelMap` only, synchronous, lowered to a pool only for a callback the backend proves allocates nothing, else refused with the reason; no pinned ABI version (the archive was at 31 after the JSON reader, c963d4bc); `parallelReduce` and `parallelSort` are not defined anywhere and are out of scope; simd-json was measured against a scalar Rust reader for JSON and dropped (see `.scratch/json-tape/NOTES.md`).

## Context
High-performance concurrency must be rigorously verified against mathematical correctness, associative reductions, and real-world multicore benchmarks.

## Requirements
1. Add native probes: `conformance/native/parallel-map-refuses.ts` (a callback that allocates, which the gate must refuse by name) and one with a numeric callback both engines must agree on; `conformance/native/multicore-concurrency.ts` already compares the two engines' `parallelMap` without a polyfill.
2. (dropped: reduce/sort are out of scope)
3. Add microbenchmark `bench/non-io/mapreduce-multicore.mjs` measuring 5M–10M element MapReduce scaling against Node.js and Bun.
4. Validate that `scripts/native.sh` and `mise run gate` pass with zero regressions.

## Acceptance Criteria
- `scripts/native.sh` passes all probes with zero divergence.
- Multicore MapReduce shows linear scaling across available CPU cores.
- Full gate pass (`mise run gate`).
