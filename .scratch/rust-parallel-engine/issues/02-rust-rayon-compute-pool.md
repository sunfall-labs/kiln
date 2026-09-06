# Issue 02: Bounded Rayon Compute Pool & Chunk Scheduler

Status: planned
Feature: rust-parallel-engine
Governing ADR: ADR 0057, ADR 0058

## Context
Data-parallel loop execution in native binaries requires work-stealing across available CPU cores. Per `AGENTS.md` and `NOTES.md`, Kiln enforces strict CPU budgeting on multi-agent hosts and forbids the unbound global Rayon pool.

## Requirements
1. Implement a private, bounded `rayon::ThreadPool` inside `kiln-runtime` / `kiln-native-number-ffi` with core count auto-tuned to machine availability.
2. Implement `kiln_parallel_map`, `kiln_parallel_reduce`, and `kiln_parallel_sort` entry points.
3. Provide adaptive chunking: small arrays ($N < 2048$) execute inline sequentially without thread-hop overhead; large arrays partition into cache-friendly chunks across workers.
4. For `kiln_parallel_reduce`, collect partial chunk accumulators across worker cores and compute the final reduction.
5. For `kiln_parallel_sort`, delegate numeric arrays to Rayon's `par_sort_unstable_by`.

## Acceptance Criteria
- `mbx test -p kiln-native-number-ffi` passes with unit tests for parallel map, reduce, and sort.
- Memory safety verified: zero data races across chunk boundaries.
