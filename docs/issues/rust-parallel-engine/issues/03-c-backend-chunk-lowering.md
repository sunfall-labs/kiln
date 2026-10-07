# Issue 03: Emitted C Chunk Worker Lowering in `kiln-backend-c`

Status: done
Feature: rust-parallel-engine
Governing ADR: ADR 0058

Reconciled 2026-09-06 to ADR 0058 (revised): `parallelMap` only, synchronous, lowered to a pool only for a callback the backend proves allocates nothing, else refused with the reason; no pinned ABI version (the archive was at 31 after the JSON reader, c963d4bc); `parallelReduce` and `parallelSort` are not defined anywhere and are out of scope; simd-json was measured against a scalar Rust reader for JSON and dropped (see `docs/issues/json-tape/NOTES.md`).

## Context
Calling Rust across FFI on every array element introduces prohibitive indirect call overhead. Chunked Loop Inversion inlines the inner loop in C while delegating work-stealing and chunk scheduling to Rust.

## Requirements
0. First, the gate: a backend predicate proving the callback allocates nothing and reads no heap object but its input element (numeric arithmetic over a numeric array, no upvalue reads of heap values, builtin calls from a fixed allowlist). `is_pure()` was not that predicate and was deleted on 2026-09-06. Anything the predicate rejects is refused with the reason named, following `conformance/native/*-refuses.ts`.
1. Update `crates/native/kiln-backend-c/src/emit_graph/ops/call_method.rs` for `parallelMap` to generate a static C chunk worker function with inlined, unboxed closure execution.
2. Pass the chunk worker function pointer and array slice pointers into `kiln_parallel_map`.
3. Implement `parallelReduce` in `call_method.rs` generating a static C chunk reducer function and passing it to `kiln_parallel_reduce`.
4. Lower `%TypedArray%.prototype.sort` and numeric array `sort` on large arrays to `kiln_parallel_sort`.

## Acceptance Criteria
- Emitted C units compile cleanly under `zig cc` without warnings.
- `mbx test -p kiln-backend-c` passes all unit and emitted C tests.
- Clang successfully auto-vectorizes the inner chunk loop.

* **2026-09-06:** landed with the commit titled "parallelMap runs its qualifying callback on a Rayon pool": `kiln_parallel_map` (ABI 32, feature `parallel`), the private Rayon pool with a measured floor of 16,384, the `parallel_safe` predicate and parallel clone in `kiln-backend-c`, `conformance/native/parallel-map.ts` and `bench/non-io/parallel-map-large.mjs`. See ADR 0058 application 2 for the numbers.
