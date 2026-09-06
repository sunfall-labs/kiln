# Issue 03: Emitted C Chunk Worker Lowering in `kiln-backend-c`

Status: planned
Feature: rust-parallel-engine
Governing ADR: ADR 0058

## Context
Calling Rust across FFI on every array element introduces prohibitive indirect call overhead. Chunked Loop Inversion inlines the inner loop in C while delegating work-stealing and chunk scheduling to Rust.

## Requirements
1. Update `crates/kiln-backend-c/src/emit_graph/ops/call_method.rs` for `parallelMap` to generate a static C chunk worker function with inlined, unboxed closure execution.
2. Pass the chunk worker function pointer and array slice pointers into `kiln_parallel_map`.
3. Implement `parallelReduce` in `call_method.rs` generating a static C chunk reducer function and passing it to `kiln_parallel_reduce`.
4. Lower `%TypedArray%.prototype.sort` and numeric array `sort` on large arrays to `kiln_parallel_sort`.

## Acceptance Criteria
- Emitted C units compile cleanly under `zig cc` without warnings.
- `mbx test -p kiln-backend-c` passes all unit and emitted C tests.
- Clang successfully auto-vectorizes the inner chunk loop.
