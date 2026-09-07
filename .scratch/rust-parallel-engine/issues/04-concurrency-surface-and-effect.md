# Issue 04: TypeScript Concurrency Surface & Effect v4 Integration

Status: wontfix
Feature: rust-parallel-engine
Governing ADR: ADR 0057, ADR 0058

Reconciled 2026-09-06 to ADR 0058 (revised): `parallelMap` only, synchronous, lowered to a pool only for a callback the backend proves allocates nothing, else refused with the reason; no pinned ABI version (the archive was at 31 after the JSON reader, c963d4bc); `parallelReduce` and `parallelSort` are not defined anywhere and are out of scope; simd-json was measured against a scalar Rust reader for JSON and dropped (see `.scratch/json-tape/NOTES.md`).

## Context
Developers writing concurrency pipelines need ergonomic, strictly-typed access to parallel reductions and sorting in `@kiln/concurrency` and `@kiln/effect`.

## Requirements
1. Expose `parallelReduce<T, R>(identity: R, fn: (acc: R, item: T) => R): Promise<R>` on `Array.prototype` and `@kiln/concurrency`.
2. Expose `parallelSort<T>(compareFn?: (a: T, b: T) => number): Promise<T[]>` on `Array.prototype` and `@kiln/concurrency`.
3. Provide dual-mode interpreted/Node.js fallback using chunked asynchronous execution.
4. Integrate with `@kiln/effect` via `reduceParallel` and `sortParallel` fiber operators.

## Acceptance Criteria
- `tsc --strict` passes cleanly across `packages/kiln-concurrency` and `packages/kiln-effect`.
- Package unit tests pass under both Kiln and Node.js.
