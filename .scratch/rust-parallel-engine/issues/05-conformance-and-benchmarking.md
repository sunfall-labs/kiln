# Issue 05: Conformance Verification & Multicore Benchmarking

Status: planned
Feature: rust-parallel-engine
Governing ADR: ADR 0058

## Context
High-performance concurrency must be rigorously verified against mathematical correctness, associative reductions, and real-world multicore benchmarks.

## Requirements
1. Add native conformance fixture `conformance/native/parallel-reduce.ts` validating parallel map, reduction, and sorting correctness.
2. Add effect fixture `conformance/effect/src/16-parallel-map-reduce.ts`.
3. Add microbenchmark `bench/non-io/mapreduce-multicore.mjs` measuring 5M–10M element MapReduce scaling against Node.js and Bun.
4. Validate that `scripts/native.sh` and `mise run gate` pass with zero regressions.

## Acceptance Criteria
- `scripts/native.sh` passes all probes with zero divergence.
- Multicore MapReduce shows linear scaling across available CPU cores.
- Full gate pass (`mise run gate`).
