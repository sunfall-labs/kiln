# Issue 02: Bounded Rayon Compute Pool & Chunk Scheduler

Status: done
Feature: rust-parallel-engine
Governing ADR: ADR 0057, ADR 0058

Reconciled 2026-09-06 to ADR 0058 (revised): `parallelMap` only, synchronous, lowered to a pool only for a callback the backend proves allocates nothing, else refused with the reason; no pinned ABI version (the archive was at 31 after the JSON reader, c963d4bc); `parallelReduce` and `parallelSort` are not defined anywhere and are out of scope; simd-json was measured against a scalar Rust reader for JSON and dropped (see `.scratch/json-tape/NOTES.md`).

## Context
Data-parallel loop execution in native binaries requires work-stealing across available CPU cores. Per `AGENTS.md` and `NOTES.md`, Kiln enforces strict CPU budgeting on multi-agent hosts and forbids the unbound global Rayon pool.

## Requirements
1. Implement a private, bounded `rayon::ThreadPool` inside `kiln-runtime` / `kiln-native-number-ffi` with core count auto-tuned to machine availability.
2. Implement the `kiln_parallel_map` entry point, with a sequential floor below which the chunk runs inline on the calling thread (a fork-join over 5000 multiply-adds is slower than the loop).
3. Provide adaptive chunking: small arrays ($N < 2048$) execute inline sequentially without thread-hop overhead; large arrays partition into cache-friendly chunks across workers.
4. For `kiln_parallel_reduce`, collect partial chunk accumulators across worker cores and compute the final reduction.
5. For `kiln_parallel_sort`, delegate numeric arrays to Rayon's `par_sort_unstable_by`.

## Acceptance Criteria
- `mbx test -p kiln-native-number-ffi` passes with unit tests for parallel map, reduce, and sort.
- Memory safety verified: zero data races across chunk boundaries.

* **2026-09-06:** landed with the commit titled "parallelMap runs its qualifying callback on a Rayon pool": `kiln_parallel_map` (ABI 32, feature `parallel`), the private Rayon pool with a measured floor of 16,384, the `parallel_safe` predicate and parallel clone in `kiln-backend-c`, `conformance/native/parallel-map.ts` and `bench/non-io/parallel-map-large.mjs`. See ADR 0058 application 2 for the numbers.
